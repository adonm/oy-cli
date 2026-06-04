//! Deterministic no-tools audit pipeline: file collection, chunking,
//! map/reduce prompt construction, report/SARIF rendering, and output
//! writing.
//!
//! `oy audit` does not expose tools to the model. It collects reviewable
//! text files, chunks large repositories, runs one or more no-tools model
//! prompts, post-processes the report, and writes the output.

use anyhow::{Context, Result, bail};
use futures_util::{StreamExt as _, stream};
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use crate::{config, session};

pub(crate) mod enhance;
pub(crate) mod findings;
pub(crate) mod input;
mod progress;
mod prompts;
pub(crate) mod reduce;
pub(crate) mod report;
mod sarif;
pub(crate) mod transparency;

use input::{
    build_manifest, build_security_index, chunk_files, chunk_text, collect_files,
    ensure_chunks_fit_prompt,
};
use progress::AuditProgress;
use reduce::{
    ReducePromptLimits, bounded_reduce_findings, compact_to_tokens,
    reduce_candidate_findings_budget,
};
pub(crate) use report::default_output_path;
use report::{
    transparency_snippet, with_structured_findings_block, with_succinct_findings_summary,
    with_transparency_line,
};
use sarif::render_sarif;

const DEFAULT_INPUT_LIMIT: usize = 128_000;
pub const DEFAULT_MAX_REVIEW_CHUNKS: usize = 80;
const MAX_FILE_BYTES: u64 = 512 * 1024;
const DEFAULT_AUDIT_PARALLELISM: usize = 8;

/// All audit sizing constants derived from the current model's token limits.
#[derive(Debug, Clone, Copy)]
struct AuditSizing {
    target_chunk_tokens: usize,
    small_repo_tokens: usize,
    reduce_prompt_max_tokens: usize,
    findings_per_chunk_limit_tokens: usize,
    security_index_limit: usize,
    reduce_findings_min_tokens: usize,
    reduce_findings_token_reserve: usize,
}

fn audit_constants(model_spec: &str) -> AuditSizing {
    let input_limit = crate::agent::model::model_limits(model_spec)
        .map(|l| l.input.unwrap_or(l.context))
        .unwrap_or(DEFAULT_INPUT_LIMIT)
        .max(1);

    // reduce prompt: 85% of input window, clamped sensibly
    let reduce_prompt = (input_limit as f64 * 0.85) as usize;
    let reduce_prompt = reduce_prompt.clamp(55_000, 2_000_000);

    // chunk size: ~half of input window
    let target_chunk = (input_limit / 2).min(reduce_prompt / 2);
    let target_chunk = target_chunk.clamp(8_000, 500_000);

    // small repo: slightly larger than one chunk
    let small_repo = (target_chunk as f64 * 1.25) as usize;

    // per-chunk findings budget: proportional to chunk size
    let findings_per_chunk = (target_chunk as f64 * 0.10) as usize;
    let findings_per_chunk = findings_per_chunk.clamp(1_000, 50_000);

    // security index entries: ~0.2% of input window
    let security_index = (input_limit as f64 * 0.002) as usize;
    let security_index = security_index.clamp(40, 500);

    // reduce floor: ~2.5% of input window
    let reduce_min = (input_limit as f64 * 0.025) as usize;
    let reduce_min = reduce_min.clamp(2_000, 50_000);

    // reduce overhead reserve: ~3.3% of input window
    let reduce_reserve = (input_limit as f64 * 0.033) as usize;
    let reduce_reserve = reduce_reserve.clamp(1_000, 30_000);

    AuditSizing {
        target_chunk_tokens: target_chunk,
        small_repo_tokens: small_repo,
        reduce_prompt_max_tokens: reduce_prompt,
        findings_per_chunk_limit_tokens: findings_per_chunk,
        security_index_limit: security_index,
        reduce_findings_min_tokens: reduce_min,
        reduce_findings_token_reserve: reduce_reserve,
    }
}

#[derive(Debug, Clone)]
pub struct AuditOptions {
    pub root: PathBuf,
    pub model: String,
    pub focus: String,
    pub out: PathBuf,
    pub max_chunks: usize,
    pub format: AuditOutputFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOutputFormat {
    Markdown,
    Sarif,
}

impl AuditOutputFormat {
    pub fn name(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Sarif => "sarif",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuditResult {
    pub output_path: PathBuf,
    pub file_count: usize,
    pub chunk_count: usize,
}

pub async fn run(options: AuditOptions) -> Result<AuditResult> {
    let started = Instant::now();
    let _title = crate::ui::title_scope(format_args!(
        "oy audit · {}",
        crate::ui::compact_preview(&options.focus, 48)
    ));
    let plan = prepare(options, started).await?;
    let raw_report = execute(&plan).await?;
    finalize(plan, raw_report)
}

/// All inputs and intermediate state collected before any model prompt
/// runs. `prepare` builds it once; `execute` consumes it; `finalize` uses
/// the original options to render and write the report.
struct AuditPlan {
    options: AuditOptions,
    model_spec: String,
    system_prompt: String,
    manifest: String,
    index: String,
    chunks: Vec<input::AuditChunk>,
    sizing: AuditSizing,
    existing_issues: Option<String>,
    output_path: PathBuf,
    file_count: usize,
    chunk_count: usize,
    progress: AuditProgress,
}

/// Collect files, build the manifest, security index, and chunk plan,
/// load any prior `ISSUES.md`, and validate against `--max-chunks`.
/// No model calls happen here.
async fn prepare(options: AuditOptions, started: Instant) -> Result<AuditPlan> {
    let model_spec = options.model.trim().to_string();
    let _ = crate::agent::model::cache_model_limits(&model_spec).await;
    let output_path = config::resolve_workspace_output_path(&options.root, &options.out)?;
    let files = collect_files(&options.root, Some(&output_path), &model_spec)?;
    if files.is_empty() {
        bail!("no reviewable text files found for audit");
    }
    let manifest = build_manifest(&files);
    let sizing = audit_constants(&model_spec);
    let index = build_security_index(&files, sizing.security_index_limit);
    let chunks = chunk_files(files, sizing.target_chunk_tokens);
    ensure_chunks_fit_prompt(&chunks, sizing.target_chunk_tokens)?;
    if chunks.len() > options.max_chunks {
        bail!(
            "audit would require {} chunks, above the --max-chunks limit of {}; rerun with a focused path/filter or pass --max-chunks {} to allow this run",
            chunks.len(),
            options.max_chunks,
            chunks.len()
        );
    }
    let file_count = chunks.iter().map(|chunk| chunk.files.len()).sum::<usize>();
    let chunk_count = chunks.len();
    let progress = AuditProgress::new(started, file_count, chunk_count);
    progress.prepared();

    let existing_issues_path = options.root.join("ISSUES.md");
    let existing_issues = if existing_issues_path.exists() {
        fs::read_to_string(&existing_issues_path)
            .with_context(|| {
                format!(
                    "failed to read existing audit file: {}",
                    existing_issues_path.display()
                )
            })
            .ok()
            .filter(|content| !content.trim().is_empty())
    } else {
        None
    };

    Ok(AuditPlan {
        system_prompt: prompts::audit_system_prompt(),
        options,
        model_spec,
        manifest,
        index,
        chunks,
        sizing,
        existing_issues,
        output_path,
        file_count,
        chunk_count,
        progress,
    })
}

/// Run the model prompts that produce the raw audit report. The
/// single-chunk fast path stays inline; the multi-chunk path streams
/// chunk prompts with bounded parallelism, then runs the reduce prompt
/// to merge per-chunk findings into a single report.
async fn execute(plan: &AuditPlan) -> Result<String> {
    if plan.chunks.len() == 1 && plan.chunks[0].tokens <= plan.sizing.small_repo_tokens {
        let repo_text = chunk_text(&plan.chunks[0]);
        let prompt = prompts::audit_full_prompt(
            &plan.options.focus,
            &plan.manifest,
            &plan.index,
            &repo_text,
            plan.existing_issues.as_deref(),
        );
        plan.progress.review_started(None);
        let report =
            session::run_prompt_once_no_tools(&plan.options.model, &plan.system_prompt, &prompt)
                .await?;
        plan.progress.review_finished(1);
        return Ok(report);
    }

    plan.progress
        .review_started(Some(DEFAULT_AUDIT_PARALLELISM));
    let completed_chunks = Arc::new(AtomicUsize::new(0));
    let mut chunk_findings = stream::iter(plan.chunks.iter().enumerate())
        .map(|(idx, chunk)| {
            let chunk_id = idx + 1;
            let prompt = prompts::audit_chunk_prompt(
                &plan.options.focus,
                &plan.manifest,
                &plan.index,
                chunk_id,
                plan.chunk_count,
                &chunk_text(chunk),
            );
            let model = &plan.options.model;
            let system_prompt = &plan.system_prompt;
            let progress = &plan.progress;
            let completed_chunks = Arc::clone(&completed_chunks);
            async move {
                let findings =
                    session::run_prompt_once_no_tools(model, system_prompt, &prompt).await?;
                let completed = completed_chunks.fetch_add(1, Ordering::Relaxed) + 1;
                progress.review_finished(completed);
                Ok::<_, anyhow::Error>((chunk_id, findings))
            }
        })
        .buffer_unordered(DEFAULT_AUDIT_PARALLELISM)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    chunk_findings.sort_by_key(|(chunk_id, _)| *chunk_id);

    let reduce_findings_budget = reduce_candidate_findings_budget(
        &plan.model_spec,
        &plan.options.focus,
        &plan.manifest,
        plan.existing_issues.as_deref(),
        plan.sizing.reduce_prompt_max_tokens,
        plan.sizing.reduce_findings_min_tokens,
        plan.sizing.reduce_findings_token_reserve,
    );
    let per_chunk_findings_limit = plan.sizing.findings_per_chunk_limit_tokens.min(
        reduce_findings_budget
            .saturating_div(chunk_findings.len().max(1))
            .max(1),
    );
    let mut candidate_findings = String::new();
    for (chunk_id, findings) in chunk_findings {
        let compact =
            compact_to_tokens(&plan.model_spec, findings.trim(), per_chunk_findings_limit);
        let _ = writeln!(
            candidate_findings,
            "\n## Candidate findings from chunk {chunk_id}\n"
        );
        candidate_findings.push_str(compact.trim());
        candidate_findings.push('\n');
    }
    let candidate_findings = bounded_reduce_findings(
        &plan.model_spec,
        &plan.options.focus,
        &plan.manifest,
        &candidate_findings,
        plan.existing_issues.as_deref(),
        ReducePromptLimits {
            max_prompt_tokens: plan.sizing.reduce_prompt_max_tokens,
            min_tokens: plan.sizing.reduce_findings_min_tokens,
            reserve_tokens: plan.sizing.reduce_findings_token_reserve,
        },
    );
    let prompt = prompts::audit_reduce_prompt(
        &plan.options.focus,
        &plan.manifest,
        &candidate_findings,
        plan.existing_issues.as_deref(),
    );
    plan.progress.summarise_started();
    let report =
        session::run_prompt_once_no_tools(&plan.options.model, &plan.system_prompt, &prompt)
            .await?;
    plan.progress.summarise_finished();
    Ok(report)
}

/// Apply the post-processing pass (transparency line, structured-findings
/// block, succinct summary, optional SARIF rendering) and write the
/// final report to the workspace output path.
fn finalize(plan: AuditPlan, raw_report: String) -> Result<AuditResult> {
    let report = with_transparency_line(&raw_report, &transparency_snippet(&plan.options));
    let report = with_structured_findings_block(&report, "audit");
    let report = with_succinct_findings_summary(&report);
    let output = match plan.options.format {
        AuditOutputFormat::Markdown => report,
        AuditOutputFormat::Sarif => render_sarif(&report)?,
    };
    plan.progress.write_started(&plan.output_path);
    config::write_workspace_file(&plan.output_path, output.as_bytes())?;
    plan.progress.write_finished(&plan.output_path);
    Ok(AuditResult {
        output_path: plan.output_path,
        file_count: plan.file_count,
        chunk_count: plan.chunk_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::input::{AuditFile, chunk_files, should_skip_path};
    use crate::audit::reduce::{ReducePromptLimits, bounded_reduce_findings, compact_to_tokens};
    use crate::compaction;

    #[test]
    fn transparency_line_is_inserted_after_title() {
        let out = with_transparency_line(
            "# Audit Issues\n\n## H1\n",
            "> Generated with [oy-cli](https://github.com/wagov-dtt/oy-cli): `oy audit`",
        );
        assert!(out.starts_with("# Audit Issues\n\n> Generated with [oy-cli]"));
        assert!(out.contains("## H1"));
    }

    #[test]
    fn succinct_summary_is_inserted_from_detailed_findings() {
        let out = with_succinct_findings_summary(
            "# Audit Issues\n\n> Generated with [oy-cli](https://github.com/wagov-dtt/oy-cli): `oy audit`\n\n## Detailed findings\n\n### High: path traversal reaches file writes\n\n- Evidence: `src/files.rs:42` passes user input into write.\n- Fix: canonicalize under the workspace.\n\n### Low: noisy retry loop\n\n- Severity: Low\n- Evidence: `src/retry.rs::spin` retries without backoff.\n",
        );
        assert!(out.contains("## Findings summary"));
        assert!(out.contains("- **High** `src/files.rs:42` — path traversal reaches file writes"));
        assert!(out.contains("- **Low** `src/retry.rs::spin` — noisy retry loop"));
        assert!(out.find("## Findings summary") < out.find("## Detailed findings"));
    }

    #[test]
    fn existing_findings_summary_is_preserved() {
        let report =
            "# Audit Issues\n\n## Findings summary\n\n- **High** `src/lib.rs:1` — existing\n";
        assert_eq!(with_succinct_findings_summary(report), report);
    }

    #[test]
    fn transparency_line_includes_non_default_max_chunks() {
        let snippet = transparency_snippet(&AuditOptions {
            root: PathBuf::from("."),
            model: String::new(),
            focus: "auth paths".to_string(),
            out: PathBuf::from("ISSUES.md"),
            max_chunks: 240,
            format: AuditOutputFormat::Markdown,
        });
        assert!(snippet.contains("oy audit --max-chunks 240 'auth paths'"));
    }

    #[test]
    fn transparency_line_quotes_shell_words() {
        let snippet = transparency_snippet(&AuditOptions {
            root: PathBuf::from("."),
            model: "my model".to_string(),
            focus: "auth paths".to_string(),
            out: PathBuf::from("audit output.md"),
            max_chunks: DEFAULT_MAX_REVIEW_CHUNKS,
            format: AuditOutputFormat::Markdown,
        });
        assert!(
            snippet.contains("OY_MODEL='my model' oy audit --out 'audit output.md' 'auth paths'")
        );
    }

    #[test]
    fn sarif_renderer_maps_findings_to_results() {
        let sarif = render_sarif(
            "# Audit Issues\n\n## Detailed findings\n\n### High: path traversal reaches writes\n\n- Evidence: `src/files.rs:42` writes attacker paths.\n- Fix: canonicalize.\n",
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&sarif).unwrap();
        assert_eq!(value["version"], "2.1.0");
        assert_eq!(
            value["runs"][0]["results"][0]["ruleId"],
            "oy/high/path-traversal-reaches-writes"
        );
        assert_eq!(
            value["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["artifactLocation"]
                ["uri"],
            "src/files.rs"
        );
        assert_eq!(
            value["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["region"]["startLine"],
            42
        );
    }

    #[test]
    fn sarif_renderer_prefers_structured_findings_block() {
        let sarif = render_sarif(
            r#"# Audit Issues

## Detailed findings

### Low: stale markdown
Evidence: src/old.rs:1

## Machine-readable findings

```json oy-findings
[
  {
    "source": "audit",
    "severity": "High",
    "title": "typed path wins",
    "locations": [{ "path": "src/new.rs", "line": 9 }],
    "evidence": "src/new.rs:9 proves it",
    "body": "Fix it.",
    "category": "security"
  }
]
```
"#,
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&sarif).unwrap();
        let results = value["runs"][0]["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["ruleId"], "oy/high/typed-path-wins");
        assert_eq!(
            results[0]["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
            "src/new.rs"
        );
    }

    #[test]
    fn sarif_renderer_omits_unsafe_locations_without_dropping_results() {
        let sarif = render_sarif(
            "# Audit Issues\n\n## Detailed findings\n\n### High: good path\n\n- Evidence: `./src/files.rs:42` writes attacker paths.\n\n### Medium: bad path\n\n- Evidence: `../secret.rs:1` is bad.\n",
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&sarif).unwrap();
        let results = value["runs"][0]["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0]["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
            "src/files.rs"
        );
        assert!(results[1].get("locations").is_none());
    }

    #[test]
    fn chunking_keeps_files_under_target_when_possible() {
        let files = vec![
            AuditFile {
                path: "a.rs".into(),
                language: "Rust",
                bytes: 1,
                tokens: 5,
                text: "a".into(),
            },
            AuditFile {
                path: "b.rs".into(),
                language: "Rust",
                bytes: 1,
                tokens: 7,
                text: "b".into(),
            },
            AuditFile {
                path: "c.rs".into(),
                language: "Rust",
                bytes: 1,
                tokens: 4,
                text: "c".into(),
            },
        ];
        let chunks = chunk_files(files, 12);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].tokens, 12);
        assert_eq!(chunks[1].tokens, 4);
    }

    #[test]
    fn audit_chunk_text_preserves_full_file_input() {
        let text = format!("{}\nIMPORTANT_TAIL_SENTINEL\n", "line\n".repeat(2_000));
        let files = vec![AuditFile {
            path: "src/large.rs".into(),
            language: "Rust",
            bytes: text.len() as u64,
            tokens: 20_000,
            text: text.clone(),
        }];

        let chunks = chunk_files(files, 8_000);
        let rendered = chunk_text(&chunks[0]);

        assert!(rendered.contains("## src/large.rs"));
        assert!(rendered.contains("IMPORTANT_TAIL_SENTINEL"));
        assert_eq!(rendered.matches("line").count(), 2_000);
    }

    #[test]
    fn audit_oversize_single_file_fails_instead_of_truncating() {
        let chunks = vec![crate::audit::input::AuditChunk {
            tokens: 20_000,
            files: vec![AuditFile {
                path: "src/huge.rs".into(),
                language: "Rust",
                bytes: 1,
                tokens: 20_000,
                text: "huge".into(),
            }],
        }];

        let err = crate::audit::input::ensure_chunks_fit_prompt(&chunks, 8_000).unwrap_err();

        assert!(err.to_string().contains("without truncating review input"));
        assert!(err.to_string().contains("src/huge.rs"));
    }

    #[test]
    fn skips_lockfiles_build_dirs_likely_secrets_and_generated_reports() {
        assert!(should_skip_path("target/debug/app"));
        assert!(should_skip_path("Cargo.lock"));
        assert!(should_skip_path(".env"));
        assert!(should_skip_path(".env.production"));
        assert!(should_skip_path("config/.npmrc"));
        assert!(should_skip_path("config/credentials.json"));
        assert!(should_skip_path("k8s/secrets.yaml"));
        assert!(should_skip_path("keys/id_ed25519"));
        assert!(should_skip_path("certs/prod.pem"));
        assert!(should_skip_path("ISSUES.md"));
        assert!(should_skip_path("issues.md"));
        assert!(should_skip_path("REVIEW.md"));
        assert!(should_skip_path("docs/review.md"));
        assert!(should_skip_path("oy.sarif"));
        assert!(should_skip_path(".tmp/oy-enhance/review.md"));
        assert!(!should_skip_path("src/main.rs"));
        assert!(!should_skip_path("src/token.rs"));
        assert!(!should_skip_path("src/secret_manager.go"));
        assert!(!should_skip_path("src/credential_store.ts"));
    }

    #[test]
    fn compact_to_tokens_enforces_token_limit() {
        let text = "candidate finding with evidence src/lib.rs:1 and remediation\n".repeat(10_000);
        let compact = compact_to_tokens("gpt-4o", &text, 1_000);
        assert!(compaction::count_tokens("gpt-4o", &compact) <= 1_000);
        assert!(compact.contains("truncated"));
    }

    #[test]
    fn reduce_findings_compaction_preserves_middle_finding_headings() {
        let manifest = "files: 1\nestimated_tokens: 1\nbytes: 1\nlanguages: Rust";
        let mut findings = String::new();
        for idx in 1..=30 {
            let _ = writeln!(findings, "### Medium: issue {idx}");
            let _ = writeln!(findings, "- Evidence: `src/file{idx}.rs:1` reaches sink.");
            findings.push_str(&"- Detail: repeated context.\n".repeat(100));
        }

        let bounded = bounded_reduce_findings(
            "gpt-4o",
            "",
            manifest,
            &findings,
            None,
            ReducePromptLimits {
                max_prompt_tokens: 2_000,
                min_tokens: 8_000,
                reserve_tokens: 4_000,
            },
        );
        assert!(bounded.contains("### Medium: issue 1"));
        assert!(bounded.contains("### Medium: issue 15"));
        assert!(bounded.contains("### Medium: issue 30"));
        assert!(!bounded.contains("truncated"));
    }

    #[test]
    fn reduce_findings_prompt_is_bounded_for_many_chunks() {
        let manifest = "files: 240\nestimated_tokens: 12000000\nbytes: 48000000\nlanguages: Rust";
        let finding = "### High: issue\n- Evidence: `src/lib.rs:1` attacker input reaches sink.\n- Impact: data exposure.\n- Fix: validate at boundary.\n";
        let mut findings = String::new();
        for chunk_id in 1..=240 {
            let _ = writeln!(findings, "\n## Candidate findings from chunk {chunk_id}\n");
            findings.push_str(&finding.repeat(200));
        }

        let bounded = bounded_reduce_findings(
            "gpt-4o",
            "",
            manifest,
            &findings,
            None,
            ReducePromptLimits {
                max_prompt_tokens: 20_000,
                min_tokens: 8_000,
                reserve_tokens: 4_000,
            },
        );
        let prompt = prompts::audit_reduce_prompt("", manifest, &bounded, None);
        assert!(compaction::count_tokens("gpt-4o", &prompt) <= 20_000);
        assert!(bounded.contains("truncated"));
    }
}
