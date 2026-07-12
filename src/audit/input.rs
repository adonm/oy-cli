//! Audit file collection: walking, skip rules, manifest building,
//! security-index construction, and chunk assignment.

use anyhow::{Context, Result, bail};
use ignore::WalkBuilder;
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use super::MAX_FILE_BYTES;

#[derive(Debug, Clone)]
pub(crate) struct AuditFile {
    pub(crate) path: String,
    pub(crate) language: &'static str,
    pub(crate) bytes: u64,
    pub(crate) tokens: usize,
    pub(crate) text: String,
    pub(crate) slice: Option<InputSlice>,
}

#[derive(Debug, Clone)]
pub(crate) struct InputSlice {
    pub(crate) index: usize,
    pub(crate) count: usize,
    pub(crate) start_byte: usize,
    pub(crate) end_byte: usize,
    pub(crate) start_line: usize,
    pub(crate) end_line: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct AuditChunk {
    pub(crate) files: Vec<AuditFile>,
    pub(crate) tokens: usize,
}

const MAX_CHUNK_TEXT_BYTES: usize = 240 * 1024;
const MAX_CHUNK_LINES: usize = 19_000;
const MAX_FILE_SLICE_BYTES: usize = MAX_CHUNK_TEXT_BYTES - 8 * 1024;
const MAX_FILE_SLICE_LINES: usize = MAX_CHUNK_LINES - 8;

pub(crate) fn collect_files(
    root: &Path,
    output_path: Option<&Path>,
    model_spec: &str,
) -> Result<Vec<AuditFile>> {
    let mut files = Vec::new();
    let output_path = output_path.and_then(|path| path.canonicalize().ok());
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
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
        let tokens = count_tokens(model_spec, &text).max(1);
        files.push(AuditFile {
            language: language_for_path(&rel),
            path: rel,
            bytes: meta.len(),
            tokens,
            text,
            slice: None,
        });
    }
    files.sort_by_key(audit_priority);
    Ok(files)
}

pub(crate) fn collect_file(
    root: &Path,
    path: &Path,
    model_spec: &str,
) -> Result<Option<AuditFile>> {
    let rel = rel_path(root, path)?;
    if should_skip_path(&rel) {
        return Ok(None);
    }
    let meta = fs::metadata(path).with_context(|| format!("failed to stat {}", path.display()))?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return Ok(None);
    }
    let raw = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let text = match crate::decode_utf8(raw) {
        Ok(text) if !text.trim().is_empty() => text,
        _ => return Ok(None),
    };
    let tokens = count_tokens(model_spec, &text).max(1);
    Ok(Some(AuditFile {
        language: language_for_path(&rel),
        path: rel,
        bytes: meta.len(),
        tokens,
        text,
        slice: None,
    }))
}

const SKIP_DIR_PREFIXES: &[&str] = &[
    ".git/",
    ".oy/",
    "target/",
    "node_modules/",
    ".venv/",
    ".tmp/",
];
const SKIP_FILENAMES: &[&str] = &[
    "cargo.lock",
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "uv.lock",
    "go.sum",
    ".npmrc",
    ".pypirc",
    ".netrc",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    "issues.md",
    "review.md",
    "oy.sarif",
];
const SKIP_FILENAME_SUBSTRINGS: &[&str] = &["credential", "secret", "token"];
const SKIP_EXTENSIONS: &[&str] = &["pem", "key", "p12", "pfx"];

pub(super) fn should_skip_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if SKIP_DIR_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
    {
        return true;
    }
    let name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if SKIP_FILENAMES.contains(&name.as_str()) {
        return true;
    }
    if name == ".env" || name.starts_with(".env.") {
        return true;
    }
    if !is_source_path(&lower)
        && SKIP_FILENAME_SUBSTRINGS
            .iter()
            .any(|needle| name.contains(needle))
    {
        return true;
    }
    Path::new(&lower)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| SKIP_EXTENSIONS.contains(&extension))
}

fn is_source_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension,
                "rs" | "py"
                    | "go"
                    | "js"
                    | "mjs"
                    | "cjs"
                    | "ts"
                    | "tsx"
                    | "java"
                    | "kt"
                    | "kts"
                    | "swift"
                    | "rb"
                    | "php"
                    | "cs"
                    | "c"
                    | "h"
                    | "cc"
                    | "cpp"
                    | "cxx"
                    | "hpp"
                    | "sh"
                    | "bash"
            )
        })
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

pub(crate) fn chunk_files(files: Vec<AuditFile>, target_tokens: usize) -> Vec<AuditChunk> {
    let files = files
        .into_iter()
        .flat_map(|file| {
            split_file(
                file,
                MAX_FILE_SLICE_BYTES,
                MAX_FILE_SLICE_LINES,
                target_tokens,
            )
        })
        .collect::<Vec<_>>();
    let mut chunks = Vec::new();
    let mut current = Vec::new();
    let mut total = 0usize;
    let mut bytes = 0usize;
    let mut lines = 0usize;
    for file in files {
        let rendered = chunk_file_text(&file);
        let file_bytes = rendered.len();
        let file_lines = rendered.lines().count();
        debug_assert!(file_bytes <= MAX_CHUNK_TEXT_BYTES);
        debug_assert!(file_lines <= MAX_CHUNK_LINES);
        if !current.is_empty()
            && (total + file.tokens > target_tokens
                || bytes + file_bytes > MAX_CHUNK_TEXT_BYTES
                || lines + file_lines > MAX_CHUNK_LINES)
        {
            chunks.push(AuditChunk {
                files: current,
                tokens: total,
            });
            current = Vec::new();
            total = 0;
            bytes = 0;
            lines = 0;
        }
        total += file.tokens;
        bytes += file_bytes;
        lines += file_lines;
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

fn split_file(
    file: AuditFile,
    max_bytes: usize,
    max_lines: usize,
    max_tokens: usize,
) -> Vec<AuditFile> {
    let max_tokens = max_tokens.max(1);
    if file.text.len() <= max_bytes
        && file.text.lines().count() <= max_lines
        && file.tokens <= max_tokens
    {
        return vec![file];
    }
    let mut ranges = Vec::new();
    let mut start = 0usize;
    let mut start_line = 1usize;
    while start < file.text.len() {
        let mut end = (start + max_bytes).min(file.text.len());
        while end > start && !file.text.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            end = file.text[start..]
                .char_indices()
                .nth(1)
                .map_or(file.text.len(), |(offset, _)| start + offset);
        }
        let candidate = &file.text[start..end];
        if candidate.lines().count() > max_lines {
            let mut newlines = 0usize;
            for (offset, ch) in candidate.char_indices() {
                if ch == '\n' {
                    newlines += 1;
                    if newlines == max_lines {
                        end = start + offset + 1;
                        break;
                    }
                }
            }
        }
        if count_tokens("", &file.text[start..end]) > max_tokens {
            end = token_bounded_end(&file.text, start, end, max_tokens);
        }
        let text = &file.text[start..end];
        let newline_count = text.bytes().filter(|byte| *byte == b'\n').count();
        let ends_newline = text.ends_with('\n');
        let end_line = start_line + newline_count - usize::from(ends_newline && newline_count > 0);
        ranges.push((start, end, start_line, end_line));
        start = end;
        start_line = if ends_newline { end_line + 1 } else { end_line };
    }
    let count = ranges.len();
    ranges
        .into_iter()
        .enumerate()
        .map(|(index, (start, end, start_line, end_line))| {
            let text = file.text[start..end].to_string();
            AuditFile {
                path: file.path.clone(),
                language: file.language,
                bytes: text.len() as u64,
                tokens: count_tokens("", &text).max(1),
                text,
                slice: Some(InputSlice {
                    index: index + 1,
                    count,
                    start_byte: start,
                    end_byte: end,
                    start_line,
                    end_line,
                }),
            }
        })
        .collect()
}

fn token_bounded_end(text: &str, start: usize, end: usize, max_tokens: usize) -> usize {
    let boundaries = text[start..end]
        .char_indices()
        .skip(1)
        .map(|(offset, _)| start + offset)
        .chain(std::iter::once(end))
        .collect::<Vec<_>>();
    let fitting = boundaries
        .partition_point(|candidate| count_tokens("", &text[start..*candidate]) <= max_tokens);
    boundaries[fitting.saturating_sub(1)]
}

pub(crate) fn ensure_chunks_fit_prompt(chunks: &[AuditChunk], target_tokens: usize) -> Result<()> {
    if let Some(chunk) = chunks.iter().find(|chunk| chunk.tokens > target_tokens) {
        let files = chunk
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        bail!(
            "audit chunk would exceed the model input budget without truncating review input ({} tokens > {} target tokens): {}; rerun with a more focused repository/path, a larger-context model, or target_tokens >= {}",
            chunk.tokens,
            target_tokens,
            files,
            chunk.tokens
        );
    }
    Ok(())
}

pub(crate) fn build_manifest(files: &[AuditFile]) -> String {
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

pub(crate) fn chunk_text(chunk: &AuditChunk) -> String {
    let mut out = String::new();
    for file in &chunk.files {
        out.push_str(&chunk_file_text(file));
    }
    out
}

fn chunk_file_text(file: &AuditFile) -> String {
    let mut out = String::new();
    if let Some(slice) = &file.slice {
        let _ = writeln!(
            out,
            "\n## {} (slice {}/{}; bytes {}-{}; lines {}-{})\n",
            file.path,
            slice.index,
            slice.count,
            slice.start_byte,
            slice.end_byte,
            slice.start_line,
            slice.end_line
        );
    } else {
        let _ = writeln!(out, "\n## {}\n", file.path);
    }
    out.push_str(&file.text);
    if !file.text.ends_with('\n') {
        out.push('\n');
    }
    out
}

// -----------------------------------------------------------------------
// Git‑diff input source (used by `oy review`)
// -----------------------------------------------------------------------

/// Parses `git diff --no-ext-diff …` output into `AuditFile` items, one per
/// file-level hunk. Binary diffs are silently skipped.
pub(crate) fn collect_diff_files(diff: &str, model: &str) -> Vec<AuditFile> {
    let mut files = Vec::new();
    let mut current = String::new();
    for line in diff.lines() {
        if line.starts_with("diff --git ") && !current.is_empty() {
            push_diff_file(&mut files, &current, model);
            current.clear();
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.is_empty() {
        push_diff_file(&mut files, &current, model);
    }
    files
}

fn push_diff_file(files: &mut Vec<AuditFile>, text: &str, model: &str) {
    if text
        .lines()
        .any(|line| line.starts_with("Binary files ") || line.starts_with("GIT binary patch"))
    {
        return;
    }
    let path = diff_file_path(text).unwrap_or_else(|| format!("diff-{}", files.len() + 1));
    let tokens = count_tokens(model, text).max(1);
    files.push(AuditFile {
        path,
        language: "Diff",
        bytes: text.len() as u64,
        tokens,
        text: text.to_string(),
        slice: None,
    });
}

fn count_tokens(_model: &str, text: &str) -> usize {
    // A deterministic approximation is enough for bounded evidence planning;
    // OpenCode owns model execution and context management.
    text.split_whitespace().count().max(text.len() / 4)
}

fn diff_file_path(text: &str) -> Option<String> {
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

/// Parses `git diff --numstat` output into `(added, deleted, path)` rows.
/// Binary entries use `-` instead of a number.
pub(crate) fn parse_numstat(text: &str) -> Vec<(Option<usize>, Option<usize>, String)> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            Some((
                parts.next()?.parse().ok(),
                parts.next()?.parse().ok(),
                parts.next()?.to_string(),
            ))
        })
        .collect()
}

/// Builds a manifest string for a git‑diff input source.
pub(crate) fn build_diff_manifest(
    target: &str,
    files: &[AuditFile],
    stats: &[(Option<usize>, Option<usize>, String)],
) -> String {
    let estimated_tokens = files.iter().map(|f| f.tokens).sum::<usize>();
    let added = stats.iter().filter_map(|e| e.0).sum::<usize>();
    let deleted = stats.iter().filter_map(|e| e.1).sum::<usize>();
    let mut out = String::new();
    let _ = writeln!(out, "source: git diff against {target}");
    let _ = writeln!(out, "changed_files: {}", files.len());
    let _ = writeln!(out, "estimated_tokens: {estimated_tokens}");
    let _ = writeln!(out, "added_lines: {added}");
    let _ = writeln!(out, "deleted_lines: {deleted}");
    out.push_str("changed files:\n");
    for file in files.iter().take(80) {
        let _ = writeln!(out, "- {} ({} tokens)", file.path, file.tokens);
    }
    out
}

fn rel_path(root: &Path, path: &Path) -> Result<String> {
    let resolved = path
        .canonicalize()
        .with_context(|| format!("failed resolving {}", path.display()))?;
    if !resolved.starts_with(root) {
        bail!("path escaped workspace: {}", path.display());
    }
    Ok(resolved.strip_prefix(root)?.to_string_lossy().into_owned())
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
mod slice_tests {
    use super::*;

    fn file(path: String, text: String) -> AuditFile {
        AuditFile {
            path,
            language: "Text",
            bytes: text.len() as u64,
            tokens: 1,
            text,
            slice: None,
        }
    }

    #[test]
    fn oversized_utf8_file_is_reconstructable_and_bounded() {
        let text = "λ line\n".repeat(40_000);
        let mut file = file("large.txt".to_string(), text.clone());
        file.tokens = count_tokens("", &text);
        let chunks = chunk_files(vec![file], 64_000);
        assert!(chunks.len() > 1);
        let reconstructed = chunks
            .iter()
            .flat_map(|chunk| chunk.files.iter())
            .map(|file| file.text.as_str())
            .collect::<String>();
        assert_eq!(reconstructed, text);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk_text(chunk).len() <= MAX_CHUNK_TEXT_BYTES)
        );
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk_text(chunk).lines().count() <= MAX_CHUNK_LINES)
        );
    }

    #[test]
    fn packed_chunks_follow_documented_byte_and_line_bounds() {
        let byte_chunks = chunk_files(
            (0..6)
                .map(|index| file(format!("bytes-{index}.txt"), "x".repeat(80 * 1024)))
                .collect(),
            usize::MAX,
        );
        assert!(byte_chunks.len() > 1);
        assert!(
            byte_chunks
                .iter()
                .all(|chunk| chunk_text(chunk).len() <= MAX_CHUNK_TEXT_BYTES)
        );

        let line_chunks = chunk_files(
            (0..4)
                .map(|index| file(format!("lines-{index}.txt"), "x\n".repeat(8_000)))
                .collect(),
            usize::MAX,
        );
        assert!(line_chunks.len() > 1);
        assert!(
            line_chunks
                .iter()
                .all(|chunk| chunk_text(chunk).lines().count() <= MAX_CHUNK_LINES)
        );
    }

    #[test]
    fn token_dense_files_are_sliced_to_the_chunk_budget() {
        let text = "x ".repeat(70_000);
        let mut input = file("tokens.txt".to_string(), text.clone());
        input.tokens = count_tokens("", &text);

        let chunks = chunk_files(vec![input], 64_000);

        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|chunk| chunk.tokens <= 64_000));
        assert_eq!(
            chunks
                .iter()
                .flat_map(|chunk| &chunk.files)
                .map(|file| file.text.as_str())
                .collect::<String>(),
            text
        );
    }
}
