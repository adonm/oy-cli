use anyhow::{Context, Result, bail};
use chrono::Utc;
use futures_util::{StreamExt as _, stream};
use ignore::WalkBuilder;
use regex::Regex;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use crate::{config, model, session};

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

#[derive(Debug, Clone, Copy)]
struct AuditProgress {
    started: Instant,
    file_count: usize,
    chunk_count: usize,
}

impl AuditProgress {
    fn new(started: Instant, file_count: usize, chunk_count: usize) -> Self {
        Self {
            started,
            file_count,
            chunk_count,
        }
    }

    fn prepared(&self) {
        self.line(
            "prepared",
            1,
            1,
            format_args!("{} files · {} chunks", self.file_count, self.chunk_count),
        );
    }

    fn review_started(&self, parallelism: Option<usize>) {
        let detail = match parallelism {
            Some(parallelism) => format!(
                "reviewing {} chunks · parallelism {parallelism}",
                self.chunk_count
            ),
            None => "reviewing full repo".to_string(),
        };
        self.line("review", 0, self.chunk_count, detail);
    }

    fn review_finished(&self, completed: usize) {
        if completed < self.chunk_count && !completed.is_multiple_of(self.review_update_stride()) {
            return;
        }
        let detail = if completed >= self.chunk_count {
            "review complete".to_string()
        } else {
            format!("{completed}/{} chunks complete", self.chunk_count)
        };
        self.line("review", completed, self.chunk_count, detail);
    }

    fn review_update_stride(&self) -> usize {
        self.chunk_count.div_ceil(10).max(1)
    }

    fn summarise_started(&self) {
        self.line("summarise", 0, 1, "deduping and ranking findings");
    }

    fn summarise_finished(&self) {
        self.line("summarise", 1, 1, "summary complete");
    }

    fn write_started(&self, output_path: &Path) {
        self.line("write", 0, 1, output_path.display());
    }

    fn write_finished(&self, output_path: &Path) {
        self.line("write", 1, 1, output_path.display());
    }

    fn line(&self, label: &str, current: usize, total: usize, detail: impl std::fmt::Display) {
        crate::ui::progress(label, current, total, detail, self.started.elapsed());
    }
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
        let text = match crate::decode_utf8(raw) {
            Ok(text) => text,
            Err(_) => continue,
        };
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
    files.sort_by_key(audit_priority);
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
    std::borrow::Cow::Owned(compact_owned_to_tokens(model_spec, text, max_tokens))
}

fn compact_owned_to_tokens(model_spec: &str, text: &str, max_tokens: usize) -> String {
    let mut max_chars = max_tokens.saturating_mul(4).max(2000);
    loop {
        let (short, truncated) = crate::ui::head_tail(text, max_chars);
        if !truncated || session::count_tokens(model_spec, &short) <= max_tokens || max_chars <= 512
        {
            return short;
        }
        max_chars = max_chars.saturating_mul(3) / 4;
    }
}

fn reduce_candidate_findings_budget(
    model_spec: &str,
    focus: &str,
    manifest: &str,
    max_prompt_tokens: usize,
) -> usize {
    let prompt_without_findings = prompts::audit_reduce_prompt(focus, manifest, "");
    let overhead_tokens = session::count_tokens(model_spec, &prompt_without_findings);
    max_prompt_tokens
        .saturating_sub(overhead_tokens)
        .saturating_sub(REDUCE_FINDINGS_TOKEN_RESERVE)
        .max(REDUCE_FINDINGS_MIN_TOKENS)
}

fn bounded_reduce_findings(
    model_spec: &str,
    focus: &str,
    manifest: &str,
    findings: &str,
    max_prompt_tokens: usize,
) -> String {
    let prompt_tokens = |findings: &str| {
        let prompt = prompts::audit_reduce_prompt(focus, manifest, findings);
        session::count_tokens(model_spec, &prompt)
    };
    if prompt_tokens(findings) <= max_prompt_tokens {
        return findings.to_string();
    }

    let findings_budget =
        reduce_candidate_findings_budget(model_spec, focus, manifest, max_prompt_tokens);
    let mut current_budget = findings_budget;
    let mut bounded = compact_owned_to_tokens(model_spec, findings, current_budget);

    while prompt_tokens(&bounded) > max_prompt_tokens && current_budget > REDUCE_FINDINGS_MIN_TOKENS
    {
        current_budget = (current_budget.saturating_mul(3) / 4).max(REDUCE_FINDINGS_MIN_TOKENS);
        bounded = compact_owned_to_tokens(model_spec, findings, current_budget);
    }

    bounded
}

fn transparency_snippet(options: &AuditOptions) -> String {
    let mut command = Vec::new();
    if !options.model.trim().is_empty() {
        command.push(format!("OY_MODEL={}", shell_quote(options.model.trim())));
    }
    command.push("oy".to_string());
    command.push("audit".to_string());
    if options.format != AuditOutputFormat::Markdown {
        command.push("--format".to_string());
        command.push(options.format.name().to_string());
    }
    if options.out != default_output_path(options.format) {
        command.push("--out".to_string());
        command.push(shell_quote(&options.out.to_string_lossy()));
    }
    if options.max_chunks != DEFAULT_MAX_REVIEW_CHUNKS {
        command.push("--max-chunks".to_string());
        command.push(options.max_chunks.to_string());
    }
    if !options.focus.trim().is_empty() {
        command.push(shell_quote(options.focus.trim()));
    }
    format!(
        "> {} `{}` · {}",
        prompts::AUDIT_TRANSPARENCY_PREFIX,
        command.join(" "),
        Utc::now().format("%Y-%m-%d")
    )
}

pub fn default_output_path(format: AuditOutputFormat) -> PathBuf {
    match format {
        AuditOutputFormat::Markdown => PathBuf::from("ISSUES.md"),
        AuditOutputFormat::Sarif => PathBuf::from("oy.sarif"),
    }
}

fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | ':' | '='))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
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
    finish_markdown(rebuilt)
}

pub(crate) fn with_succinct_findings_summary(report: &str) -> String {
    let lines = report.lines().collect::<Vec<_>>();
    if has_heading(&lines, "Findings summary") {
        return finish_markdown(lines);
    }
    let findings = extract_findings(&lines);
    if findings.is_empty() {
        return finish_markdown(lines);
    }

    let insert_at = transparency_insert_index(&lines);
    let mut rebuilt = Vec::with_capacity(lines.len() + findings.len() + 4);
    rebuilt.extend(lines[..insert_at].iter().map(|line| (*line).to_string()));
    if rebuilt.last().is_some_and(|line| !line.trim().is_empty()) {
        rebuilt.push(String::new());
    }
    rebuilt.push("## Findings summary".to_string());
    rebuilt.push(String::new());
    rebuilt.extend(findings.into_iter().map(|finding| finding.to_markdown()));
    rebuilt.push(String::new());
    rebuilt.extend(lines[insert_at..].iter().map(|line| (*line).to_string()));
    finish_markdown_owned(rebuilt)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FindingSummary {
    severity: String,
    title: String,
    code_ref: String,
}

impl FindingSummary {
    fn to_markdown(&self) -> String {
        format!(
            "- **{}** `{}` — {}",
            self.severity, self.code_ref, self.title
        )
    }
}

fn extract_findings(lines: &[&str]) -> Vec<FindingSummary> {
    static HEADING_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"^(#{2,4})\s+(.+?)\s*$").expect("valid heading regex")
    });
    let mut findings = Vec::new();
    let mut current: Option<(String, Vec<&str>)> = None;

    for line in lines {
        if let Some(captures) = HEADING_RE.captures(line) {
            if let Some((heading, body)) = current.take()
                && let Some(finding) = finding_from_section(&heading, &body)
            {
                findings.push(finding);
            }
            let level = captures.get(1).map(|m| m.as_str().len()).unwrap_or(0);
            let heading = captures
                .get(2)
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default();
            if level >= 2 && is_finding_heading(&heading) {
                current = Some((heading, Vec::new()));
            } else {
                current = None;
            }
        } else if let Some((_, body)) = current.as_mut() {
            body.push(line);
        }
    }
    if let Some((heading, body)) = current.take()
        && let Some(finding) = finding_from_section(&heading, &body)
    {
        findings.push(finding);
    }
    findings
}

fn finding_from_section(heading: &str, body: &[&str]) -> Option<FindingSummary> {
    let severity = severity_from_text(heading)
        .or_else(|| body.iter().find_map(|line| severity_from_text(line)))
        .unwrap_or_else(|| "Unrated".to_string());
    let title = clean_finding_title(heading);
    let code_ref = body
        .iter()
        .find_map(|line| code_ref_from_line(line))
        .or_else(|| code_ref_from_line(heading))?;
    Some(FindingSummary {
        severity,
        title,
        code_ref,
    })
}

fn is_finding_heading(heading: &str) -> bool {
    let lower = heading.to_ascii_lowercase();
    !matches!(
        lower.as_str(),
        "findings summary"
            | "summary"
            | "detailed findings"
            | "details"
            | "no concrete findings"
            | "audit issues"
    )
}

fn severity_from_text(text: &str) -> Option<String> {
    static SEVERITY_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?i)\b(critical|high|medium|low|info|informational)\b")
            .expect("valid severity regex")
    });
    SEVERITY_RE
        .captures(text)
        .and_then(|captures| captures.get(1))
        .map(
            |match_| match match_.as_str().to_ascii_lowercase().as_str() {
                "critical" => "Critical".to_string(),
                "high" => "High".to_string(),
                "medium" => "Medium".to_string(),
                "low" => "Low".to_string(),
                _ => "Info".to_string(),
            },
        )
}

fn clean_finding_title(heading: &str) -> String {
    static TITLE_SEVERITY_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(
            r"(?i)^\s*[\[(]?\s*(informational|critical|high|medium|low|info)\s*[\])]?\s*[:—–-]+\s*",
        )
        .expect("valid title severity regex")
    });
    let title = heading.trim().trim_matches('#').trim();
    let title = TITLE_SEVERITY_RE.replace(title, "").trim().to_string();
    if title.is_empty() {
        "Untitled finding".to_string()
    } else {
        title
    }
}

fn code_ref_from_line(line: &str) -> Option<String> {
    static CODE_REF_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"[A-Za-z0-9_.@+\-/]+\.[A-Za-z0-9]+(?::\d+)?(?:::[A-Za-z_][A-Za-z0-9_]*)?")
            .expect("valid code reference regex")
    });
    CODE_REF_RE.find(line).map(|match_| {
        match_
            .as_str()
            .trim_matches(|ch: char| ch == '`' || ch == ',' || ch == ')' || ch == ']')
            .to_string()
    })
}

fn has_heading(lines: &[&str], heading: &str) -> bool {
    lines.iter().any(|line| {
        line.trim_start_matches('#')
            .trim()
            .eq_ignore_ascii_case(heading)
    })
}

fn transparency_insert_index(lines: &[&str]) -> usize {
    lines
        .iter()
        .position(|line| line.starts_with(&format!("> {}", prompts::AUDIT_TRANSPARENCY_PREFIX)))
        .map(|idx| idx + 1)
        .unwrap_or_else(|| {
            lines
                .iter()
                .position(|line| line.trim() == prompts::AUDIT_REPORT_TITLE)
                .map(|idx| idx + 1)
                .unwrap_or(0)
        })
}

fn finish_markdown(lines: Vec<&str>) -> String {
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn finish_markdown_owned(lines: Vec<String>) -> String {
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn render_sarif(report: &str) -> Result<String> {
    let findings = extract_findings(&report.lines().collect::<Vec<_>>());
    let mut rules = std::collections::BTreeMap::<String, Value>::new();
    let mut results = Vec::new();

    for finding in findings {
        let Some(location) = sarif_location(&finding.code_ref)? else {
            continue;
        };
        let rule_id = sarif_rule_id(&finding);
        let level = sarif_level(&finding.severity);
        rules.entry(rule_id.clone()).or_insert_with(|| {
            json!({
                "id": rule_id,
                "name": finding.title,
                "shortDescription": { "text": finding.title },
                "defaultConfiguration": { "level": level },
                "properties": {
                    "severity": finding.severity,
                    "security-severity": sarif_security_severity(&finding.severity)
                }
            })
        });
        results.push(json!({
            "ruleId": rule_id,
            "level": level,
            "message": { "text": format!("{}: {}", finding.severity, finding.title) },
            "locations": [location],
            "properties": {
                "severity": finding.severity,
                "codeRef": finding.code_ref
            }
        }));
    }

    let sarif = json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "oy-cli",
                    "semanticVersion": env!("CARGO_PKG_VERSION"),
                    "informationUri": "https://github.com/wagov-dtt/oy-cli",
                    "rules": rules.into_values().collect::<Vec<_>>()
                }
            },
            "results": results,
            "columnKind": "utf16CodeUnits"
        }]
    });
    let mut out = serde_json::to_string_pretty(&sarif)?;
    out.push('\n');
    Ok(out)
}

fn sarif_rule_id(finding: &FindingSummary) -> String {
    let mut slug = String::new();
    for ch in finding.title.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() { "finding" } else { slug };
    format!("oy/{}/{}", finding.severity.to_ascii_lowercase(), slug)
}

fn sarif_level(severity: &str) -> &'static str {
    match severity.to_ascii_lowercase().as_str() {
        "critical" | "high" => "error",
        "medium" => "warning",
        _ => "note",
    }
}

fn sarif_security_severity(severity: &str) -> &'static str {
    match severity.to_ascii_lowercase().as_str() {
        "critical" => "9.0",
        "high" => "7.0",
        "medium" => "5.0",
        "low" => "2.0",
        _ => "0.0",
    }
}

fn sarif_location(code_ref: &str) -> Result<Option<Value>> {
    let (path, line) = split_code_ref(code_ref);
    if !is_safe_relative_path(path) {
        bail!("audit finding path escapes workspace: {path}");
    }
    let mut region = serde_json::Map::new();
    if let Some(line) = line {
        region.insert("startLine".to_string(), json!(line));
    }
    let mut physical = serde_json::Map::new();
    physical.insert(
        "artifactLocation".to_string(),
        json!({ "uri": path.replace('\\', "/"), "uriBaseId": "%SRCROOT%" }),
    );
    if !region.is_empty() {
        physical.insert("region".to_string(), Value::Object(region));
    }
    Ok(Some(json!({ "physicalLocation": Value::Object(physical) })))
}

fn split_code_ref(code_ref: &str) -> (&str, Option<u32>) {
    if let Some((path, tail)) = code_ref.rsplit_once(':')
        && !tail.contains(':')
        && let Ok(line) = tail.parse::<u32>()
    {
        return (path, Some(line));
    }
    (
        code_ref
            .split_once("::")
            .map(|(path, _)| path)
            .unwrap_or(code_ref),
        None,
    )
}

fn is_safe_relative_path(path: &str) -> bool {
    let path = Path::new(path);
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
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
        let value: Value = serde_json::from_str(&sarif).unwrap();
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
        assert!(session::count_tokens("gpt-4o", &compact) <= 1_000);
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
        assert!(session::count_tokens("gpt-4o", &prompt) <= 20_000);
        assert!(bounded.contains("truncated"));
    }
}

// === Audit prompts ===
mod prompts {
    use std::fmt::Write as _;

    pub const AUDIT_REPORT_TITLE: &str = "# Audit Issues";
    pub const AUDIT_TRANSPARENCY_PREFIX: &str =
        "Generated with [oy-cli](https://github.com/wagov-dtt/oy-cli):";

    pub const AUDIT_SYSTEM_PROMPT: &str = r#"You are oy in audit mode. Audit the repository for security issues, unnecessary complexity, and material usability or performance problems.
Be terse, evidence-first, and repo-specific. Avoid generic best-practice advice, style nits, and speculation.

Finding quality bar:
- Report only concrete issues with a plausible attack path, trigger, broken invariant, data exposure, integrity risk, privilege impact, or material operational impact.
- For vulnerabilities, include the trust boundary, sink, affected path/symbol evidence, impact, exploitability/preconditions, and a concrete fix.
- Prefer critical/high security findings and issues likely to cause production incidents.
- Prefer simple remediations that remove whole bug classes.
- Return [] or say no concrete findings for a chunk when evidence is weak.
- Final reports must include a succinct all-findings summary with code references, then detailed writeups for only the most severe 10-20 findings.

Use the embedded OWASP/grugbrain reference as a lightweight checklist and citation guide. Spend tokens on repository evidence, not long standards explanations."#;

    pub const AUDIT_REFERENCE: &str = r#"Audit reference checklist:

OWASP ASVS 5.0 quick map:
- V1 Architecture: trust boundaries, secure design, attack surface, threat model gaps, dangerous defaults.
- V2 Authentication: credential handling, MFA, session/auth lifecycle, account recovery.
- V3 Session: cookie/token handling, fixation, expiration, revocation, CSRF-relevant state.
- V4 Access Control: object/function authorization, tenant isolation, confused deputy paths.
- V5 Validation: parser boundaries, canonicalization, path traversal, SSRF, injection, deserialization.
- V6 Cryptography: key management, weak/custom crypto, randomness, secret storage.
- V7 Error/Logging: secret leakage, unsafe diagnostics, audit trail gaps.
- V8 Data Protection: sensitive data at rest/in transit, retention, cache/backup exposure.
- V9 Communications: TLS verification, hostname validation, downgrade/debug transport.
- V10 Malicious Code: supply chain, unsafe dynamic loading, dependency/update risk.
- V11 Business Logic: state-machine bypass, race/double-submit, workflow abuse.
- V12 Files/Resources: upload/download, archive extraction, filesystem boundaries, quotas.
- V13 API/Web Service: mass assignment, schema validation, rate limits, authz on APIs.
- V14 Configuration: insecure defaults, debug flags, secret/config sprawl.

OWASP MASVS/MASWE for mobile repos only:
- STORAGE, CRYPTO, AUTH, NETWORK, PLATFORM, CODE, RESILIENCE, PRIVACY; use MASWE IDs only when a concrete mobile weakness maps cleanly.

Grugbrain complexity filter:
- Grugbrain has no formal section IDs; do not invent citations. Use exact lookup phrases only.
- Useful phrases: `complexity very bad`, `local reasoning`, `small sharp tools`, `avoid wrong abstraction`, `too much abstraction`, `closures like salt`, `reproduce bug first`, `testing`.
- Use grugbrain for complexity/maintainability findings, or as secondary support where complexity materially increases exploitability or review failure risk.

Combined heuristic:
- Security bug plus high complexity is higher priority because it is harder to review, fix safely, and prevent from recurring.
- Prefer findings where code both violates a security control and hides that violation behind abstraction, config sprawl, hidden state, or broad capability.
- If a simpler design removes an entire bug class, say so explicitly."#;

    pub fn audit_chunk_prompt(
        focus: &str,
        manifest: &str,
        index: &str,
        chunk_id: usize,
        chunk_count: usize,
        chunk_text: &str,
    ) -> String {
        let mut prompt = String::new();
        let _ = writeln!(prompt, "Review audit chunk {chunk_id}/{chunk_count}.");
        push_focus(&mut prompt, focus);
        prompt.push_str("\nReturn concise candidate findings for this chunk only. Use markdown with one `###` heading per finding, or return `[]` if there are no concrete findings. For each finding include severity, category, evidence path/symbol, trust boundary/sink when security-relevant, impact, reference, and fix. Do not write files.\n\n");
        prompt.push_str("Repository manifest:\n");
        prompt.push_str(manifest.trim());
        prompt.push_str("\n\nSecurity-relevant index:\n");
        prompt.push_str(index.trim());
        prompt.push_str("\n\nChunk contents:\n");
        prompt.push_str(chunk_text.trim());
        prompt
    }

    pub fn audit_full_prompt(focus: &str, manifest: &str, index: &str, repo_text: &str) -> String {
        let mut prompt = String::new();
        prompt.push_str("Conduct a full repository audit and return the final markdown report.\n");
        push_focus(&mut prompt, focus);
        prompt.push_str("\nReport format:\n1. Start with `# Audit Issues`.\n2. Add `## Findings summary` with one succinct bullet/table row for every concrete finding, including severity, short title, and code reference (`path:line` or `path::symbol`).\n3. Add `## Detailed findings` for only the most severe 10-20 findings, ranked by severity/exploitability/impact; include category, evidence, trust boundary/sink where security-relevant, impact, exploitability/preconditions, reference, and fix.\n4. Avoid generic advice. Do not write files.\n\n");
        prompt.push_str("Repository manifest:\n");
        prompt.push_str(manifest.trim());
        prompt.push_str("\n\nSecurity-relevant index:\n");
        prompt.push_str(index.trim());
        prompt.push_str("\n\nRepository contents:\n");
        prompt.push_str(repo_text.trim());
        prompt
    }

    pub fn audit_reduce_prompt(focus: &str, manifest: &str, findings: &str) -> String {
        let mut prompt = String::new();
        prompt.push_str("Condense candidate audit findings into the final markdown report.\n");
        push_focus(&mut prompt, focus);
        prompt.push_str("\nReport format:\n1. Start with `# Audit Issues`.\n2. Add `## Findings summary` with one succinct bullet/table row for every concrete finding that survives dedupe, including severity, short title, and code reference (`path:line` or `path::symbol`).\n3. Add `## Detailed findings` for only the most severe 10-20 findings, ranked by severity/exploitability/impact; preserve the shortest evidence needed to prove exploitability or impact, plus category, trust boundary/sink where security-relevant, reference, and fix.\n4. Drop weak/speculative/duplicate items, but do not omit concrete lower-severity findings from the summary.\n\n");
        prompt.push_str("Repository manifest:\n");
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

    pub fn audit_system_prompt() -> String {
        format!(
            "{}\n\n{}",
            AUDIT_SYSTEM_PROMPT.trim(),
            AUDIT_REFERENCE.trim()
        )
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn audit_system_prompt_embeds_owasp_and_grugbrain_reference() {
            let prompt = audit_system_prompt();
            assert!(prompt.contains("OWASP ASVS 5.0"));
            assert!(prompt.contains("Grugbrain"));
            assert!(prompt.contains("complexity very bad"));
            assert!(prompt.contains("trust boundary"));
        }

        #[test]
        fn audit_prompts_include_focus_when_present() {
            let prompt = audit_full_prompt("auth paths", "files: 1", "- hit", "src/lib.rs");
            assert!(prompt.contains("Additional focus: auth paths"));
            assert!(prompt.contains("# Audit Issues"));
        }

        #[test]
        fn final_audit_prompts_request_succinct_summary_and_limited_details() {
            let full = audit_full_prompt("", "files: 1", "- hit", "src/lib.rs");
            assert!(full.contains("## Findings summary"));
            assert!(full.contains("every concrete finding"));
            assert!(full.contains("most severe 10-20"));

            let reduce = audit_reduce_prompt("", "files: 1", "### High\nEvidence: src/lib.rs:1");
            assert!(reduce.contains("## Findings summary"));
            assert!(reduce.contains("do not omit concrete lower-severity findings"));
            assert!(reduce.contains("most severe 10-20"));
        }
    }
}
