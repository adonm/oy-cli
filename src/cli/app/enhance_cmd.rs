//! `oy enhance` subcommand: run audit + review, choose findings,
//! then address them one committed change at a time.

use anyhow::{Context as _, Result, bail};
use clap::Args;
use dialoguer::{MultiSelect, theme::ColorfulTheme};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::audit::{self, AuditOutputFormat};
use crate::config::{self, SafetyMode};
use crate::model;
use crate::review;
use crate::session::{self, Session};

const AUDIT_REPORT: &str = ".tmp/oy-enhance/audit.md";
const REVIEW_REPORT: &str = ".tmp/oy-enhance/review.md";
const STATE_FILE: &str = ".tmp/oy-enhance/state.json";
const MAX_FINDINGS: usize = 40;

#[derive(Debug, Args, Clone)]
pub(super) struct EnhanceArgs {
    #[arg(
        long,
        alias = "agent",
        default_value = "default",
        help = "Selection/remediation mode: ask/edit picks findings interactively; auto addresses as many as possible unattended"
    )]
    pub(super) mode: SafetyMode,
    #[arg(
        long,
        value_name = "TARGET",
        help = "Optional branch/commit/ref for the review step; omitted reviews the whole workspace"
    )]
    pub(super) review_target: Option<String>,
    #[arg(
        long,
        value_name = "N",
        default_value_t = audit::DEFAULT_MAX_REVIEW_CHUNKS,
        help = "Maximum audit chunks before failing closed"
    )]
    pub(super) audit_max_chunks: usize,
    #[arg(
        long,
        value_name = "N",
        default_value_t = review::DEFAULT_MAX_REVIEW_CHUNKS,
        help = "Maximum review chunks before failing closed"
    )]
    pub(super) review_max_chunks: usize,
    #[arg(
        value_name = "FOCUS",
        help = "Optional audit/review/remediation focus text"
    )]
    pub(super) focus: Vec<String>,
}

use crate::audit::report::{EnhanceFinding as Finding, FindingSource, parse_findings};

#[derive(Debug, Clone)]
struct EnhancePlan {
    focus: String,
    review_target: Option<String>,
    findings: Vec<Finding>,
    selected: Vec<usize>,
    next: usize,
    resumed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EnhanceState {
    version: u8,
    focus: String,
    review_target: Option<String>,
    findings: Vec<StateFinding>,
    selected: Vec<usize>,
    next: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StateFinding {
    source: String,
    title: String,
    body: String,
}

pub(super) async fn enhance_command(args: EnhanceArgs) -> Result<i32> {
    let started = std::time::Instant::now();
    let root = config::oy_root()?;
    ensure_git_workspace(&root)?;
    ensure_clean_workspace(&root)?;

    let model = model::resolve_model(None)?;
    let focus = args.focus.join(" ");
    let unattended = args.mode == SafetyMode::AutoAll;
    if !crate::ui::is_quiet() {
        crate::ui::section("enhance");
        crate::ui::kv("workspace", root.display());
        crate::ui::kv("model", &model);
        crate::ui::kv("mode", args.mode.name());
        crate::ui::kv("remediate", remediation_mode(args.mode).name());
        crate::ui::kv(
            "review target",
            args.review_target.as_deref().unwrap_or("whole workspace"),
        );
        if !focus.trim().is_empty() {
            crate::ui::kv("focus", crate::ui::compact_preview(&focus, 100));
        }
    }

    let state_path = root.join(STATE_FILE);
    let mut plan =
        load_or_create_plan(&root, &model, &focus, unattended, &args, &state_path).await?;
    if plan.findings.is_empty() {
        cleanup_state(&state_path)?;
        crate::ui::success("no addressable findings found");
        return Ok(0);
    }
    if plan.selected.is_empty() {
        crate::ui::warn("no findings selected");
        cleanup_state(&state_path)?;
        return Ok(0);
    }
    let selected_total = plan.selected.len();

    let mut addressed = 0usize;
    while plan.next < plan.selected.len() {
        let selection_position = plan.next;
        let index = plan.selected[selection_position];
        let finding = plan
            .findings
            .get(index)
            .with_context(|| format!("invalid enhance resume state: finding index {index}"))?
            .clone();
        if !crate::ui::is_quiet() {
            crate::ui::section(&format!(
                "enhance {}/{}",
                selection_position + 1,
                selected_total
            ));
            crate::ui::kv("finding", finding.summary());
        }

        match address_one(&root, &model, args.mode, &plan.focus, &finding).await {
            Ok(AddressOutcome::Committed(hash)) => {
                addressed += 1;
                plan.next += 1;
                save_state(&state_path, &plan)?;
                crate::ui::success(format_args!(
                    "committed {} ({})",
                    short_hash(&hash),
                    finding.summary()
                ));
            }
            Ok(AddressOutcome::NoChange(answer)) => {
                plan.next += 1;
                save_state(&state_path, &plan)?;
                crate::ui::warn(format_args!(
                    "skipped without changes: {}{}",
                    finding.summary(),
                    answer
                        .trim()
                        .is_empty()
                        .then(String::new)
                        .unwrap_or_else(|| format!(
                            " — {}",
                            crate::ui::compact_preview(&answer, 120)
                        ))
                ));
            }
            Err(err) if unattended => {
                crate::ui::warn(format_args!(
                    "skipping {} after error: {err}",
                    finding.summary()
                ));
                reset_workspace(&root)?;
                plan.next += 1;
                save_state(&state_path, &plan)?;
            }
            Err(err) => return Err(err),
        }
    }
    cleanup_state(&state_path)?;

    if crate::ui::is_json() {
        let payload = serde_json::json!({
            "findings": plan.findings.len(),
            "selected": selected_total,
            "commits": addressed,
            "resumed": plan.resumed,
            "elapsed_ms": started.elapsed().as_millis(),
        });
        crate::ui::line(serde_json::to_string_pretty(&payload)?);
    } else {
        crate::ui::success(format_args!(
            "enhance complete: {addressed} commit(s), temporary reports were written under .tmp/oy-enhance ({})",
            crate::ui::format_duration(started.elapsed())
        ));
    }
    Ok(0)
}

impl EnhanceState {
    fn into_plan(self) -> Result<EnhancePlan> {
        if self.version != 1 {
            bail!("unsupported enhance resume state version: {}", self.version);
        }
        let findings = self
            .findings
            .into_iter()
            .map(|finding| {
                Ok(Finding {
                    source: FindingSource::parse(&finding.source)?,
                    title: finding.title,
                    body: finding.body,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        validate_plan_indices(&findings, &self.selected, self.next)?;
        Ok(EnhancePlan {
            focus: self.focus,
            review_target: self.review_target,
            findings,
            selected: self.selected,
            next: self.next,
            resumed: true,
        })
    }
}

impl From<&Finding> for StateFinding {
    fn from(finding: &Finding) -> Self {
        Self {
            source: finding.source.label().to_string(),
            title: finding.title.clone(),
            body: finding.body.clone(),
        }
    }
}

async fn load_or_create_plan(
    root: &Path,
    model: &str,
    focus: &str,
    unattended: bool,
    args: &EnhanceArgs,
    state_path: &Path,
) -> Result<EnhancePlan> {
    if state_path.exists() {
        let plan = load_state(state_path)?;
        if !crate::ui::is_quiet() {
            crate::ui::kv(
                "resume",
                format_args!("{} / {} selected", plan.next, plan.selected.len()),
            );
        }
        return Ok(plan);
    }

    let audit_path = root.join(AUDIT_REPORT);
    let review_path = root.join(REVIEW_REPORT);
    let (findings, from_reports) = if audit_path.exists() || review_path.exists() {
        (
            collect_findings_from_optional_reports(&audit_path, &review_path)?,
            true,
        )
    } else {
        (run_reports(root, model, focus, args).await?, false)
    };

    let selected = if unattended {
        (0..findings.len()).collect::<Vec<_>>()
    } else {
        choose_findings(&findings)?
    };
    validate_plan_indices(&findings, &selected, 0)?;
    let plan = EnhancePlan {
        focus: focus.to_string(),
        review_target: args.review_target.clone(),
        findings,
        selected,
        next: 0,
        resumed: from_reports,
    };
    save_state(state_path, &plan)?;
    remove_internal_reports(&[&audit_path, &review_path])?;
    Ok(plan)
}

async fn run_reports(
    root: &Path,
    model: &str,
    focus: &str,
    args: &EnhanceArgs,
) -> Result<Vec<Finding>> {
    let audit_result = audit::run(audit::AuditOptions {
        root: root.to_path_buf(),
        model: model.to_string(),
        focus: focus.to_string(),
        out: PathBuf::from(AUDIT_REPORT),
        max_chunks: args.audit_max_chunks,
        format: AuditOutputFormat::Markdown,
    })
    .await?;
    let review_result = review::run(review::ReviewOptions {
        root: root.to_path_buf(),
        model: model.to_string(),
        target: args.review_target.clone(),
        focus: focus.to_string(),
        out: PathBuf::from(REVIEW_REPORT),
        max_chunks: args.review_max_chunks,
    })
    .await?;
    collect_findings_from_reports(&audit_result.output_path, &review_result.output_path)
}

fn collect_findings_from_optional_reports(
    audit_path: &Path,
    review_path: &Path,
) -> Result<Vec<Finding>> {
    let audit_report = read_optional_report(audit_path)?;
    let review_report = read_optional_report(review_path)?;
    Ok(collect_findings(&audit_report, &review_report))
}

fn collect_findings_from_reports(audit_path: &Path, review_path: &Path) -> Result<Vec<Finding>> {
    let audit_report = read_report(audit_path)?;
    let review_report = read_report(review_path)?;
    Ok(collect_findings(&audit_report, &review_report))
}

fn read_optional_report(path: &Path) -> Result<String> {
    if path.exists() {
        read_report(path)
    } else {
        Ok(String::new())
    }
}

fn load_state(path: &Path) -> Result<EnhancePlan> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str::<EnhanceState>(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?
        .into_plan()
}

fn save_state(path: &Path, plan: &EnhancePlan) -> Result<()> {
    validate_plan_indices(&plan.findings, &plan.selected, plan.next)?;
    let state = EnhanceState {
        version: 1,
        focus: plan.focus.clone(),
        review_target: plan.review_target.clone(),
        findings: plan.findings.iter().map(StateFinding::from).collect(),
        selected: plan.selected.clone(),
        next: plan.next,
    };
    let bytes = serde_json::to_vec_pretty(&state)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed creating {}", parent.display()))?;
    }
    fs::write(path, bytes).with_context(|| format!("failed writing {}", path.display()))
}

fn cleanup_state(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(err).with_context(|| format!("failed to remove {}", path.display()));
        }
    }
    if let Some(parent) = path.parent() {
        match fs::remove_dir(parent) {
            Ok(()) => {}
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) => {}
            Err(err) => {
                return Err(err).with_context(|| format!("failed to remove {}", parent.display()));
            }
        }
    }
    Ok(())
}

fn validate_plan_indices(findings: &[Finding], selected: &[usize], next: usize) -> Result<()> {
    if next > selected.len() {
        bail!("invalid enhance resume state: next index is past selected findings");
    }
    if let Some(index) = selected
        .iter()
        .copied()
        .find(|index| *index >= findings.len())
    {
        bail!("invalid enhance resume state: finding index {index} is out of range");
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AddressOutcome {
    Committed(String),
    NoChange(String),
}

async fn address_one(
    root: &Path,
    model: &str,
    mode: SafetyMode,
    focus: &str,
    finding: &Finding,
) -> Result<AddressOutcome> {
    ensure_clean_workspace(root)?;
    let mut session = Session::new(
        root.to_path_buf(),
        model.to_string(),
        false,
        remediation_mode(mode),
    );
    let prompt = remediation_prompt(focus, finding);
    let answer = session::run_prompt(&mut session, &prompt).await?;
    if git_status_porcelain(root)?.trim().is_empty() {
        return Ok(AddressOutcome::NoChange(answer));
    }

    let message = commit_message(finding);
    commit_all(root, &message)?;
    let hash = git_output(root, ["rev-parse", "HEAD"])?;
    Ok(AddressOutcome::Committed(hash.trim().to_string()))
}

fn remediation_mode(requested: SafetyMode) -> SafetyMode {
    let _ = requested;
    SafetyMode::AutoAll
}

fn remediation_prompt(focus: &str, finding: &Finding) -> String {
    let mut prompt = String::new();
    prompt.push_str("Address the selected audit/review finding as one focused improvement.\n");
    prompt.push_str("Work in auto mode: inspect first, choose the smallest design/code change that resolves the finding, run the narrowest useful checks, and stop when this finding is resolved.\n");
    prompt.push_str("Keep the patch scoped to this finding. Leave .tmp/oy-enhance reports unchanged. Leave committing to the enhance command after this turn.\n");
    if !focus.trim().is_empty() {
        prompt.push_str("\nUser focus:\n");
        prompt.push_str(focus.trim());
        prompt.push('\n');
    }
    prompt.push_str("\nSelected finding:\n");
    prompt.push_str(&format!("Source: {}\n", finding.source.label()));
    prompt.push_str(&finding.body);
    prompt.push_str("\n\nFinal response: summarize changed files and checks, or explain the remaining blocker if this finding could not be safely addressed.\n");
    prompt
}

fn commit_message(finding: &Finding) -> String {
    let prefix = match finding.source {
        FindingSource::Audit => "Fix audit finding",
        FindingSource::Review => "Fix review finding",
    };
    let title = finding
        .title
        .trim()
        .trim_matches('#')
        .trim_matches(['`', '*', ' '])
        .chars()
        .filter(|ch| !ch.is_control())
        .collect::<String>();
    let title = crate::ui::truncate_chars(&title, 72);
    if title.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}: {title}")
    }
}

fn choose_findings(findings: &[Finding]) -> Result<Vec<usize>> {
    if !config::can_prompt() {
        bail!(
            "enhance needs an interactive terminal to pick findings; rerun with --mode auto for unattended remediation"
        );
    }
    let items = findings.iter().map(Finding::summary).collect::<Vec<_>>();
    Ok(MultiSelect::with_theme(&ColorfulTheme::default())
        .with_prompt("Findings to address")
        .items(&items)
        .interact()?)
}

fn collect_findings(audit_report: &str, review_report: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    findings.extend(parse_findings(FindingSource::Audit, audit_report));
    findings.extend(parse_findings(FindingSource::Review, review_report));
    findings.truncate(MAX_FINDINGS);
    findings
}

fn read_report(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))
}

fn remove_internal_reports(paths: &[&Path]) -> Result<()> {
    for path in paths {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err).with_context(|| format!("failed to remove {}", path.display()));
            }
        }
    }
    Ok(())
}

fn ensure_git_workspace(root: &Path) -> Result<()> {
    git_output(root, ["rev-parse", "--show-toplevel"])
        .map(|_| ())
        .context("enhance requires a git workspace")
}

fn ensure_clean_workspace(root: &Path) -> Result<()> {
    let status = git_status_porcelain(root)?;
    if !status.trim().is_empty() {
        bail!(
            "enhance requires a clean git workspace before each finding; commit or stash current changes first"
        );
    }
    Ok(())
}

fn reset_workspace(root: &Path) -> Result<()> {
    git_success(root, ["reset", "--hard", "HEAD"])?;
    git_success(root, ["clean", "-fd"])
}

fn git_status_porcelain(root: &Path) -> Result<String> {
    git_output(root, ["status", "--porcelain"])
}

fn commit_all(root: &Path, message: &str) -> Result<()> {
    git_success(root, ["add", "-A"])?;
    git_success(root, ["commit", "-m", message])
}

fn git_output<const N: usize>(root: &Path, args: [&str; N]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .context("failed to run git")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git failed: {}", stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn git_success<const N: usize>(root: &Path, args: [&str; N]) -> Result<()> {
    git_output(root, args).map(|_| ())
}

fn short_hash(hash: &str) -> &str {
    hash.get(..hash.len().min(12)).unwrap_or(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_messages_are_short_and_source_specific() {
        let finding = Finding {
            source: FindingSource::Review,
            title: "High: a very long structural problem that should be trimmed before git commit sees it and annoys everyone".into(),
            body: String::new(),
        };
        let message = commit_message(&finding);
        assert!(message.starts_with("Fix review finding: High:"));
        assert!(message.len() <= "Fix review finding: ".len() + 75);
    }

    #[test]
    fn remediation_always_uses_auto_mode() {
        assert_eq!(remediation_mode(SafetyMode::Default), SafetyMode::AutoAll);
        assert_eq!(remediation_mode(SafetyMode::Plan), SafetyMode::AutoAll);
        assert_eq!(remediation_mode(SafetyMode::AutoAll), SafetyMode::AutoAll);
    }

    #[test]
    fn state_round_trips_resume_progress() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join(".tmp/oy-enhance/state.json");
        let plan = EnhancePlan {
            focus: "security".into(),
            review_target: Some("main".into()),
            findings: vec![Finding {
                source: FindingSource::Audit,
                title: "High: path traversal".into(),
                body: "### High: path traversal\n- Evidence: src/files.rs:42".into(),
            }],
            selected: vec![0],
            next: 1,
            resumed: false,
        };

        save_state(&state_path, &plan).unwrap();
        let loaded = load_state(&state_path).unwrap();

        assert!(loaded.resumed);
        assert_eq!(loaded.focus, "security");
        assert_eq!(loaded.review_target.as_deref(), Some("main"));
        assert_eq!(loaded.selected, vec![0]);
        assert_eq!(loaded.next, 1);
        assert_eq!(loaded.findings[0].source, FindingSource::Audit);
    }

    #[test]
    fn optional_reports_support_pre_state_resume() {
        let dir = tempfile::tempdir().unwrap();
        let audit_path = dir.path().join("audit.md");
        let review_path = dir.path().join("review.md");
        fs::write(
            &audit_path,
            "# Audit Issues\n\n## Detailed findings\n\n### High: auth bypass\n\n- Evidence: src/auth.rs:7\n",
        )
        .unwrap();

        let findings = collect_findings_from_optional_reports(&audit_path, &review_path).unwrap();

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].source, FindingSource::Audit);
        assert_eq!(findings[0].title, "High: auth bypass");
    }

    #[test]
    fn state_rejects_invalid_selected_indices() {
        let state = EnhanceState {
            version: 1,
            focus: String::new(),
            review_target: None,
            findings: Vec::new(),
            selected: vec![0],
            next: 0,
        };

        let err = state.into_plan().unwrap_err();
        assert!(err.to_string().contains("out of range"));
    }
}
