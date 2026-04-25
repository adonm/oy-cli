use anyhow::{Context, Result, anyhow, bail};
use bzip2::read::BzDecoder;
use flate2::read::GzDecoder;
use glob::glob;
use globset::{Glob, GlobSet, GlobSetBuilder};
use grep_regex::RegexMatcher;
use grep_searcher::SearcherBuilder;
use grep_searcher::sinks::UTF8;
use ignore::WalkBuilder;
use isahc::{AsyncReadResponseExt, Request, ResponseExt, config::Configurable};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Cursor, Read};
use std::net::IpAddr;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use strsim::levenshtein;
use tokei::{Config as TokeiConfig, Languages};
use tokio::net::lookup_host;
use tokio::process::Command;
use tokio::time::timeout;
use toon_format::encode_default;
use tree_sitter::Parser;
use url::Url;
use xz2::read::XzDecoder;
use zip::ZipArchive;

use genai::chat::Tool;

use crate::config;

pub const DEFAULT_LIMIT: usize = 910;
pub const DEFAULT_WEBFETCH_TIMEOUT_SECONDS: u64 = 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub task: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolContext {
    pub root: PathBuf,
    pub interactive: bool,
    pub yolo: bool,
    pub agent: String,
    pub todos: Vec<TodoItem>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ExcludeArg {
    String(String),
    Array(Vec<String>),
}

impl ExcludeArg {
    fn patterns(&self) -> Vec<String> {
        match self {
            Self::String(value) => value
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(ToOwned::to_owned)
                .collect(),
            Self::Array(values) => values
                .iter()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ListArgs {
    #[serde(default = "default_glob")]
    path: String,
    #[serde(default)]
    exclude: Option<ExcludeArg>,
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct ReadArgs {
    path: String,
    #[serde(default = "default_offset")]
    offset: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct SearchArgs {
    pattern: String,
    #[serde(default = "default_dot")]
    path: String,
    #[serde(default)]
    fuzzy: Option<String>,
    #[serde(default)]
    best_match: bool,
    #[serde(default)]
    enhance_match: bool,
    #[serde(default)]
    exclude: Option<ExcludeArg>,
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct ReplaceArgs {
    pattern: String,
    replacement: String,
    #[serde(default = "default_dot")]
    path: String,
    #[serde(default)]
    exclude: Option<ExcludeArg>,
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct SlocArgs {
    #[serde(default = "default_dot")]
    path: String,
    #[serde(default)]
    exclude: Option<ExcludeArg>,
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct BashArgs {
    command: String,
    #[serde(default = "default_bash_timeout")]
    timeout_seconds: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct WebfetchArgs {
    url: String,
    #[serde(default = "default_method")]
    method: String,
    #[serde(default)]
    headers: Option<BTreeMap<String, String>>,
    #[serde(default)]
    follow_redirects: bool,
    #[serde(default = "default_web_timeout")]
    timeout_seconds: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct AskArgs {
    question: String,
    #[serde(default)]
    choices: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
struct TodoArgs {
    todos: Vec<TodoItem>,
}

fn default_glob() -> String {
    "*".to_string()
}
fn default_dot() -> String {
    ".".to_string()
}
fn default_limit() -> usize {
    DEFAULT_LIMIT
}
fn default_offset() -> usize {
    1
}
fn default_bash_timeout() -> u64 {
    120
}
fn default_method() -> String {
    "GET".to_string()
}
fn default_web_timeout() -> u64 {
    DEFAULT_WEBFETCH_TIMEOUT_SECONDS
}

pub fn tool_specs(ctx: &ToolContext) -> Vec<Tool> {
    let mut tools = vec![
        Tool::new("list")
            .with_description(&crate::config::tool_description("list"))
            .with_schema(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "default": "*"},
                    "exclude": {"anyOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}, {"type": "null"}]},
                    "limit": {"type": "integer", "default": DEFAULT_LIMIT}
                },
                "additionalProperties": false
            })),
        Tool::new("read")
            .with_description(&crate::config::tool_description("read"))
            .with_schema(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "offset": {"type": "integer", "default": 1},
                    "limit": {"type": "integer", "default": DEFAULT_LIMIT}
                },
                "required": ["path"],
                "additionalProperties": false
            })),
        Tool::new("search")
            .with_description(&crate::config::tool_description("search"))
            .with_schema(json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string"},
                    "path": {"type": "string", "default": "."},
                    "fuzzy": {"type": ["string", "null"]},
                    "best_match": {"type": "boolean", "default": false},
                    "enhance_match": {"type": "boolean", "default": false},
                    "exclude": {"anyOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}, {"type": "null"}]},
                    "limit": {"type": "integer", "default": DEFAULT_LIMIT}
                },
                "required": ["pattern"],
                "additionalProperties": false
            })),
        Tool::new("sloc")
            .with_description(&crate::config::tool_description("sloc"))
            .with_schema(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "default": "."},
                    "exclude": {"anyOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}, {"type": "null"}]},
                    "limit": {"type": "integer", "default": DEFAULT_LIMIT}
                },
                "additionalProperties": false
            })),
        Tool::new("todo")
            .with_description(&crate::config::tool_description("todo"))
            .with_schema(json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": {"type": "string"},
                                "task": {"type": "string"},
                                "status": {"type": "string", "enum": ["pending", "in_progress", "done"]}
                            },
                            "required": ["id", "task", "status"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["todos"],
                "additionalProperties": false
            })),
    ];

    if ctx.interactive {
        tools.push(
            Tool::new("ask")
                .with_description(&crate::config::tool_description("ask"))
                .with_schema(json!({
                    "type": "object",
                    "properties": {
                        "question": {"type": "string"},
                        "choices": {"type": ["array", "null"], "items": {"type": "string"}}
                    },
                    "required": ["question"],
                    "additionalProperties": false
                })),
        );
    }

    if ctx.agent != "plan" {
        tools.push(
            Tool::new("webfetch")
                .with_description(&crate::config::tool_description("webfetch"))
                .with_schema(json!({
                    "type": "object",
                    "properties": {
                        "url": {"type": "string"},
                        "method": {"type": "string", "default": "GET"},
                        "headers": {"type": ["object", "null"], "additionalProperties": {"type": "string"}},
                        "follow_redirects": {"type": "boolean", "default": false},
                        "timeout_seconds": {"type": "integer", "default": DEFAULT_WEBFETCH_TIMEOUT_SECONDS}
                    },
                    "required": ["url"],
                    "additionalProperties": false
                })),
        );
    }

    if ctx.agent != "plan" {
        tools.push(
            Tool::new("replace")
                .with_description(&crate::config::tool_description("replace"))
                .with_schema(json!({
                    "type": "object",
                    "properties": {
                        "pattern": {"type": "string"},
                        "replacement": {"type": "string"},
                        "path": {"type": "string", "default": "."},
                        "exclude": {"anyOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}, {"type": "null"}]},
                        "limit": {"type": "integer", "default": DEFAULT_LIMIT}
                    },
                    "required": ["pattern", "replacement"],
                    "additionalProperties": false
                })),
        );
    }

    if ctx.agent != "plan" {
        tools.push(
            Tool::new("bash")
                .with_description(&crate::config::tool_description("bash"))
                .with_schema(json!({
                    "type": "object",
                    "properties": {
                        "command": {"type": "string"},
                        "timeout_seconds": {"type": "integer", "default": 120}
                    },
                    "required": ["command"],
                    "additionalProperties": false
                })),
        );
    }

    tools
}

pub async fn invoke(ctx: &mut ToolContext, name: &str, args: Value) -> Result<Value> {
    match name {
        "list" => tool_list(ctx, serde_json::from_value(args)?),
        "read" => tool_read(ctx, serde_json::from_value(args)?),
        "search" => tool_search(ctx, serde_json::from_value(args)?),
        "replace" => tool_replace(ctx, serde_json::from_value(args)?),
        "sloc" => tool_sloc(ctx, serde_json::from_value(args)?),
        "bash" => tool_bash(ctx, serde_json::from_value(args)?).await,
        "webfetch" => tool_webfetch(ctx, serde_json::from_value(args)?).await,
        "ask" => tool_ask(ctx, serde_json::from_value(args)?),
        "todo" => tool_todo(ctx, serde_json::from_value(args)?),
        other => bail!("unknown tool: {other}"),
    }
}

pub fn encode_tool_output(value: &Value) -> String {
    encode_default(value).unwrap_or_else(|_| {
        serde_json::to_string_pretty(value).unwrap_or_else(|_| String::from("{}"))
    })
}

fn tool_list(ctx: &ToolContext, args: ListArgs) -> Result<Value> {
    validate_pattern(&args.path)?;
    let exclude = build_exclude_set(args.exclude.as_ref())?;
    let shown_limit = args.limit.max(1);
    let items = if args.path == "." || args.path == "./" {
        let mut out = Vec::new();
        for entry in fs::read_dir(&ctx.root)? {
            let path = entry?.path();
            let rel = rel_path(&ctx.root, &path);
            if exclude.is_match(rel.as_str()) {
                continue;
            }
            out.push(display_path(&ctx.root, &path));
        }
        out.sort();
        out
    } else {
        let pattern = ctx.root.join(&args.path).to_string_lossy().to_string();
        let mut out = glob(&pattern)?
            .filter_map(|entry| entry.ok())
            .filter(|path| within_root(&ctx.root, path))
            .filter(|path| !exclude.is_match(rel_path(&ctx.root, path).as_str()))
            .map(|path| display_path(&ctx.root, &path))
            .collect::<Vec<_>>();
        out.sort();
        out.dedup();
        out
    };
    Ok(json!({
        "path": args.path,
        "items": items.iter().take(shown_limit).cloned().collect::<Vec<_>>(),
        "count": items.len(),
        "truncated": items.len() > shown_limit,
        "exclude": args.exclude.as_ref().map(ExcludeArg::patterns)
    }))
}

fn tool_read(ctx: &ToolContext, args: ReadArgs) -> Result<Value> {
    let target = resolve_existing_path(&ctx.root, &args.path)?;
    if target.is_dir() {
        bail!("read path is a directory: {}", args.path);
    }
    let mut shown = Vec::new();
    let mut line_count = 0usize;
    let start = args.offset.saturating_sub(1);
    let stop = start + args.limit.max(1);
    let file = fs::File::open(&target)?;
    for (idx, line) in BufReader::new(file).lines().enumerate() {
        let line = line.unwrap_or_default();
        line_count = idx + 1;
        if idx < start {
            continue;
        }
        if idx < stop {
            shown.push(line);
        }
    }
    let truncated = line_count > stop;
    Ok(json!({
        "path": args.path,
        "offset": args.offset,
        "limit": args.limit,
        "text": shown.join("\n"),
        "line_count": line_count,
        "truncated": truncated
    }))
}

fn tool_search(ctx: &ToolContext, args: SearchArgs) -> Result<Value> {
    let regex = if args.fuzzy.as_deref().is_some_and(|s| !s.trim().is_empty()) {
        None
    } else {
        Some((
            RegexMatcher::new_line_matcher(&args.pattern)
                .with_context(|| format!("invalid regex: {}", args.pattern))?,
            Regex::new(&args.pattern)
                .with_context(|| format!("invalid regex: {}", args.pattern))?,
        ))
    };
    let fuzzy = args
        .fuzzy
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(parse_fuzzy_distance)
        .transpose()?;
    let exclude = build_exclude_set(args.exclude.as_ref())?;
    let target = resolve_existing_path(&ctx.root, &args.path)?;
    let mut matches = Vec::new();
    let mut errors = Vec::new();
    for path in walk_files(&ctx.root, &target, &exclude)? {
        let outcome = match (regex.as_ref(), fuzzy) {
            (Some((matcher, column_regex)), None) => {
                search_file(&ctx.root, &path, matcher, column_regex, args.enhance_match)
            }
            (None, Some(distance)) => search_file_fuzzy(&ctx.root, &path, &args.pattern, distance),
            _ => Ok(Vec::new()),
        };
        match outcome {
            Ok(mut found) => matches.append(&mut found),
            Err(err) => {
                errors.push(json!({"path": rel_path(&ctx.root, &path), "message": err.to_string()}))
            }
        }
    }
    let shown = args.limit.max(1);
    Ok(json!({
        "pattern": args.pattern,
        "path": args.path,
        "match_count": matches.len(),
        "matches": matches.iter().take(shown).cloned().collect::<Vec<_>>(),
        "truncated": matches.len() > shown,
        "best_match": args.best_match,
        "enhance_match": args.enhance_match,
        "exclude": args.exclude.as_ref().map(ExcludeArg::patterns),
        "errors": if errors.is_empty() { Value::Null } else { Value::Array(errors) }
    }))
}

fn tool_replace(ctx: &ToolContext, args: ReplaceArgs) -> Result<Value> {
    require_mutation_approval(ctx, "replace")?;
    let regex =
        Regex::new(&args.pattern).with_context(|| format!("invalid regex: {}", args.pattern))?;
    let exclude = build_exclude_set(args.exclude.as_ref())?;
    let target = resolve_existing_path(&ctx.root, &args.path)?;
    let mut changed_files = Vec::new();
    let mut skipped = Vec::new();
    let mut errors = Vec::new();
    let mut replacement_count = 0usize;
    for path in walk_files(&ctx.root, &target, &exclude)? {
        match replace_file(&path, &regex, &args.replacement) {
            Ok(ReplaceOutcome::Changed(count)) => {
                changed_files
                    .push(json!({"path": rel_path(&ctx.root, &path), "replacements": count}));
                replacement_count += count;
            }
            Ok(ReplaceOutcome::Unchanged) => {}
            Ok(ReplaceOutcome::Skipped(reason)) => {
                skipped.push(json!({"path": rel_path(&ctx.root, &path), "reason": reason}))
            }
            Err(err) => {
                errors.push(json!({"path": rel_path(&ctx.root, &path), "message": err.to_string()}))
            }
        }
    }
    let shown = args.limit.max(1);
    Ok(json!({
        "pattern": args.pattern,
        "replacement": args.replacement,
        "path": args.path,
        "changed_file_count": changed_files.len(),
        "replacement_count": replacement_count,
        "changed_files": changed_files.iter().take(shown).cloned().collect::<Vec<_>>(),
        "truncated": changed_files.len() > shown,
        "exclude": args.exclude.as_ref().map(ExcludeArg::patterns),
        "skipped": skipped,
        "errors": errors
    }))
}

fn tool_sloc(ctx: &ToolContext, args: SlocArgs) -> Result<Value> {
    let target = resolve_existing_path(&ctx.root, &args.path)?;
    let excludes = args
        .exclude
        .as_ref()
        .map(ExcludeArg::patterns)
        .unwrap_or_default();
    let exclude_refs = excludes.iter().map(String::as_str).collect::<Vec<_>>();
    let target_str = target.to_string_lossy().to_string();
    let targets = [target_str.as_str()];
    let mut languages = Languages::new();
    languages.get_statistics(&targets, &exclude_refs, &TokeiConfig::default());

    let mut total_files = 0usize;
    let mut total_code = 0usize;
    let mut total_comments = 0usize;
    let mut total_blanks = 0usize;
    let mut top_files = Vec::new();
    let mut language_rows = Vec::new();

    for (language_type, language) in &languages {
        let file_count = language.reports.len();
        total_files += file_count;
        total_code += language.code;
        total_comments += language.comments;
        total_blanks += language.blanks;
        language_rows.push(json!({
            "language": language_type.to_string(),
            "file_count": file_count,
            "code_count": language.code,
            "documentation_count": language.comments,
            "empty_count": language.blanks,
            "string_count": 0
        }));
        for report in &language.reports {
            top_files.push(json!({
                "path": rel_path(&ctx.root, &report.name),
                "language": language_type.to_string(),
                "code_count": report.stats.code,
                "documentation_count": report.stats.comments,
                "empty_count": report.stats.blanks,
                "string_count": 0,
                "line_count": report.stats.lines()
            }));
        }
    }

    language_rows.sort_by(|a, b| {
        b.get("code_count")
            .and_then(Value::as_u64)
            .cmp(&a.get("code_count").and_then(Value::as_u64))
    });
    top_files.sort_by(|a, b| {
        b.get("code_count")
            .and_then(Value::as_u64)
            .cmp(&a.get("code_count").and_then(Value::as_u64))
    });
    let shown = args.limit.max(1);
    let total_lines = total_code + total_comments + total_blanks;
    Ok(json!({
        "path": args.path,
        "total_file_count": total_files,
        "total_code_count": total_code,
        "total_documentation_count": total_comments,
        "total_empty_count": total_blanks,
        "total_string_count": 0,
        "total_line_count": total_lines,
        "language_count": language_rows.len(),
        "languages": language_rows.iter().take(shown).cloned().collect::<Vec<_>>(),
        "top_file_count": top_files.len(),
        "top_files": top_files.iter().take(20).cloned().collect::<Vec<_>>(),
        "truncated": language_rows.len() > shown || top_files.len() > 20,
        "exclude": if excludes.is_empty() { Value::Null } else { serde_json::to_value(excludes)? }
    }))
}

async fn tool_bash(ctx: &ToolContext, args: BashArgs) -> Result<Value> {
    require_mutation_approval(ctx, "bash")?;
    if args.command.as_bytes().len() > config::max_bash_cmd_bytes() {
        bail!(
            "command too large ({} bytes)",
            args.command.as_bytes().len()
        );
    }
    eprintln!("tool: bash command: {}", args.command);
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(&args.command)
        .current_dir(&ctx.root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = timeout(Duration::from_secs(args.timeout_seconds), cmd.output()).await??;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let (stdout, stdout_truncated) = summarize_text(&stdout, 6000);
    let (stderr, stderr_truncated) = summarize_text(&stderr, 4000);
    Ok(json!({
        "command": args.command,
        "returncode": output.status.code().unwrap_or(-1),
        "stdout": stdout,
        "stderr": stderr,
        "stdout_truncated": stdout_truncated,
        "stderr_truncated": stderr_truncated
    }))
}

async fn tool_webfetch(ctx: &ToolContext, args: WebfetchArgs) -> Result<Value> {
    let _ = ctx;
    let method = args.method.to_ascii_uppercase();
    if !matches!(method.as_str(), "GET" | "HEAD" | "OPTIONS") {
        bail!("Only GET/HEAD/OPTIONS are allowed, got {method}");
    }
    let url = validate_public_url(&args.url).await?;
    let mut builder = Request::builder()
        .method(method.as_str())
        .uri(url.as_str())
        .timeout(Duration::from_secs(args.timeout_seconds))
        .redirect_policy(if args.follow_redirects {
            isahc::config::RedirectPolicy::Limit(10)
        } else {
            isahc::config::RedirectPolicy::None
        });
    if let Some(headers) = args.headers.as_ref() {
        for (key, value) in headers {
            let lower = key.to_ascii_lowercase();
            if matches!(
                lower.as_str(),
                "authorization"
                    | "cookie"
                    | "host"
                    | "proxy-authorization"
                    | "x-forwarded-for"
                    | "x-real-ip"
            ) {
                bail!("Header {key:?} is not allowed in webfetch requests");
            }
            if value.contains('\r') || value.contains('\n') {
                bail!("Header value for {key:?} contains invalid CRLF characters");
            }
            builder = builder.header(key, value);
        }
    }
    let request = builder.body(())?;
    let mut response = isahc::send_async(request).await?;
    let status = response.status();
    let headers = response.headers().clone();
    let final_url = response
        .effective_uri()
        .map(ToString::to_string)
        .unwrap_or_else(|| url.to_string());
    let content_type = headers
        .get(isahc::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let header_map = headers
        .iter()
        .map(|(k, v)| {
            let key = k.as_str().to_string();
            let value = if matches!(
                key.to_ascii_lowercase().as_str(),
                "set-cookie" | "www-authenticate" | "proxy-authenticate" | "location"
            ) {
                "<redacted>".to_string()
            } else {
                v.to_str().unwrap_or("").to_string()
            };
            (key, Value::String(value))
        })
        .collect::<Map<String, Value>>();

    if is_text_content_type(&content_type) {
        let text = response.text().await?;
        let normalized = if content_type.contains("text/html")
            || text.trim_start().starts_with("<!DOCTYPE html")
            || text.trim_start().starts_with("<html")
        {
            html2md::parse_html(&text)
        } else {
            text
        };
        let (text, truncated) = summarize_text(&normalized, 12000);
        return Ok(json!({
            "method": method,
            "url": final_url,
            "status_code": status.as_u16(),
            "reason_phrase": status.canonical_reason().unwrap_or(""),
            "http_version": format!("{:?}", response.version()),
            "headers": header_map,
            "text": text,
            "format": if content_type.contains("html") { "markdown" } else { "text" },
            "truncated": truncated
        }));
    }

    let bytes = response.bytes().await?;
    Ok(json!({
        "method": method,
        "url": final_url,
        "status_code": status.as_u16(),
        "reason_phrase": status.canonical_reason().unwrap_or(""),
        "http_version": format!("{:?}", response.version()),
        "headers": header_map,
        "binary": true,
        "content_bytes": bytes.len()
    }))
}

fn tool_ask(ctx: &ToolContext, args: AskArgs) -> Result<Value> {
    if !ctx.interactive {
        bail!("Cannot ask: interactive prompting is unavailable");
    }
    Ok(Value::String(crate::ui::ask(
        &args.question,
        args.choices.as_deref(),
    )?))
}

fn tool_todo(ctx: &mut ToolContext, args: TodoArgs) -> Result<Value> {
    for item in &args.todos {
        if !matches!(item.status.as_str(), "pending" | "in_progress" | "done") {
            bail!("invalid todo status: {}", item.status);
        }
    }
    ctx.todos = args.todos;
    Ok(json!({"items": ctx.todos, "count": ctx.todos.len()}))
}

fn validate_pattern(pattern: &str) -> Result<()> {
    let path = Path::new(pattern);
    if path.is_absolute() {
        bail!("Path traversal denied: '{pattern}'");
    }
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        bail!("Path traversal denied: '{pattern}'");
    }
    Ok(())
}

fn resolve_existing_path(root: &Path, path: &str) -> Result<PathBuf> {
    validate_pattern(path)?;
    let joined = root.join(path);
    let resolved = joined
        .canonicalize()
        .with_context(|| format!("path does not exist: {path}"))?;
    if !within_root(root, &resolved) {
        bail!("Path traversal denied: '{path}'");
    }
    Ok(resolved)
}

fn build_exclude_set(exclude: Option<&ExcludeArg>) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    if let Some(exclude) = exclude {
        for pattern in exclude.patterns() {
            builder.add(
                Glob::new(&pattern).with_context(|| format!("invalid exclude glob: {pattern}"))?,
            );
        }
    }
    Ok(builder.build()?)
}

fn rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"))
}

fn display_path(root: &Path, path: &Path) -> String {
    let mut value = rel_path(root, path);
    if path.is_dir() && !value.ends_with('/') {
        value.push('/');
    }
    value
}

fn within_root(root: &Path, path: &Path) -> bool {
    path == root || path.starts_with(root)
}

fn walk_files(root: &Path, target: &Path, exclude: &GlobSet) -> Result<Vec<PathBuf>> {
    if target.is_file() {
        return Ok(vec![target.to_path_buf()]);
    }
    let mut out = Vec::new();
    let mut builder = WalkBuilder::new(target);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true);
    for entry in builder.build() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => return Err(anyhow!(err)),
        };
        let path = entry.path();
        if entry.file_type().is_some_and(|ft| ft.is_file()) {
            let rel = rel_path(root, path);
            if exclude.is_match(rel.as_str()) {
                continue;
            }
            out.push(path.to_path_buf());
        }
    }
    Ok(out)
}

struct SearchText {
    display_path: String,
    text: String,
}

fn file_texts(root: &Path, path: &Path) -> Result<Vec<SearchText>> {
    let rel = rel_path(root, path);
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    if path.extension().and_then(|s| s.to_str()) == Some("zip") {
        return zip_texts(path, &rel);
    }
    if name.ends_with(".tar") || name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        return tar_texts(path, &rel);
    }
    let raw = fs::read(path)?;
    if raw.contains(&0) {
        return Ok(Vec::new());
    }
    let bytes = decode_compressed(path, raw)?;
    Ok(String::from_utf8(bytes)
        .ok()
        .map(|text| {
            vec![SearchText {
                display_path: rel,
                text,
            }]
        })
        .unwrap_or_default())
}

fn decode_compressed(path: &Path, raw: Vec<u8>) -> Result<Vec<u8>> {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    if name.ends_with(".gz") && !name.ends_with(".tar.gz") {
        let mut out = Vec::new();
        GzDecoder::new(Cursor::new(raw)).read_to_end(&mut out)?;
        return Ok(out);
    }
    if name.ends_with(".bz2") {
        let mut out = Vec::new();
        BzDecoder::new(Cursor::new(raw)).read_to_end(&mut out)?;
        return Ok(out);
    }
    if name.ends_with(".xz") {
        let mut out = Vec::new();
        XzDecoder::new(Cursor::new(raw)).read_to_end(&mut out)?;
        return Ok(out);
    }
    if name.ends_with(".zst") {
        return zstd::decode_all(Cursor::new(raw)).map_err(Into::into);
    }
    Ok(raw)
}

fn zip_texts(path: &Path, rel: &str) -> Result<Vec<SearchText>> {
    let file = fs::File::open(path)?;
    let mut archive = ZipArchive::new(file)?;
    let mut out = Vec::new();
    for index in 0..archive.len() {
        let mut member = archive.by_index(index)?;
        if member.is_dir() {
            continue;
        }
        let mut bytes = Vec::new();
        member.read_to_end(&mut bytes)?;
        if bytes.contains(&0) {
            continue;
        }
        if let Ok(text) = String::from_utf8(bytes) {
            out.push(SearchText {
                display_path: format!("{rel}::{}", member.name()),
                text,
            });
        }
    }
    Ok(out)
}

fn tar_texts(path: &Path, rel: &str) -> Result<Vec<SearchText>> {
    let file = fs::File::open(path)?;
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let reader: Box<dyn Read> = if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Box::new(GzDecoder::new(file))
    } else {
        Box::new(file)
    };
    let mut archive = tar::Archive::new(reader);
    let mut out = Vec::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let member = entry.path()?.to_string_lossy().replace('\\', "/");
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        if bytes.contains(&0) {
            continue;
        }
        if let Ok(text) = String::from_utf8(bytes) {
            out.push(SearchText {
                display_path: format!("{rel}::{member}"),
                text,
            });
        }
    }
    Ok(out)
}

fn push_match(
    display_path: &str,
    text: &str,
    line_number: usize,
    line: &str,
    column: usize,
    enhance_match: bool,
    out: &mut Vec<Value>,
) {
    let mut item = json!({
        "path": display_path,
        "line_number": line_number,
        "column": column,
        "text": truncate_long_line(line.trim_end_matches(['\r', '\n']))
    });
    if enhance_match {
        if let Some(context) = syntax_context(display_path, text, line_number) {
            item["context"] = Value::String(context);
        }
    }
    out.push(item);
}

fn search_text_grep(
    display_path: &str,
    text: &str,
    matcher: &RegexMatcher,
    column_regex: &Regex,
    enhance_match: bool,
    out: &mut Vec<Value>,
) -> Result<()> {
    let mut searcher = SearcherBuilder::new().line_number(true).build();
    let mut sink = UTF8(|line_number, line: &str| {
        let column = column_regex.find(line).map(|m| m.start() + 1).unwrap_or(1);
        push_match(
            display_path,
            text,
            line_number as usize,
            line,
            column,
            enhance_match,
            out,
        );
        Ok(true)
    });
    searcher.search_reader(matcher, text.as_bytes(), &mut sink)?;
    Ok(())
}

fn search_text_fuzzy(
    display_path: &str,
    text: &str,
    pattern: &str,
    max_distance: usize,
    out: &mut Vec<Value>,
) {
    for (index, line) in text.lines().enumerate() {
        if let Some((column, _distance)) = fuzzy_find_column(line, pattern, max_distance) {
            out.push(json!({
                "path": display_path,
                "line_number": index + 1,
                "column": column,
                "text": truncate_long_line(line)
            }));
        }
    }
}

fn syntax_context(display_path: &str, text: &str, line_number: usize) -> Option<String> {
    if !display_path.ends_with(".rs") {
        return None;
    }
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .ok()?;
    let tree = parser.parse(text, None)?;
    let target_row = line_number.saturating_sub(1);
    let mut cursor = tree.walk();
    let mut stack = vec![tree.root_node()];
    let mut best = None;
    while let Some(node) = stack.pop() {
        let range = node.range();
        if range.start_point.row <= target_row && target_row <= range.end_point.row {
            if matches!(
                node.kind(),
                "function_item"
                    | "impl_item"
                    | "struct_item"
                    | "enum_item"
                    | "trait_item"
                    | "mod_item"
                    | "macro_definition"
            ) {
                let line = text
                    .lines()
                    .nth(range.start_point.row)
                    .unwrap_or_default()
                    .trim();
                best = Some(format!(
                    "{} at lines {}-{}: {}",
                    node.kind(),
                    range.start_point.row + 1,
                    range.end_point.row + 1,
                    truncate_long_line(line)
                ));
            }
            for child in node.children(&mut cursor) {
                stack.push(child);
            }
        }
    }
    best
}

fn search_file(
    root: &Path,
    path: &Path,
    matcher: &RegexMatcher,
    column_regex: &Regex,
    enhance_match: bool,
) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for item in file_texts(root, path)? {
        search_text_grep(
            &item.display_path,
            &item.text,
            matcher,
            column_regex,
            enhance_match,
            &mut out,
        )?;
    }
    Ok(out)
}

fn search_file_fuzzy(
    root: &Path,
    path: &Path,
    pattern: &str,
    max_distance: usize,
) -> Result<Vec<Value>> {
    if pattern.is_empty() {
        bail!("fuzzy search pattern must not be empty");
    }
    let mut out = Vec::new();
    for item in file_texts(root, path)? {
        search_text_fuzzy(
            &item.display_path,
            &item.text,
            pattern,
            max_distance,
            &mut out,
        );
    }
    Ok(out)
}

fn parse_fuzzy_distance(value: &str) -> Result<usize> {
    let trimmed = value
        .trim()
        .trim_start_matches('{')
        .trim_end_matches('}')
        .trim();
    let digits = trimmed
        .chars()
        .filter(|ch| ch.is_ascii_digit())
        .collect::<String>();
    if digits.is_empty() {
        bail!("fuzzy must contain a max edit distance, e.g. `e<=1`");
    }
    Ok(digits.parse::<usize>()?)
}

fn fuzzy_find_column(line: &str, pattern: &str, max_distance: usize) -> Option<(usize, usize)> {
    let line_chars = line.chars().collect::<Vec<_>>();
    let pattern_chars = pattern.chars().collect::<Vec<_>>();
    let target_len = pattern_chars.len();
    if target_len == 0 {
        return Some((1, 0));
    }
    let min_len = target_len.saturating_sub(max_distance).max(1);
    let max_len = target_len + max_distance;
    let mut best: Option<(usize, usize)> = None;
    for start in 0..line_chars.len() {
        for length in min_len..=max_len.min(line_chars.len() - start) {
            let candidate = line_chars[start..start + length].iter().collect::<String>();
            let distance = levenshtein(candidate.as_str(), pattern);
            if distance <= max_distance {
                let column = start + 1;
                match best {
                    Some((best_col, best_dist))
                        if distance > best_dist
                            || (distance == best_dist && column >= best_col) => {}
                    _ => best = Some((column, distance)),
                }
            }
        }
    }
    best
}

enum ReplaceOutcome {
    Changed(usize),
    Unchanged,
    Skipped(&'static str),
}

fn replace_file(path: &Path, regex: &Regex, replacement: &str) -> Result<ReplaceOutcome> {
    if path.is_symlink() {
        return Ok(ReplaceOutcome::Skipped("symlink"));
    }
    let raw = fs::read(path)?;
    if raw.contains(&0) {
        return Ok(ReplaceOutcome::Skipped("binary file"));
    }
    let text = String::from_utf8(raw).map_err(|_| anyhow!("cannot decode utf-8"))?;
    let count = regex.find_iter(&text).count();
    if count == 0 {
        return Ok(ReplaceOutcome::Unchanged);
    }
    let updated = regex.replace_all(&text, replacement).into_owned();
    fs::write(path, updated)?;
    Ok(ReplaceOutcome::Changed(count))
}

fn truncate_long_line(text: &str) -> String {
    const MAX: usize = 1000;
    if text.len() <= MAX {
        return text.to_string();
    }
    format!("{}... [truncated {} chars]", &text[..MAX], text.len() - MAX)
}

fn summarize_text(text: &str, max_chars: usize) -> (String, bool) {
    if text.len() <= max_chars {
        return (text.to_string(), false);
    }
    let head_len = max_chars / 2;
    let tail_len = max_chars.saturating_sub(head_len);
    let head = &text[..head_len.min(text.len())];
    let tail = &text[text.len().saturating_sub(tail_len)..];
    (
        format!(
            "{head}\n... [truncated {} chars] ...\n{tail}",
            text.len() - head.len() - tail.len()
        ),
        true,
    )
}

async fn validate_public_url(input: &str) -> Result<Url> {
    let url = Url::parse(input).with_context(|| format!("invalid URL: {input}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("Only http/https URLs are allowed, got {:?}", url.scheme());
    }
    let host = url.host_str().context("missing hostname")?;
    let lower = host.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "localhost" | "localhost.localdomain" | "ip6-localhost" | "ip6-loopback"
    ) {
        bail!("Local addresses are not allowed: {host}");
    }
    let port = url.port_or_known_default().unwrap_or(80);
    let addrs = lookup_host((host, port)).await?;
    for addr in addrs {
        let ip = addr.ip();
        if !is_public_ip(ip) {
            bail!("URL resolves to non-public address ({ip})");
        }
    }
    Ok(url)
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.is_unspecified())
        }
        IpAddr::V6(ip) => {
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local())
        }
    }
}

fn is_text_content_type(content_type: &str) -> bool {
    content_type.starts_with("text/")
        || content_type.contains("json")
        || content_type.contains("xml")
        || content_type.contains("javascript")
        || content_type.contains("svg")
        || content_type.is_empty()
}

fn require_mutation_approval(ctx: &ToolContext, tool: &str) -> Result<()> {
    if auto_approved(ctx, tool) {
        return Ok(());
    }
    if !ctx.interactive {
        return Ok(());
    }
    eprintln!("approve {}? [y/N]", tool);
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let answer = line.trim().to_ascii_lowercase();
    if matches!(answer.as_str(), "y" | "yes") {
        Ok(())
    } else {
        bail!("tool denied by user")
    }
}

fn auto_approved(ctx: &ToolContext, tool: &str) -> bool {
    ctx.yolo || ctx.agent == "auto-approve" || (ctx.agent == "accept-edits" && tool == "replace")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_distance_parser_accepts_common_forms() {
        assert_eq!(parse_fuzzy_distance("1").unwrap(), 1);
        assert_eq!(parse_fuzzy_distance("{e<=2}").unwrap(), 2);
    }

    #[test]
    fn fuzzy_find_column_finds_near_match() {
        let found = fuzzy_find_column("hello wurld", "world", 1);
        assert_eq!(found, Some((7, 1)));
    }

    #[test]
    fn search_file_reads_zip_members() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("sample.zip");
        {
            let file = fs::File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("src/lib.rs", options).unwrap();
            std::io::Write::write_all(
                &mut zip,
                b"fn archive_hit() {}
",
            )
            .unwrap();
            zip.finish().unwrap();
        }
        let matcher = RegexMatcher::new_line_matcher("archive_hit").unwrap();
        let column_regex = Regex::new("archive_hit").unwrap();
        let found = search_file(dir.path(), &zip_path, &matcher, &column_regex, false).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0]["path"], "sample.zip::src/lib.rs");
    }

    #[test]
    fn enhanced_search_adds_rust_syntax_context() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lib.rs");
        fs::write(
            &path,
            "fn outer() {
    let needle = 1;
}
",
        )
        .unwrap();
        let matcher = RegexMatcher::new_line_matcher("needle").unwrap();
        let column_regex = Regex::new("needle").unwrap();
        let found = search_file(dir.path(), &path, &matcher, &column_regex, true).unwrap();
        assert_eq!(found.len(), 1);
        assert!(
            found[0]["context"]
                .as_str()
                .unwrap()
                .contains("function_item")
        );
    }
}
