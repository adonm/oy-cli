//! Audit file collection: walking, skip rules, manifest building,
//! security-index construction, and chunk assignment.

use anyhow::{Context, Result, bail};
use ignore::WalkBuilder;
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use crate::compaction;

use super::MAX_FILE_BYTES;

#[derive(Debug, Clone)]
pub(crate) struct AuditFile {
    pub(crate) path: String,
    pub(crate) language: &'static str,
    pub(crate) bytes: u64,
    pub(crate) tokens: usize,
    pub(crate) text: String,
}

#[derive(Debug, Clone)]
pub(crate) struct AuditChunk {
    pub(crate) files: Vec<AuditFile>,
    pub(crate) tokens: usize,
}

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
        let tokens = compaction::count_tokens(model_spec, &text).max(1);
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

const SKIP_DIR_PREFIXES: &[&str] = &[".git/", "target/", "node_modules/", ".venv/", ".tmp/"];
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

pub(crate) fn ensure_chunks_fit_prompt(chunks: &[AuditChunk], target_tokens: usize) -> Result<()> {
    if let Some(chunk) = chunks.iter().find(|chunk| chunk.tokens > target_tokens) {
        let files = chunk
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        bail!(
            "audit chunk would exceed the model input budget without truncating review input ({} tokens > {} target tokens): {}; rerun with a more focused repository/path or a larger-context model",
            chunk.tokens,
            target_tokens,
            files
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

pub(super) fn build_security_index(files: &[AuditFile], limit: usize) -> String {
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
                    if count >= limit {
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

pub(crate) fn chunk_text(chunk: &AuditChunk) -> String {
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
