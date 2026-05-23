//! Strict no-tools code-quality review pipeline for `oy review`.

use anyhow::{Context, Result, bail};
use chrono::Utc;
use futures_util::{StreamExt as _, stream};
use std::fmt::Write as _;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::audit::{reduce::compact_to_tokens, report};
use crate::{config, session};

pub const DEFAULT_MAX_REVIEW_CHUNKS: usize = 80;
const DEFAULT_PARALLELISM: usize = 8;
const DEFAULT_INPUT_LIMIT: usize = 128_000;
const REPORT_TITLE: &str = "# Code Quality Review";
const TRANSPARENCY_PREFIX: &str = "Generated with [oy-cli](https://github.com/wagov-dtt/oy-cli):";

#[derive(Debug, Clone)]
pub struct ReviewOptions {
    pub root: PathBuf,
    pub model: String,
    pub target: Option<String>,
    pub focus: String,
    pub out: PathBuf,
    pub max_chunks: usize,
}

#[derive(Debug, Clone)]
pub struct ReviewResult {
    pub output_path: PathBuf,
    pub item_count: usize,
    pub chunk_count: usize,
    pub source: String,
}

#[derive(Debug, Clone)]
struct ReviewInput {
    source: String,
    manifest: String,
    chunks: Vec<ReviewChunk>,
    item_count: usize,
}

#[derive(Debug, Clone)]
struct ReviewChunk {
    text: String,
    tokens: usize,
    item_count: usize,
}

#[derive(Debug, Clone)]
struct DiffItem {
    path: String,
    text: String,
    tokens: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NumstatEntry {
    added: Option<usize>,
    deleted: Option<usize>,
    path: String,
}

#[derive(Debug, Clone, Copy)]
struct Sizing {
    target_chunk_tokens: usize,
    small_input_tokens: usize,
    reduce_prompt_max_tokens: usize,
    findings_per_chunk_tokens: usize,
}

pub fn default_output_path() -> PathBuf {
    PathBuf::from("REVIEW.md")
}

pub async fn run(options: ReviewOptions) -> Result<ReviewResult> {
    let model = options.model.trim().to_string();
    let _ = crate::agent::model::cache_model_limits(&model).await;
    let output_path = config::resolve_workspace_output_path(&options.root, &options.out)?;
    let sizing = sizing(&model);
    let input = match options
        .target
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(target) => {
            prepare_diff_input(&options.root, target, &model, sizing.target_chunk_tokens)?
        }
        None => prepare_workspace_input(
            &options.root,
            &output_path,
            &model,
            sizing.target_chunk_tokens,
        )?,
    };
    if input.chunks.len() > options.max_chunks {
        bail!(
            "review would require {} chunks, above the --max-chunks limit of {}; rerun with a narrower target/focus or pass --max-chunks {} to allow this run",
            input.chunks.len(),
            options.max_chunks,
            input.chunks.len()
        );
    }
    if !crate::ui::is_quiet() {
        crate::ui::kv("source", &input.source);
        crate::ui::kv("items", input.item_count);
        crate::ui::kv("chunks", input.chunks.len());
    }

    let system = review_system_prompt();
    let report = if input.chunks.len() == 1 && input.chunks[0].tokens <= sizing.small_input_tokens {
        let prompt = review_full_prompt(
            &options.focus,
            &input.source,
            &input.manifest,
            &input.chunks[0].text,
        );
        session::run_prompt_once_no_tools(&options.model, &system, &prompt).await?
    } else {
        let completed = Arc::new(AtomicUsize::new(0));
        let chunk_count = input.chunks.len();
        let mut findings = stream::iter(input.chunks.iter().enumerate())
            .map(|(idx, chunk)| {
                let prompt = review_chunk_prompt(
                    &options.focus,
                    &input.source,
                    &input.manifest,
                    idx + 1,
                    chunk_count,
                    &chunk.text,
                );
                let system = &system;
                let model = &options.model;
                let completed = Arc::clone(&completed);
                async move {
                    let out = session::run_prompt_once_no_tools(model, system, &prompt).await?;
                    let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
                    if !crate::ui::is_quiet() {
                        crate::ui::err_line(format_args!(
                            "oy review · chunk {done}/{chunk_count} complete"
                        ));
                    }
                    Ok::<_, anyhow::Error>((idx + 1, out))
                }
            })
            .buffer_unordered(DEFAULT_PARALLELISM)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        findings.sort_by_key(|(id, _)| *id);
        let mut candidate = String::new();
        let per_chunk_limit = sizing.findings_per_chunk_tokens.min(
            sizing
                .reduce_prompt_max_tokens
                .saturating_div(findings.len().max(1))
                .max(1),
        );
        for (id, text) in findings {
            let _ = writeln!(candidate, "\n## Candidate findings from chunk {id}\n");
            candidate.push_str(
                compact_to_tokens(&model, text.trim(), per_chunk_limit)
                    .trim()
                    .as_ref(),
            );
            candidate.push('\n');
        }
        let budget = sizing
            .reduce_prompt_max_tokens
            .saturating_sub(8_000)
            .max(2_000);
        let candidate = compact_to_tokens(&model, &candidate, budget);
        let prompt =
            review_reduce_prompt(&options.focus, &input.source, &input.manifest, &candidate);
        session::run_prompt_once_no_tools(&options.model, &system, &prompt).await?
    };

    let report = with_transparency_line(&report, &transparency_snippet(&options));
    config::write_workspace_file(&output_path, report.as_bytes())?;
    Ok(ReviewResult {
        output_path,
        item_count: input.item_count,
        chunk_count: input.chunks.len(),
        source: input.source,
    })
}

fn sizing(model: &str) -> Sizing {
    let input_limit = crate::agent::model::model_limits(model)
        .map(|limits| limits.input.unwrap_or(limits.context))
        .unwrap_or(DEFAULT_INPUT_LIMIT)
        .max(1);
    let reduce_prompt_max_tokens = ((input_limit as f64 * 0.85) as usize).clamp(55_000, 2_000_000);
    let target_chunk_tokens = (input_limit / 2)
        .min(reduce_prompt_max_tokens / 2)
        .clamp(8_000, 500_000);
    Sizing {
        target_chunk_tokens,
        small_input_tokens: ((target_chunk_tokens as f64 * 1.25) as usize).max(target_chunk_tokens),
        reduce_prompt_max_tokens,
        findings_per_chunk_tokens: ((target_chunk_tokens as f64 * 0.10) as usize)
            .clamp(1_000, 50_000),
    }
}

fn prepare_workspace_input(
    root: &Path,
    output_path: &Path,
    model: &str,
    target_tokens: usize,
) -> Result<ReviewInput> {
    let files = crate::audit::input::collect_files(root, Some(output_path), model)?;
    if files.is_empty() {
        bail!("no reviewable text files found for review");
    }
    let mut manifest = crate::audit::input::build_manifest(&files);
    manifest.push('\n');
    manifest.push_str(&workspace_size_index(&files));
    let chunks = crate::audit::input::chunk_files(files, target_tokens);
    crate::audit::input::ensure_chunks_fit_prompt(&chunks, target_tokens)?;
    let item_count = chunks.iter().map(|chunk| chunk.files.len()).sum::<usize>();
    let chunks = chunks
        .into_iter()
        .map(|chunk| ReviewChunk {
            tokens: chunk.tokens,
            item_count: chunk.files.len(),
            text: crate::audit::input::chunk_text(&chunk),
        })
        .collect();
    Ok(ReviewInput {
        source: "whole workspace".into(),
        manifest,
        chunks,
        item_count,
    })
}

fn prepare_diff_input(
    root: &Path,
    target: &str,
    model: &str,
    target_tokens: usize,
) -> Result<ReviewInput> {
    validate_target_ref(target)?;
    let _ = git_output(root, &["rev-parse", "--show-toplevel"])
        .context("review target requires a git workspace")?;
    let diff = git_output(
        root,
        &[
            "diff",
            "--no-ext-diff",
            "--find-renames",
            "--find-copies",
            "--unified=80",
            target,
            "--",
        ],
    )
    .with_context(|| format!("failed to collect git diff against {target}"))?;
    if diff.trim().is_empty() {
        bail!("no git diff found against target {target}");
    }
    let stats = parse_numstat(&git_output(root, &["diff", "--numstat", target, "--"])?);
    let items = split_git_diff_items(&diff, model);
    if items.is_empty() {
        bail!("no reviewable text diff found against target {target}");
    }
    let mut manifest = diff_manifest(target, &items, &stats);
    manifest.push('\n');
    manifest.push_str(&diff_size_index(root, &items, &stats));
    let item_count = items.len();
    let chunks = chunk_diff_items(items, target_tokens);
    ensure_chunks_fit(&chunks, target_tokens)?;
    Ok(ReviewInput {
        source: format!("git diff against {target}"),
        manifest,
        chunks,
        item_count,
    })
}

fn validate_target_ref(target: &str) -> Result<()> {
    if target.trim().is_empty() {
        bail!("review target cannot be empty");
    }
    if target.starts_with('-') {
        bail!("review target must be a branch/commit/ref, not an option-like value");
    }
    if target.contains('\0') || target.contains('\n') || target.contains('\r') {
        bail!("review target contains invalid control characters");
    }
    Ok(())
}

fn git_output(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "git {} failed: {}",
            args.join(" "),
            stderr.trim().lines().next().unwrap_or("unknown git error")
        );
    }
    String::from_utf8(output.stdout).context("git output was not UTF-8")
}

fn split_git_diff_items(diff: &str, model: &str) -> Vec<DiffItem> {
    let mut items = Vec::new();
    let mut current = String::new();
    for line in diff.lines() {
        if line.starts_with("diff --git ") && !current.is_empty() {
            push_diff_item(&mut items, &current, model);
            current.clear();
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.is_empty() {
        push_diff_item(&mut items, &current, model);
    }
    items
}

fn push_diff_item(items: &mut Vec<DiffItem>, text: &str, model: &str) {
    if text
        .lines()
        .any(|line| line.starts_with("Binary files ") || line.starts_with("GIT binary patch"))
    {
        return;
    }
    let path = diff_item_path(text).unwrap_or_else(|| format!("diff-{}", items.len() + 1));
    items.push(DiffItem {
        path,
        text: text.to_string(),
        tokens: crate::compaction::count_tokens(model, text).max(1),
    });
}

fn diff_item_path(text: &str) -> Option<String> {
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("+++ b/") {
            return Some(path.to_string());
        }
        if let Some(path) = line.strip_prefix("rename to ") {
            return Some(path.to_string());
        }
    }
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("diff --git a/")
            && let Some((_, path)) = rest.split_once(" b/")
        {
            return Some(path.to_string());
        }
    }
    None
}

fn chunk_diff_items(items: Vec<DiffItem>, target_tokens: usize) -> Vec<ReviewChunk> {
    let mut chunks = Vec::new();
    let mut current = Vec::new();
    let mut total = 0usize;
    let mut count = 0usize;
    for item in items {
        if !current.is_empty() && total + item.tokens > target_tokens {
            chunks.push(ReviewChunk {
                text: current.join("\n"),
                tokens: total,
                item_count: count,
            });
            current = Vec::new();
            total = 0;
            count = 0;
        }
        current.push(format!("\n## {}\n\n{}", item.path, item.text));
        total += item.tokens;
        count += 1;
    }
    if !current.is_empty() {
        chunks.push(ReviewChunk {
            text: current.join("\n"),
            tokens: total,
            item_count: count,
        });
    }
    chunks
}

fn ensure_chunks_fit(chunks: &[ReviewChunk], target_tokens: usize) -> Result<()> {
    if let Some(chunk) = chunks.iter().find(|chunk| chunk.tokens > target_tokens) {
        bail!(
            "review chunk would exceed the model input budget without truncating review input ({} tokens > {} target tokens, {} item(s)); rerun with a narrower target or a larger-context model",
            chunk.tokens,
            target_tokens,
            chunk.item_count
        );
    }
    Ok(())
}

fn parse_numstat(text: &str) -> Vec<NumstatEntry> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            Some(NumstatEntry {
                added: parts.next()?.parse().ok(),
                deleted: parts.next()?.parse().ok(),
                path: parts.next()?.to_string(),
            })
        })
        .collect()
}

fn diff_manifest(target: &str, items: &[DiffItem], stats: &[NumstatEntry]) -> String {
    let estimated_tokens = items.iter().map(|item| item.tokens).sum::<usize>();
    let added = stats.iter().filter_map(|entry| entry.added).sum::<usize>();
    let deleted = stats
        .iter()
        .filter_map(|entry| entry.deleted)
        .sum::<usize>();
    let mut out = String::new();
    let _ = writeln!(out, "source: git diff against {target}");
    let _ = writeln!(out, "changed_files: {}", items.len());
    let _ = writeln!(out, "estimated_tokens: {estimated_tokens}");
    let _ = writeln!(out, "added_lines: {added}");
    let _ = writeln!(out, "deleted_lines: {deleted}");
    out.push_str("changed files:\n");
    for item in items.iter().take(80) {
        let _ = writeln!(out, "- {} ({} tokens)", item.path, item.tokens);
    }
    out
}

fn workspace_size_index(files: &[crate::audit::input::AuditFile]) -> String {
    let mut entries = files
        .iter()
        .map(|file| (file.path.as_str(), file.text.lines().count()))
        .collect::<Vec<_>>();
    entries.sort_by_key(|(_, lines)| std::cmp::Reverse(*lines));
    let mut out = String::from("Large-file/decomposition index:\n");
    let mut found = false;
    for (path, lines) in entries.iter().filter(|(_, lines)| *lines >= 900).take(80) {
        found = true;
        let status = if *lines >= 1_000 {
            "over 1k lines"
        } else {
            "near 1k lines"
        };
        let _ = writeln!(out, "- {path}: {lines} lines ({status})");
    }
    if !found {
        out.push_str("- no reviewable file is near the 1000-line threshold\nLargest files:\n");
        for (path, lines) in entries.into_iter().take(20) {
            let _ = writeln!(out, "- {path}: {lines} lines");
        }
    }
    out
}

fn diff_size_index(root: &Path, items: &[DiffItem], stats: &[NumstatEntry]) -> String {
    let mut out = String::from("Large-file/decomposition index:\n");
    let mut rows = Vec::new();
    for item in items {
        let Some(current) = current_line_count(root, &item.path) else {
            continue;
        };
        let before = stats
            .iter()
            .find(|entry| entry.path == item.path)
            .and_then(|entry| Some(current.saturating_sub(entry.added?) + entry.deleted?));
        let crosses = before.is_some_and(|before| before < 1_000 && current >= 1_000);
        if current >= 900 || crosses {
            rows.push((item.path.as_str(), current, before, crosses));
        }
    }
    rows.sort_by_key(|(_, current, _, crosses)| (!*crosses, std::cmp::Reverse(*current)));
    if rows.is_empty() {
        out.push_str("- no changed file is known to be near or crossing the 1000-line threshold\n");
        return out;
    }
    for (path, current, before, crosses) in rows.into_iter().take(80) {
        match (before, crosses) {
            (Some(before), true) => {
                let _ = writeln!(
                    out,
                    "- {path}: {before} -> {current} lines (crosses 1k threshold)"
                );
            }
            (Some(before), false) => {
                let _ = writeln!(out, "- {path}: {before} -> {current} lines");
            }
            (None, _) => {
                let _ = writeln!(out, "- {path}: {current} lines");
            }
        }
    }
    out
}

fn current_line_count(root: &Path, rel_path: &str) -> Option<usize> {
    if !safe_relative_path(rel_path) {
        return None;
    }
    fs::read_to_string(root.join(rel_path))
        .ok()
        .map(|text| text.lines().count())
}

fn safe_relative_path(path: &str) -> bool {
    let path = Path::new(path);
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

fn review_system_prompt() -> String {
    r#"You are oy in code-quality review mode. Review the supplied changes or workspace for maintainability, local reasoning, and structural simplicity.
Be terse, evidence-first, and repo-specific. Avoid generic best-practice advice, style nits, and speculation.

Finding quality bar:
- Report only high-conviction structural issues with concrete code evidence and a practical fix.
- Prefer code-judo findings: simpler designs that delete branches, helpers, modes, layers, conditionals, or concepts.
- Flag files crossing or approaching 1000 lines when decomposition would make ownership clearer.
- Flag spaghetti growth: ad-hoc conditionals, scattered special cases, feature checks in unrelated flows, or narrow edge cases inside busy functions.
- Push on type and boundary cleanliness when optionality, casts, loose shapes, wrappers, or silent fallbacks hide invariants.
- Call out architectural drift, duplicated canonical helpers, feature logic leaking across boundaries, and non-atomic related updates.
- Prefer direct, boring code over magical or generic mechanisms that hide simple assumptions.
- Return [] or say there are no major structural concerns when evidence is weak.

Final reports must include a verdict, a succinct findings summary, and detailed writeups for only the most important findings. Spend tokens on repository evidence and concrete simplification, not broad philosophy."#
        .trim()
        .to_string()
}

fn review_full_prompt(focus: &str, source: &str, manifest: &str, input: &str) -> String {
    let mut prompt = String::new();
    let _ = writeln!(
        prompt,
        "Conduct a code-quality review for this input source: {source}."
    );
    push_focus(&mut prompt, focus);
    prompt.push_str("\nReport format:\n1. Start with `# Code Quality Review`.\n2. Add `## Verdict` with `Block`, `Needs work`, or `No major structural concerns`.\n3. Add `## Findings summary` with one concise bullet/table row for each high-conviction finding, including severity and code reference (`path:line` or `path::symbol`).\n4. Add `## Detailed findings` for only the most important findings; each must include severity, evidence, structural impact, and the concrete simplification or decomposition.\n5. Drop weak/speculative items and cosmetic nits. Do not write files.\n\nInput manifest:\n");
    prompt.push_str(manifest.trim());
    prompt.push_str("\n\nReview input:\n");
    prompt.push_str(input.trim());
    prompt
}

fn review_chunk_prompt(
    focus: &str,
    source: &str,
    manifest: &str,
    id: usize,
    count: usize,
    input: &str,
) -> String {
    let mut prompt = String::new();
    let _ = writeln!(
        prompt,
        "Review code-quality chunk {id}/{count} for source: {source}."
    );
    push_focus(&mut prompt, focus);
    prompt.push_str("\nReturn concise candidate findings for this chunk only. Use one `### [Severity] Title` heading per finding, or return `[]` if there are no high-conviction structural findings. For each finding include severity, evidence path/symbol/line when available, structural impact, and a concrete simplification or decomposition. Do not write files.\n\nInput manifest:\n");
    prompt.push_str(manifest.trim());
    prompt.push_str("\n\nChunk input:\n");
    prompt.push_str(input.trim());
    prompt
}

fn review_reduce_prompt(focus: &str, source: &str, manifest: &str, findings: &str) -> String {
    let mut prompt = String::new();
    let _ = writeln!(
        prompt,
        "Condense candidate code-quality findings into the final markdown report for source: {source}."
    );
    push_focus(&mut prompt, focus);
    prompt.push_str("\nReport format:\n1. Start with `# Code Quality Review`.\n2. Add `## Verdict` with `Block`, `Needs work`, or `No major structural concerns`.\n3. Add `## Findings summary` with each surviving finding, severity, and code reference.\n4. Add `## Detailed findings` for only the most important findings; preserve evidence, structural impact, and the concrete simplification or decomposition.\n5. Drop weak/speculative/duplicate items and cosmetic nits.\n\nInput manifest:\n");
    prompt.push_str(manifest.trim());
    prompt.push_str("\n\nCandidate findings:\n");
    prompt.push_str(findings.trim());
    prompt
}

fn push_focus(out: &mut String, focus: &str) {
    let focus = focus.trim();
    if !focus.is_empty() {
        let _ = writeln!(out, "Additional focus: {focus}");
    }
}

fn transparency_snippet(options: &ReviewOptions) -> String {
    let mut command = Vec::new();
    if !options.model.trim().is_empty() {
        command.push(format!(
            "OY_MODEL={}",
            report::shell_quote(options.model.trim())
        ));
    }
    command.push("oy".to_string());
    command.push("review".to_string());
    if options.out != default_output_path() {
        command.push("--out".to_string());
        command.push(report::shell_quote(&options.out.to_string_lossy()));
    }
    if options.max_chunks != DEFAULT_MAX_REVIEW_CHUNKS {
        command.push("--max-chunks".to_string());
        command.push(options.max_chunks.to_string());
    }
    if let Some(target) = options
        .target
        .as_deref()
        .map(str::trim)
        .filter(|target| !target.is_empty())
    {
        command.push(report::shell_quote(target));
    }
    if !options.focus.trim().is_empty() {
        command.push("--focus".to_string());
        command.push(report::shell_quote(options.focus.trim()));
    }
    format!(
        "> {} `{}` · {}",
        TRANSPARENCY_PREFIX,
        command.join(" "),
        Utc::now().format("%Y-%m-%d")
    )
}

fn with_transparency_line(report: &str, snippet: &str) -> String {
    report::with_report_transparency_line(report, snippet, REPORT_TITLE, TRANSPARENCY_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_system_prompt_contains_code_quality_rules() {
        let prompt = review_system_prompt();
        assert!(prompt.contains("code-quality review mode"));
        assert!(prompt.contains("evidence-first"));
        assert!(prompt.contains("code-judo"));
        assert!(prompt.contains("1000 lines"));
        assert!(prompt.contains("spaghetti"));
    }

    #[test]
    fn split_git_diff_items_skips_binary_and_keeps_file_patches() {
        let diff = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-old\n+new\ndiff --git a/logo.png b/logo.png\nBinary files a/logo.png and b/logo.png differ\ndiff --git a/src/b.rs b/src/b.rs\n--- a/src/b.rs\n+++ b/src/b.rs\n@@ -1 +1 @@\n-one\n+two\n";
        let items = split_git_diff_items(diff, "gpt-4o");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].path, "src/a.rs");
        assert_eq!(items[1].path, "src/b.rs");
    }

    #[test]
    fn target_ref_rejects_option_like_values() {
        assert!(validate_target_ref("main").is_ok());
        assert!(validate_target_ref("HEAD~1").is_ok());
        assert!(validate_target_ref("--no-index").is_err());
        assert!(validate_target_ref("bad\nref").is_err());
    }

    #[test]
    fn transparency_line_quotes_target_and_focus() {
        let snippet = transparency_snippet(&ReviewOptions {
            root: PathBuf::from("."),
            model: "my model".to_string(),
            target: Some("feature branch".to_string()),
            focus: "types and boundaries".to_string(),
            out: PathBuf::from("review output.md"),
            max_chunks: 120,
        });
        assert!(snippet.contains("OY_MODEL='my model' oy review --out 'review output.md' --max-chunks 120 'feature branch' --focus 'types and boundaries'"));
    }

    #[test]
    fn with_transparency_line_inserts_title() {
        let out = with_transparency_line(
            "## Verdict\nNeeds work\n",
            "> Generated with [oy-cli](https://github.com/wagov-dtt/oy-cli): `oy review`",
        );
        assert!(out.starts_with("# Code Quality Review\n\n> Generated with [oy-cli]"));
        assert!(out.contains("## Verdict"));
    }

    #[test]
    fn numstat_parser_handles_binary_entries() {
        let stats = parse_numstat("3\t2\tsrc/a.rs\n-\t-\tlogo.png\n");
        assert_eq!(stats[0].added, Some(3));
        assert_eq!(stats[0].deleted, Some(2));
        assert_eq!(stats[1].added, None);
        assert_eq!(stats[1].deleted, None);
    }
}
