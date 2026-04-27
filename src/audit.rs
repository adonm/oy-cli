use anyhow::{Context, Result, bail};
use chrono::Utc;
use ignore::WalkBuilder;
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use crate::{config, model, prompts, session};

const TARGET_CHUNK_TOKENS: usize = 64_000;
const SMALL_REPO_TOKENS: usize = 80_000;
const MAX_REVIEW_CHUNKS: usize = 80;
const MAX_FILE_BYTES: u64 = 512 * 1024;
const SECURITY_INDEX_LIMIT: usize = 160;
const FINDINGS_PER_CHUNK_LIMIT_TOKENS: usize = 6_000;

#[derive(Debug, Clone)]
pub struct AuditOptions {
    pub root: PathBuf,
    pub model: String,
    pub focus: String,
    pub out: PathBuf,
}

#[derive(Debug, Clone)]
pub struct AuditResult {
    pub output_path: PathBuf,
    pub file_count: usize,
    pub chunk_count: usize,
}

#[derive(Debug, Clone)]
struct AuditFile {
    path: String,
    language: &'static str,
    bytes: u64,
    tokens: usize,
    text: String,
}

#[derive(Debug, Clone)]
struct AuditChunk {
    files: Vec<AuditFile>,
    tokens: usize,
}

pub async fn run(options: AuditOptions) -> Result<AuditResult> {
    let model_spec = model::to_genai_model_spec(&options.model);
    let output_path = config::resolve_workspace_output_path(&options.root, &options.out)?;
    let files = collect_files(&options.root, Some(&output_path), &model_spec)?;
    if files.is_empty() {
        bail!("no reviewable text files found for audit");
    }
    let manifest = build_manifest(&files);
    let index = build_security_index(&files);
    let chunks = chunk_files(files, TARGET_CHUNK_TOKENS);
    if chunks.len() > MAX_REVIEW_CHUNKS {
        bail!(
            "audit would require {} chunks; narrow the repo or raise chunking limits before running",
            chunks.len()
        );
    }

    let system_prompt = prompts::audit_system_prompt();
    let report = if chunks.len() == 1 && chunks[0].tokens <= SMALL_REPO_TOKENS {
        let repo_text = chunk_text(&chunks[0]);
        let prompt = prompts::audit_full_prompt(&options.focus, &manifest, &index, &repo_text);
        session::run_prompt_once_no_tools(&options.model, &system_prompt, &prompt).await?
    } else {
        let mut candidate_findings = String::new();
        for (idx, chunk) in chunks.iter().enumerate() {
            let chunk_id = idx + 1;
            let prompt = prompts::audit_chunk_prompt(
                &options.focus,
                &manifest,
                &index,
                chunk_id,
                chunks.len(),
                &chunk_text(chunk),
            );
            let findings =
                session::run_prompt_once_no_tools(&options.model, &system_prompt, &prompt).await?;
            let compact = compact_to_tokens(
                &model_spec,
                findings.trim(),
                FINDINGS_PER_CHUNK_LIMIT_TOKENS,
            );
            let _ = writeln!(
                candidate_findings,
                "\n## Candidate findings from chunk {chunk_id}\n"
            );
            candidate_findings.push_str(compact.trim());
            candidate_findings.push('\n');
        }
        let prompt = prompts::audit_reduce_prompt(&options.focus, &manifest, &candidate_findings);
        session::run_prompt_once_no_tools(&options.model, &system_prompt, &prompt).await?
    };

    let report = with_transparency_line(&report, &transparency_snippet(&options));
    config::write_workspace_file(&output_path, report.as_bytes())?;
    Ok(AuditResult {
        output_path,
        file_count: chunks.iter().map(|chunk| chunk.files.len()).sum(),
        chunk_count: chunks.len(),
    })
}

fn collect_files(
    root: &Path,
    output_path: Option<&Path>,
    model_spec: &str,
) -> Result<Vec<AuditFile>> {
    let mut files = Vec::new();
    let output_path = output_path.and_then(|path| path.canonicalize().ok());
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .follow_links(false);
    for entry in builder.build() {
        let entry = entry.map_err(|err| anyhow::anyhow!(err))?;
        let path = entry.path();
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let rel = rel_path(root, path)?;
        if should_skip_path(&rel) {
            continue;
        }
        if output_path.as_ref().is_some_and(|out| path == out) {
            continue;
        }
        let meta = match fs::metadata(path) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        if meta.len() > MAX_FILE_BYTES {
            continue;
        }
        let raw = match fs::read(path) {
            Ok(raw) => raw,
            Err(_) => continue,
        };
        if raw.contains(&0) {
            continue;
        }
        let text = String::from_utf8_lossy(&raw).to_string();
        if text.trim().is_empty() {
            continue;
        }
        let tokens = session::count_tokens(model_spec, &text).max(1);
        files.push(AuditFile {
            language: language_for_path(&rel),
            path: rel,
            bytes: meta.len(),
            tokens,
            text,
        });
    }
    files.sort_by(|a, b| audit_priority(a).cmp(&audit_priority(b)));
    Ok(files)
}

fn should_skip_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if lower.starts_with(".git/")
        || lower.starts_with("target/")
        || lower.starts_with("node_modules/")
        || lower.starts_with(".venv/")
        || lower.starts_with(".tmp/")
    {
        return true;
    }
    let name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "cargo.lock" | "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock" | "uv.lock" | "go.sum"
    )
}

fn audit_priority(file: &AuditFile) -> (u8, std::cmp::Reverse<usize>, String) {
    let path = file.path.to_ascii_lowercase();
    let score = if security_path_score(&path) { 0 } else { 1 };
    (score, std::cmp::Reverse(file.tokens), path)
}

fn security_path_score(path: &str) -> bool {
    [
        "auth",
        "session",
        "token",
        "secret",
        "crypto",
        "password",
        "policy",
        "permission",
        "admin",
        "login",
        "security",
        "config",
        "route",
        "api",
        "http",
        "request",
        "shell",
        "command",
        "process",
        "file",
        "path",
        "upload",
        "download",
        "network",
    ]
    .iter()
    .any(|needle| path.contains(needle))
}

fn chunk_files(files: Vec<AuditFile>, target_tokens: usize) -> Vec<AuditChunk> {
    let mut chunks = Vec::new();
    let mut current = Vec::new();
    let mut total = 0usize;
    for file in files {
        if !current.is_empty() && total + file.tokens > target_tokens {
            chunks.push(AuditChunk {
                files: current,
                tokens: total,
            });
            current = Vec::new();
            total = 0;
        }
        total += file.tokens;
        current.push(file);
    }
    if !current.is_empty() {
        chunks.push(AuditChunk {
            files: current,
            tokens: total,
        });
    }
    chunks
}

fn build_manifest(files: &[AuditFile]) -> String {
    let mut languages = BTreeSet::new();
    let total_tokens = files.iter().map(|file| file.tokens).sum::<usize>();
    let total_bytes = files.iter().map(|file| file.bytes).sum::<u64>();
    for file in files {
        languages.insert(file.language);
    }
    let mut out = String::new();
    let _ = writeln!(out, "files: {}", files.len());
    let _ = writeln!(out, "estimated_tokens: {total_tokens}");
    let _ = writeln!(out, "bytes: {total_bytes}");
    let _ = writeln!(
        out,
        "languages: {}",
        languages.into_iter().collect::<Vec<_>>().join(", ")
    );
    out.push_str("largest/security-prioritized files:\n");
    for file in files.iter().take(40) {
        let _ = writeln!(
            out,
            "- {} ({}; {} tokens; {} bytes)",
            file.path, file.language, file.tokens, file.bytes
        );
    }
    out
}

fn build_security_index(files: &[AuditFile]) -> String {
    let keywords = [
        "auth",
        "authorize",
        "permission",
        "role",
        "session",
        "token",
        "secret",
        "password",
        "key",
        "credential",
        "crypto",
        "encrypt",
        "decrypt",
        "sign",
        "verify",
        "path",
        "file",
        "canonical",
        "symlink",
        "upload",
        "download",
        "shell",
        "command",
        "process",
        "env",
        "http",
        "url",
        "fetch",
        "request",
        "deserialize",
        "unsafe",
        "eval",
        "admin",
    ];
    let mut out = String::new();
    let mut count = 0usize;
    'files: for file in files {
        for (line_no, line) in file.text.lines().enumerate() {
            let lower = line.to_ascii_lowercase();
            if keywords.iter().any(|keyword| lower.contains(keyword)) {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    let _ = writeln!(
                        out,
                        "- {}:{}: {}",
                        file.path,
                        line_no + 1,
                        crate::ui::truncate_chars(trimmed, 180)
                    );
                    count += 1;
                    if count >= SECURITY_INDEX_LIMIT {
                        break 'files;
                    }
                }
            }
        }
    }
    if out.is_empty() {
        "- no keyword hits found".to_string()
    } else {
        out
    }
}

fn chunk_text(chunk: &AuditChunk) -> String {
    let mut out = String::new();
    for file in &chunk.files {
        let _ = writeln!(out, "\n## {}\n", file.path);
        out.push_str(&file.text);
        if !file.text.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

fn compact_to_tokens<'a>(
    model_spec: &str,
    text: &'a str,
    max_tokens: usize,
) -> std::borrow::Cow<'a, str> {
    if session::count_tokens(model_spec, text) <= max_tokens {
        return std::borrow::Cow::Borrowed(text);
    }
    let (short, _) = crate::ui::head_tail(text, max_tokens.saturating_mul(4).max(2000));
    std::borrow::Cow::Owned(short)
}

fn transparency_snippet(options: &AuditOptions) -> String {
    let mut command = String::new();
    if !options.model.trim().is_empty() {
        let _ = write!(command, "OY_MODEL={} ", options.model.trim());
    }
    command.push_str("oy audit");
    if options.out != Path::new("ISSUES.md") {
        let _ = write!(command, " --out {}", options.out.display());
    }
    if !options.focus.trim().is_empty() {
        command.push(' ');
        command.push_str(options.focus.trim());
    }
    format!(
        "> {} `{}` · {}",
        prompts::AUDIT_TRANSPARENCY_PREFIX,
        command,
        Utc::now().format("%Y-%m-%d")
    )
}

pub(crate) fn with_transparency_line(report: &str, snippet: &str) -> String {
    let mut lines = report
        .lines()
        .filter(|line| !line.starts_with(&format!("> {}", prompts::AUDIT_TRANSPARENCY_PREFIX)))
        .collect::<Vec<_>>();
    while lines.first().is_some_and(|line| line.trim().is_empty()) {
        lines.remove(0);
    }
    if lines
        .first()
        .is_none_or(|line| line.trim() != prompts::AUDIT_REPORT_TITLE)
    {
        lines.insert(0, prompts::AUDIT_REPORT_TITLE);
    }
    let insert_at = 1;
    let mut rebuilt = Vec::new();
    rebuilt.extend_from_slice(&lines[..insert_at]);
    rebuilt.push("");
    rebuilt.push(snippet);
    if lines.len() > insert_at {
        rebuilt.push("");
        for line in &lines[insert_at..] {
            if !line.trim().is_empty() || rebuilt.last().is_some_and(|last| !last.trim().is_empty())
            {
                rebuilt.push(line);
            }
        }
    }
    let mut out = rebuilt.join("\n");
    out.push('\n');
    out
}

fn rel_path(root: &Path, path: &Path) -> Result<String> {
    let resolved = path
        .canonicalize()
        .with_context(|| format!("failed resolving {}", path.display()))?;
    if !resolved.starts_with(root) {
        bail!("path escaped workspace: {}", path.display());
    }
    Ok(resolved
        .strip_prefix(root)?
        .to_string_lossy()
        .replace('\\', "/"))
}

fn language_for_path(path: &str) -> &'static str {
    match Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "rs" => "Rust",
        "py" => "Python",
        "go" => "Go",
        "js" | "mjs" | "cjs" => "JavaScript",
        "ts" | "tsx" => "TypeScript",
        "java" => "Java",
        "kt" | "kts" => "Kotlin",
        "swift" => "Swift",
        "rb" => "Ruby",
        "php" => "PHP",
        "cs" => "C#",
        "c" | "h" => "C",
        "cc" | "cpp" | "cxx" | "hpp" => "C++",
        "toml" => "TOML",
        "yaml" | "yml" => "YAML",
        "json" => "JSON",
        "md" => "Markdown",
        "sh" | "bash" => "Shell",
        _ => "Text",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
