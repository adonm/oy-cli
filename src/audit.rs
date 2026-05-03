use anyhow::{Result, bail};
use futures_util::{StreamExt as _, stream};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use crate::{config, model, session};

mod input;
mod progress;
mod prompts;
mod reduce;
mod report;
mod sarif;

use input::{build_manifest, build_security_index, chunk_files, chunk_text, collect_files};
use progress::AuditProgress;
use reduce::{bounded_reduce_findings, compact_to_tokens, reduce_candidate_findings_budget};
pub(crate) use report::default_output_path;
use report::{transparency_snippet, with_succinct_findings_summary, with_transparency_line};
use sarif::render_sarif;

const TARGET_CHUNK_TOKENS: usize = 64_000;
const SMALL_REPO_TOKENS: usize = 80_000;
pub const DEFAULT_MAX_REVIEW_CHUNKS: usize = 80;
const MAX_FILE_BYTES: u64 = 512 * 1024;
const SECURITY_INDEX_LIMIT: usize = 160;
const FINDINGS_PER_CHUNK_LIMIT_TOKENS: usize = 6_000;
const REDUCE_PROMPT_MAX_TOKENS: usize = 220_000;
const REDUCE_FINDINGS_TOKEN_RESERVE: usize = 4_000;
const REDUCE_FINDINGS_MIN_TOKENS: usize = 8_000;
const DEFAULT_AUDIT_PARALLELISM: usize = 8;

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
    let model_spec = model::to_genai_model_spec(&options.model);
    let output_path = config::resolve_workspace_output_path(&options.root, &options.out)?;
    let files = collect_files(&options.root, Some(&output_path), &model_spec)?;
    if files.is_empty() {
        bail!("no reviewable text files found for audit");
    }
    let manifest = build_manifest(&files);
    let index = build_security_index(&files);
    let chunks = chunk_files(files, TARGET_CHUNK_TOKENS);
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

    let system_prompt = prompts::audit_system_prompt();
    let report = if chunks.len() == 1 && chunks[0].tokens <= SMALL_REPO_TOKENS {
        let repo_text = chunk_text(&chunks[0]);
        let prompt = prompts::audit_full_prompt(&options.focus, &manifest, &index, &repo_text);
        progress.review_started(None);
        let report =
            session::run_prompt_once_no_tools(&options.model, &system_prompt, &prompt).await?;
        progress.review_finished(1);
        report
    } else {
        progress.review_started(Some(DEFAULT_AUDIT_PARALLELISM));
        let completed_chunks = Arc::new(AtomicUsize::new(0));
        let mut chunk_findings = stream::iter(chunks.iter().enumerate())
            .map(|(idx, chunk)| {
                let chunk_id = idx + 1;
                let prompt = prompts::audit_chunk_prompt(
                    &options.focus,
                    &manifest,
                    &index,
                    chunk_id,
                    chunk_count,
                    &chunk_text(chunk),
                );
                let model = &options.model;
                let system_prompt = &system_prompt;
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
            &model_spec,
            &options.focus,
            &manifest,
            REDUCE_PROMPT_MAX_TOKENS,
        );
        let per_chunk_findings_limit = FINDINGS_PER_CHUNK_LIMIT_TOKENS.min(
            reduce_findings_budget
                .saturating_div(chunk_findings.len().max(1))
                .max(1),
        );
        let mut candidate_findings = String::new();
        for (chunk_id, findings) in chunk_findings {
            let compact = compact_to_tokens(&model_spec, findings.trim(), per_chunk_findings_limit);
            let _ = writeln!(
                candidate_findings,
                "\n## Candidate findings from chunk {chunk_id}\n"
            );
            candidate_findings.push_str(compact.trim());
            candidate_findings.push('\n');
        }
        let candidate_findings = bounded_reduce_findings(
            &model_spec,
            &options.focus,
            &manifest,
            &candidate_findings,
            REDUCE_PROMPT_MAX_TOKENS,
        );
        let prompt = prompts::audit_reduce_prompt(&options.focus, &manifest, &candidate_findings);
        progress.summarise_started();
        let report =
            session::run_prompt_once_no_tools(&options.model, &system_prompt, &prompt).await?;
        progress.summarise_finished();
        report
    };

    let report = with_transparency_line(&report, &transparency_snippet(&options));
    let report = with_succinct_findings_summary(&report);
    let output = match options.format {
        AuditOutputFormat::Markdown => report,
        AuditOutputFormat::Sarif => render_sarif(&report)?,
    };
    progress.write_started(&output_path);
    config::write_workspace_file(&output_path, output.as_bytes())?;
    progress.write_finished(&output_path);
    Ok(AuditResult {
        output_path,
        file_count,
        chunk_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::input::{AuditFile, chunk_files, should_skip_path};
    use crate::audit::reduce::{bounded_reduce_findings, compact_to_tokens};
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
    fn sarif_renderer_rejects_escaping_paths() {
        let err = render_sarif(
            "# Audit Issues\n\n## Detailed findings\n\n### High: bad path\n\n- Evidence: `../secret.rs:1` is bad.\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("escapes workspace"));
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
    fn skips_lockfiles_and_build_dirs() {
        assert!(should_skip_path("target/debug/app"));
        assert!(should_skip_path("Cargo.lock"));
        assert!(!should_skip_path("src/main.rs"));
    }

    #[test]
    fn compact_to_tokens_enforces_token_limit() {
        let text = "candidate finding with evidence src/lib.rs:1 and remediation\n".repeat(10_000);
        let compact = compact_to_tokens("gpt-4o", &text, 1_000);
        assert!(compaction::count_tokens("gpt-4o", &compact) <= 1_000);
        assert!(compact.contains("truncated"));
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

        let bounded = bounded_reduce_findings("gpt-4o", "", manifest, &findings, 20_000);
        let prompt = prompts::audit_reduce_prompt("", manifest, &bounded);
        assert!(compaction::count_tokens("gpt-4o", &prompt) <= 20_000);
        assert!(bounded.contains("truncated"));
    }
}
