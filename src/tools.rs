use anyhow::{Context, Result, anyhow, bail};
use flate2::read::GzDecoder;
use glob::glob;
use globset::{Glob, GlobSet, GlobSetBuilder};
use grep_regex::RegexMatcher;
use grep_searcher::SearcherBuilder;
use grep_searcher::sinks::UTF8;
use ignore::WalkBuilder;
use regex::Regex;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::{Cursor, Read};
use std::net::IpAddr;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokei::{
    Config as TokeiConfig, Language as TokeiLanguage, Languages as TokeiLanguages,
    Sort as TokeiSort,
};
use tokio::net::lookup_host;
use tokio::process::Command;
use tokio::time::timeout;
use toon_format::encode_default;
use url::Url;
use zip::ZipArchive;

use genai::chat::Tool;

use crate::config;

pub const DEFAULT_LIMIT: usize = 910;
pub const DEFAULT_WEBFETCH_TIMEOUT_SECONDS: u64 = 60;
const TODO_FILE: &str = "TODO.md";
const PREVIEW_ITEMS: usize = 20;
const PREVIEW_LINES: usize = 30;
const PREVIEW_LINE_CHARS: usize = 180;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub task: String,
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
struct TodoItemInput {
    #[serde(default)]
    id: Option<String>,
    task: String,
    #[serde(default = "default_todo_status")]
    status: String,
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
    #[serde(default, alias = "items")]
    todos: Vec<TodoItemInput>,
    #[serde(default)]
    persist: bool,
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
fn default_todo_status() -> String {
    "pending".to_string()
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
                            "description": "Complete replacement todo list. Alias: items. Omit to return current list.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": {"type": "string", "description": "Stable short id; optional, defaults to 1-based position."},
                                    "task": {"type": "string"},
                                    "status": {"type": "string", "enum": ["pending", "in_progress", "done"], "default": "pending"}
                                },
                                "required": ["task"],
                                "additionalProperties": false
                            }
                        },
                        "items": {
                            "type": "array",
                            "description": "Alias for todos.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": {"type": "string", "description": "Stable short id; optional, defaults to 1-based position."},
                                    "task": {"type": "string"},
                                    "status": {"type": "string", "enum": ["pending", "in_progress", "done"], "default": "pending"}
                                },
                                "required": ["task"],
                                "additionalProperties": false
                            }
                        },
                        "persist": {"type": "boolean", "default": false, "description": "Write to TODO.md; default false avoids git churn. If TODO.md exists, read it first and pass a merged list so still-relevant existing items are preserved."}
                    },
                    "additionalProperties": false
                }))
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
    note_tool(name, &args);
    let result = match name {
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
    };
    if let Ok(value) = &result {
        let preview = preview_tool_output(name, value);
        if !preview.trim().is_empty() {
            crate::highlight::stderr(&(preview.trim_end().to_string() + "\n"));
        }
    } else if let Err(err) = &result {
        crate::highlight::stderr(&format!("✗ {name}: {err:#}\n"));
    }
    result
}

pub fn encode_tool_output(value: &Value) -> String {
    encode_default(value).unwrap_or_else(|_| {
        serde_json::to_string_pretty(value).unwrap_or_else(|_| String::from("{}"))
    })
}

fn note_tool(name: &str, args: &Value) {
    let detail = tool_call_summary(name, args);
    if detail.is_empty() {
        crate::highlight::stderr(&format!("→ {name}\n"));
    } else {
        crate::highlight::stderr(&format!("→ {name} {detail}\n"));
    }
}

fn tool_call_summary(name: &str, args: &Value) -> String {
    match name {
        "list" => compact_kvs(args, &[("path", 60), ("exclude", 40)]),
        "read" => compact_kvs(args, &[("path", 70), ("offset", 12), ("limit", 12)]),
        "search" => compact_kvs(args, &[("pattern", 70), ("path", 50), ("exclude", 35)]),
        "replace" => compact_kvs(args, &[("path", 45), ("pattern", 45), ("replacement", 45)]),
        "sloc" => compact_kvs(args, &[("path", 70), ("exclude", 40)]),
        "bash" => preview_value(args.get("command").unwrap_or(&Value::Null), 100),
        "webfetch" => compact_kvs(args, &[("method", 8), ("url", 100)]),
        "ask" => preview_value(args.get("question").unwrap_or(&Value::Null), 100),
        "todo" => todo_call_summary(args),
        _ => preview_value(args, 120),
    }
}

fn compact_kvs(args: &Value, keys: &[(&str, usize)]) -> String {
    keys.iter()
        .filter_map(|(key, max)| {
            let value = args.get(*key)?;
            if value.is_null() || value == false || value == "" {
                return None;
            }
            if *key == "limit" && value.as_u64() == Some(DEFAULT_LIMIT as u64) {
                return None;
            }
            Some(format!(
                "{}={}",
                key.replace('_', "-"),
                preview_value(value, *max)
            ))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn todo_call_summary(args: &Value) -> String {
    let items = args
        .get("todos")
        .or_else(|| args.get("items"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if items.is_empty() {
        return "0 items".to_string();
    }
    let first = items
        .first()
        .map(|item| preview_value(item.get("task").unwrap_or(item), 56))
        .unwrap_or_default();
    if items.len() == 1 {
        format!("1 item · {first}")
    } else {
        format!("{} items · {first}", items.len())
    }
}

pub fn preview_tool_output(name: &str, value: &Value) -> String {
    match name {
        "list" => preview_list(value),
        "read" => preview_read(value),
        "search" => preview_search(value),
        "replace" => preview_replace(value),
        "bash" => preview_bash(value),
        "webfetch" => preview_webfetch(value),
        "sloc" => preview_sloc(value),
        "ask" => preview_ask(value),
        "todo" => preview_todo(value),
        _ => preview_generic(value),
    }
}

fn preview_value(value: &Value, max: usize) -> String {
    let raw = value
        .as_str()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| value.to_string());
    crate::text::compact_preview(&raw, max)
}

fn value_str<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

fn value_usize(value: &Value, key: &str) -> usize {
    value.get(key).and_then(Value::as_u64).unwrap_or(0) as usize
}

fn value_bool(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn append_preview_lines(out: &mut String, text: &str, indent: &str) {
    let line_count = text.lines().count();
    for line in text.lines().take(PREVIEW_LINES) {
        let _ = write!(
            out,
            "\n{indent}{}",
            crate::text::truncate_chars(line, PREVIEW_LINE_CHARS)
        );
    }
    if line_count > PREVIEW_LINES {
        let _ = write!(
            out,
            "\n{indent}… {} more preview lines",
            line_count - PREVIEW_LINES
        );
    }
}

fn preview_generic(value: &Value) -> String {
    crate::text::clamp_lines(
        &encode_tool_output(value),
        PREVIEW_LINES,
        PREVIEW_LINE_CHARS,
    )
}

fn preview_list(value: &Value) -> String {
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let total = value_usize(value, "count");
    let mut out = format!(
        "{} item{} in {}",
        total,
        plural(total),
        value_str(value, "path")
    );
    for item in items.iter().take(PREVIEW_ITEMS) {
        let _ = write!(
            out,
            "\n  {}",
            crate::text::truncate_chars(item.as_str().unwrap_or(""), PREVIEW_LINE_CHARS)
        );
    }
    let shown = items.len().min(PREVIEW_ITEMS);
    if total > shown || value_bool(value, "truncated") {
        let remaining = total.saturating_sub(shown);
        let _ = write!(out, "\n  … {remaining} more item{}", plural(remaining));
    }
    out
}

fn preview_read(value: &Value) -> String {
    let path = value_str(value, "path");
    let offset = value_usize(value, "offset");
    let line_count = value_usize(value, "line_count");
    let text = value_str(value, "text");
    let shown = text.lines().count();
    let end = offset.saturating_add(shown).saturating_sub(1);
    let mut out = format!("{path}:{offset}-{end} ({line_count} lines)");
    if text.is_empty() {
        out.push_str("\n  <empty>");
    } else {
        append_preview_lines(&mut out, text, "  ");
    }
    if value_bool(value, "truncated") {
        let hidden = line_count.saturating_sub(end);
        let _ = write!(
            out,
            "\n  … read truncated: {hidden} more line{} available",
            plural(hidden)
        );
    }
    out
}

fn preview_search(value: &Value) -> String {
    let matches = value
        .get("matches")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let total = value_usize(value, "match_count");
    if matches.is_empty() {
        return format!("0 matches for /{}/", value_str(value, "pattern"));
    }
    let mut out = format!(
        "{} match{} for /{}/",
        total,
        plural(total),
        value_str(value, "pattern")
    );
    for item in matches.iter().take(PREVIEW_ITEMS) {
        let path = value_str(item, "path");
        let line = value_usize(item, "line_number");
        let col = value_usize(item, "column");
        let text = crate::text::truncate_chars(value_str(item, "text"), PREVIEW_LINE_CHARS);
        let _ = write!(out, "\n  {path}:{line}:{col}: {text}");
    }
    if value
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let _ = write!(
            out,
            "\n  … {} more matches",
            total.saturating_sub(matches.len().min(PREVIEW_ITEMS))
        );
    }
    out
}

fn preview_replace(value: &Value) -> String {
    let changed = value
        .get("changed_files")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let replacements = value_usize(value, "replacement_count");
    let mut out = format!(
        "{} file{} changed · {} replacement{}",
        changed.len(),
        plural(changed.len()),
        replacements,
        plural(replacements)
    );
    if changed.is_empty() {
        out.push_str("\n  <no changes>");
    } else {
        for item in changed.iter().take(PREVIEW_ITEMS) {
            let _ = write!(
                out,
                "\n  {} · {} repl",
                value_str(item, "path"),
                value_usize(item, "replacements")
            );
        }
        if changed.len() > PREVIEW_ITEMS {
            let _ = write!(out, "\n  … {} more files", changed.len() - PREVIEW_ITEMS);
        }
    }
    out
}

fn preview_bash(value: &Value) -> String {
    let code = value
        .get("returncode")
        .and_then(Value::as_i64)
        .unwrap_or(-1);
    let mut out = format!("exit {code}");
    for key in ["stdout", "stderr"] {
        let text = value_str(value, key);
        let truncated_key = format!("{key}_truncated");
        let truncated = value_bool(value, &truncated_key);
        if text.is_empty() {
            if truncated {
                let _ = write!(out, "\n{key}:\n  … {key} truncated");
            }
            continue;
        }
        let _ = write!(out, "\n{key}:");
        append_preview_lines(&mut out, text, "  ");
        if truncated {
            let _ = write!(out, "\n  … {key} truncated for model context");
        }
    }
    out
}

fn preview_webfetch(value: &Value) -> String {
    let status = value
        .get("status_code")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let url = value_str(value, "url");
    if value
        .get("binary")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return format!(
            "HTTP {status} {url}\n  binary · {} bytes",
            value_usize(value, "content_bytes")
        );
    }
    let mut out = format!("HTTP {status} {url}");
    let text = value_str(value, "text");
    if !text.is_empty() {
        let preview = crate::text::clamp_lines(text, PREVIEW_LINES, PREVIEW_LINE_CHARS);
        for line in preview.lines() {
            let _ = write!(out, "\n  {line}");
        }
    }
    if value_bool(value, "truncated") {
        out.push_str("\n  … response body truncated for model context");
    }
    out
}

fn preview_sloc(value: &Value) -> String {
    let total = value
        .pointer("/output/Total/code")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let comments = value
        .pointer("/output/Total/comments")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let blanks = value
        .pointer("/output/Total/blanks")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mut langs = value
        .get("output")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter(|(name, _)| name.as_str() != "Total")
                .filter_map(|(name, stats)| {
                    stats
                        .get("code")
                        .and_then(Value::as_u64)
                        .map(|code| (name, code))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    langs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    let mut out = format!(
        "{}: {total} code · {comments} comments · {blanks} blank",
        value_str(value, "path")
    );
    for (name, code) in langs.into_iter().take(PREVIEW_ITEMS) {
        let _ = write!(out, "\n  {name}: {code}");
    }
    out
}

fn preview_ask(value: &Value) -> String {
    let answer = value.as_str().unwrap_or_default();
    if answer.is_empty() {
        "<no selection>".to_string()
    } else {
        format!(
            "selected: {}",
            crate::text::truncate_chars(answer, PREVIEW_LINE_CHARS)
        )
    }
}

fn preview_todo(value: &Value) -> String {
    value_str(value, "preview").to_string().if_empty_then(|| {
        let items = value
            .get("items")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        format_todo_preview_from_values(items)
    })
}
fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

pub fn format_todos(todos: &[TodoItem]) -> String {
    if todos.is_empty() {
        return "<empty todo list>".to_string();
    }
    todos
        .iter()
        .map(format_todo_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_todo_preview(todos: &[TodoItem]) -> String {
    let counts = todo_status_counts(todos);
    let active = counts.pending + counts.in_progress;
    let mut out = format!(
        "{} todo{} · {} active · {} done",
        todos.len(),
        plural(todos.len()),
        active,
        counts.done
    );
    let lines = format_todos(todos);
    let total_lines = lines.lines().count();
    for line in lines.lines().take(PREVIEW_ITEMS) {
        let _ = write!(out, "\n  {line}");
    }
    if total_lines > PREVIEW_ITEMS {
        let _ = write!(out, "\n  … {} more todos", total_lines - PREVIEW_ITEMS);
    }
    out
}

fn format_todo_preview_from_values(items: &[Value]) -> String {
    let todos = items
        .iter()
        .map(|item| TodoItem {
            id: value_str(item, "id").to_string(),
            task: value_str(item, "task").to_string(),
            status: value_str(item, "status").to_string(),
        })
        .collect::<Vec<_>>();
    format_todo_preview(&todos)
}

fn format_todo_line(item: &TodoItem) -> String {
    let icon = match item.status.as_str() {
        "done" => "✓",
        "in_progress" => "…",
        _ => "·",
    };
    if item.task.is_empty() {
        format!("{icon} {}", item.id)
    } else {
        format!("{icon} {} {}", item.id, item.task)
    }
}

#[derive(Debug, Clone, Copy)]
struct TodoStatusCounts {
    pending: usize,
    in_progress: usize,
    done: usize,
}

fn todo_status_counts(todos: &[TodoItem]) -> TodoStatusCounts {
    let mut counts = TodoStatusCounts {
        pending: 0,
        in_progress: 0,
        done: 0,
    };
    for item in todos {
        match item.status.as_str() {
            "done" => counts.done += 1,
            "in_progress" => counts.in_progress += 1,
            _ => counts.pending += 1,
        }
    }
    counts
}

trait EmptyStringExt {
    fn if_empty_then<F: FnOnce() -> String>(self, fallback: F) -> String;
}

impl EmptyStringExt for String {
    fn if_empty_then<F: FnOnce() -> String>(self, fallback: F) -> String {
        if self.is_empty() { fallback() } else { self }
    }
}

pub fn save_todos_to_file(root: &Path, todos: &[TodoItem]) -> Result<()> {
    let path = todo_path(root);
    fs::write(&path, todos_to_markdown(todos))
        .with_context(|| format!("failed to write {}", TODO_FILE))
}

fn todo_path(root: &Path) -> PathBuf {
    root.join(TODO_FILE)
}

fn todos_to_markdown(todos: &[TodoItem]) -> String {
    let mut out = String::from(
        "# todo

",
    );
    if todos.is_empty() {
        out.push_str(
            "<!-- empty -->
",
        );
        return out;
    }
    for item in todos {
        let box_mark = match item.status.as_str() {
            "done" => "x",
            "in_progress" => "~",
            _ => " ",
        };
        let _ = writeln!(out, "- [{box_mark}] {}: {}", item.id, item.task);
    }
    out
}

fn tool_list(ctx: &ToolContext, args: ListArgs) -> Result<Value> {
    validate_pattern(&args.path)?;
    let exclude = build_exclude_set(args.exclude.as_ref())?;
    let shown_limit = args.limit.max(1);
    if args.path.contains("::") {
        let items = list_archive_virtual(&ctx.root, &args.path, &exclude)?;
        return Ok(json!({
            "path": args.path,
            "items": items.iter().take(shown_limit).cloned().collect::<Vec<_>>(),
            "count": items.len(),
            "truncated": items.len() > shown_limit,
            "exclude": args.exclude.as_ref().map(ExcludeArg::patterns)
        }));
    }
    let target_for_archive = resolve_existing_path(&ctx.root, &args.path).ok();
    if target_for_archive
        .as_ref()
        .is_some_and(|path| path.is_file() && is_archive_path(path))
    {
        let items = list_archive_virtual(&ctx.root, &format!("{}::", args.path), &exclude)?;
        return Ok(json!({
            "path": args.path,
            "items": items.iter().take(shown_limit).cloned().collect::<Vec<_>>(),
            "count": items.len(),
            "truncated": items.len() > shown_limit,
            "exclude": args.exclude.as_ref().map(ExcludeArg::patterns)
        }));
    }
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
    let (display_path, text) = read_virtual_text(&ctx.root, &args.path)?;
    let mut shown = Vec::new();
    let start = args.offset.saturating_sub(1);
    let stop = start + args.limit.max(1);
    let mut line_count = 0usize;
    for (idx, line) in text.lines().enumerate() {
        line_count = idx + 1;
        if idx < start {
            continue;
        }
        if idx < stop {
            shown.push(line.to_string());
        }
    }
    let truncated = line_count > stop;
    Ok(json!({
        "path": display_path,
        "offset": args.offset,
        "limit": args.limit,
        "text": shown.join("
    "),
        "line_count": line_count,
        "truncated": truncated
    }))
}

fn tool_search(ctx: &ToolContext, args: SearchArgs) -> Result<Value> {
    let matcher = RegexMatcher::new_line_matcher(&args.pattern)
        .with_context(|| format!("invalid regex: {}", args.pattern))?;
    let column_regex =
        Regex::new(&args.pattern).with_context(|| format!("invalid regex: {}", args.pattern))?;
    let exclude = build_exclude_set(args.exclude.as_ref())?;
    let target = resolve_existing_path(&ctx.root, &args.path)?;
    let mut matches = Vec::new();
    let mut errors = Vec::new();
    for path in walk_files(&ctx.root, &target, &exclude)? {
        match search_file(&ctx.root, &path, &matcher, &column_regex) {
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
    let exclude = args
        .exclude
        .as_ref()
        .map(ExcludeArg::patterns)
        .unwrap_or_default();
    let target = target.to_string_lossy().to_string();
    let excluded = exclude.iter().map(String::as_str).collect::<Vec<_>>();

    let config = TokeiConfig {
        hidden: Some(false),
        no_ignore: Some(false),
        no_ignore_parent: Some(false),
        no_ignore_dot: Some(false),
        no_ignore_vcs: Some(false),
        ..TokeiConfig::default()
    };
    let mut languages = TokeiLanguages::new();
    languages.get_statistics(&[target.as_str()], &excluded, &config);
    sort_tokei_reports(&mut languages);

    let mut output = serde_json::to_value(&languages)?;
    if let Value::Object(ref mut map) = output {
        map.insert(
            "Total".to_string(),
            serde_json::to_value(languages.total())?,
        );
    }

    Ok(json!({
        "path": args.path,
        "format": "tokei-json",
        "output": output,
        "exclude": if exclude.is_empty() { Value::Null } else { serde_json::to_value(exclude)? }
    }))
}

fn sort_tokei_reports(languages: &mut TokeiLanguages) {
    for language in languages.values_mut() {
        language.sort_by(TokeiSort::Code);
    }
}

pub fn compact_workspace_snapshot(root: &Path) -> Option<String> {
    let config = TokeiConfig {
        hidden: Some(false),
        no_ignore: Some(false),
        no_ignore_parent: Some(false),
        no_ignore_dot: Some(false),
        no_ignore_vcs: Some(false),
        ..TokeiConfig::default()
    };
    let target = root.to_string_lossy().to_string();
    let mut languages = TokeiLanguages::new();
    languages.get_statistics(&[target.as_str()], &[] as &[&str], &config);
    if languages.is_empty() {
        return None;
    }
    sort_tokei_reports(&mut languages);
    Some(format_workspace_snapshot(root, &languages))
}

fn format_workspace_snapshot(root: &Path, languages: &TokeiLanguages) -> String {
    let total = languages.total();
    let mut language_parts = languages
        .iter()
        .filter(|(_, language)| language.code > 0 || language.comments > 0)
        .map(|(name, language)| (name.to_string(), language.code, language.reports.len()))
        .collect::<Vec<_>>();
    language_parts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let mut largest = Vec::new();
    collect_largest_reports(root, &total, &mut largest);
    largest.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let largest = largest
        .into_iter()
        .take(5)
        .map(|(path, code)| format!("{path} {code}"))
        .collect::<Vec<_>>()
        .join(", ");

    let mut out = format!(
        "Workspace size snapshot: total {} code LOC, {} comment lines, {} blank lines.",
        total.code, total.comments, total.blanks
    );
    if !language_parts.is_empty() {
        let shown = language_parts
            .into_iter()
            .take(8)
            .map(|(name, code, files)| format!("{name}: {code} LOC/{files} files"))
            .collect::<Vec<_>>()
            .join("; ");
        out.push_str(&format!(" Languages: {shown}."));
    }
    if !largest.is_empty() {
        out.push_str(&format!(" Largest files: {largest}."));
    }
    out
}

fn collect_largest_reports(root: &Path, language: &TokeiLanguage, out: &mut Vec<(String, usize)>) {
    for report in &language.reports {
        out.push((rel_path(root, &report.name), report.stats.code));
    }
    for reports in language.children.values() {
        for report in reports {
            out.push((rel_path(root, &report.name), report.stats.code));
        }
    }
}
async fn tool_bash(ctx: &ToolContext, args: BashArgs) -> Result<Value> {
    require_mutation_approval(ctx, "bash")?;
    if args.command.as_bytes().len() > config::max_bash_cmd_bytes() {
        bail!(
            "command too large ({} bytes)",
            args.command.as_bytes().len()
        );
    }
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
    let (stdout, stdout_truncated) = crate::text::head_tail(&stdout, 6000);
    let (stderr, stderr_truncated) = crate::text::head_tail(&stderr, 4000);
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
    let client = reqwest::Client::builder()
        .redirect(if args.follow_redirects {
            reqwest::redirect::Policy::limited(10)
        } else {
            reqwest::redirect::Policy::none()
        })
        .timeout(Duration::from_secs(args.timeout_seconds))
        .build()?;
    let mut request = client.request(method.parse()?, url.clone());
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
            request = request.header(key, value);
        }
    }
    let response = request.send().await?;
    let status = response.status();
    let version = response.version();
    let final_url = response.url().to_string();
    if final_url != url.as_str() {
        validate_public_url(&final_url).await?;
    }
    let headers = response.headers().clone();
    let content_type = headers
        .get(reqwest::header::CONTENT_TYPE)
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
        let (text, truncated) = crate::text::head_tail(&normalized, 12000);
        return Ok(json!({
            "method": method,
            "url": final_url,
            "status_code": status.as_u16(),
            "reason_phrase": reason_phrase(status),
            "http_version": format!("{:?}", version),
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
        "reason_phrase": reason_phrase(status),
        "http_version": format!("{:?}", version),
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
    let input_todos = if args.todos.is_empty() {
        ctx.todos.clone()
    } else {
        args.todos
            .into_iter()
            .map(|item| TodoItem {
                id: item.id.unwrap_or_default(),
                task: item.task,
                status: item.status,
            })
            .collect()
    };
    let mut todos = Vec::with_capacity(input_todos.len());
    for (index, item) in input_todos.into_iter().enumerate() {
        let id = Some(crate::text::compact_spaces(&item.id))
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| (index + 1).to_string());
        let task = crate::text::compact_spaces(&item.task);
        if task.is_empty() {
            bail!("todo task cannot be empty");
        }
        if !matches!(item.status.as_str(), "pending" | "in_progress" | "done") {
            bail!("invalid todo status: {}", item.status);
        }
        todos.push(TodoItem {
            id,
            task,
            status: item.status,
        });
    }
    ctx.todos = todos;
    if args.persist {
        save_todos_to_file(&ctx.root, &ctx.todos)?;
    }
    let counts = todo_status_counts(&ctx.todos);
    let preview = format_todo_preview(&ctx.todos);
    Ok(json!({
        "path": TODO_FILE,
        "persisted": args.persist,
        "items": ctx.todos,
        "count": ctx.todos.len(),
        "status_counts": {
            "pending": counts.pending,
            "in_progress": counts.in_progress,
            "done": counts.done
        },
        "preview": preview
    }))
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

#[derive(Debug, Clone)]
struct VirtualPath {
    archive: PathBuf,
    member: Option<String>,
}

fn resolve_virtual_path(root: &Path, path: &str) -> Result<VirtualPath> {
    let (archive_path, member) = path
        .split_once("::")
        .map(|(archive, member)| (archive, Some(member.trim_start_matches('/').to_string())))
        .unwrap_or((path, None));
    if member.as_ref().is_some_and(|m| m.contains("..")) {
        bail!("invalid archive member path: {path}");
    }
    Ok(VirtualPath {
        archive: resolve_existing_path(root, archive_path)?,
        member,
    })
}

fn is_archive_path(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    path.extension().and_then(|s| s.to_str()) == Some("zip")
        || name.ends_with(".tar")
        || name.ends_with(".tar.gz")
        || name.ends_with(".tgz")
}

fn normalize_member(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches('/').to_string()
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

fn list_archive_virtual(root: &Path, path: &str, exclude: &GlobSet) -> Result<Vec<String>> {
    let virtual_path = resolve_virtual_path(root, path)?;
    if !is_archive_path(&virtual_path.archive) {
        bail!("list archive path is not an archive: {path}");
    }
    let rel = rel_path(root, &virtual_path.archive);
    let prefix = virtual_path
        .member
        .as_deref()
        .map(normalize_member)
        .unwrap_or_default();
    let mut out = file_texts_for_archive(&virtual_path.archive, &rel)?
        .into_iter()
        .map(|item| item.display_path)
        .filter(|display| {
            if prefix.is_empty() {
                true
            } else {
                display
                    .split_once("::")
                    .map(|(_, member)| member.starts_with(&prefix))
                    .unwrap_or(false)
            }
        })
        .filter(|display| !exclude.is_match(display.as_str()))
        .collect::<Vec<_>>();
    out.sort();
    out.dedup();
    Ok(out)
}

fn read_virtual_text(root: &Path, path: &str) -> Result<(String, String)> {
    let virtual_path = resolve_virtual_path(root, path)?;
    if virtual_path.archive.is_dir() {
        bail!("read path is a directory: {path}");
    }
    let rel = rel_path(root, &virtual_path.archive);
    if let Some(member) = virtual_path.member.as_ref() {
        let member = normalize_member(member);
        let item = archive_member_text(&virtual_path.archive, &rel, &member)?
            .with_context(|| format!("archive member not found: {rel}::{member}"))?;
        return Ok((item.display_path, item.text));
    }
    let mut texts = file_texts(root, &virtual_path.archive)?;
    if texts.len() == 1 {
        let item = texts.remove(0);
        return Ok((item.display_path, item.text));
    }
    if is_archive_path(&virtual_path.archive) {
        bail!("archive path requires a member, e.g. {rel}::path/in/archive");
    }
    bail!("read path is not utf-8 text: {path}")
}

fn archive_member_text(path: &Path, rel: &str, member: &str) -> Result<Option<SearchText>> {
    for item in file_texts_for_archive(path, rel)? {
        if item.display_path == format!("{rel}::{member}") {
            return Ok(Some(item));
        }
    }
    Ok(None)
}

fn file_texts(root: &Path, path: &Path) -> Result<Vec<SearchText>> {
    let rel = rel_path(root, path);
    if is_archive_path(path) {
        return file_texts_for_archive(path, &rel);
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
    Ok(raw)
}

fn file_texts_for_archive(path: &Path, rel: &str) -> Result<Vec<SearchText>> {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    if path.extension().and_then(|s| s.to_str()) == Some("zip") {
        return zip_texts(path, rel);
    }
    if name.ends_with(".tar") || name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        return tar_texts(path, rel);
    }
    Ok(Vec::new())
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
    line_number: usize,
    line: &str,
    column: usize,
    out: &mut Vec<Value>,
) {
    out.push(json!({
        "path": display_path,
        "line_number": line_number,
        "column": column,
        "text": crate::text::truncate_chars(line.trim_end_matches(['\r', '\n']), 1000)
    }));
}

fn search_text_grep(
    display_path: &str,
    text: &str,
    matcher: &RegexMatcher,
    column_regex: &Regex,
    out: &mut Vec<Value>,
) -> Result<()> {
    let mut searcher = SearcherBuilder::new().line_number(true).build();
    let mut sink = UTF8(|line_number, line: &str| {
        let column = column_regex.find(line).map(|m| m.start() + 1).unwrap_or(1);
        push_match(display_path, line_number as usize, line, column, out);
        Ok(true)
    });
    searcher.search_reader(matcher, text.as_bytes(), &mut sink)?;
    Ok(())
}

fn search_file(
    root: &Path,
    path: &Path,
    matcher: &RegexMatcher,
    column_regex: &Regex,
) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for item in file_texts(root, path)? {
        search_text_grep(
            &item.display_path,
            &item.text,
            matcher,
            column_regex,
            &mut out,
        )?;
    }
    Ok(out)
}

enum ReplaceOutcome {
    Changed(usize),
    Unchanged,
    Skipped(&'static str),
}

fn reason_phrase(status: StatusCode) -> &'static str {
    status.canonical_reason().unwrap_or("")
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
    crate::highlight::stderr(&format!("approve {tool}? [y/N]\n"));
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
        let found = search_file(dir.path(), &zip_path, &matcher, &column_regex).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0]["path"], "sample.zip::src/lib.rs");
    }

    #[test]
    fn read_supports_zip_virtual_member() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("sample.zip");
        {
            let file = fs::File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file("docs/readme.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(
                &mut zip,
                b"one
two
three
",
            )
            .unwrap();
            zip.finish().unwrap();
        }
        let value = tool_read(
            &ToolContext {
                root: dir.path().to_path_buf(),
                interactive: false,
                yolo: false,
                agent: "default".into(),
                todos: Vec::new(),
            },
            ReadArgs {
                path: "sample.zip::docs/readme.txt".into(),
                offset: 2,
                limit: 1,
            },
        )
        .unwrap();
        assert_eq!(value["text"], "two");
        assert_eq!(value["path"], "sample.zip::docs/readme.txt");
        assert_eq!(value["line_count"], 3);
    }

    #[test]
    fn list_shows_zip_members() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("sample.zip");
        {
            let file = fs::File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file("a.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut zip, b"a").unwrap();
            zip.start_file("nested/b.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut zip, b"b").unwrap();
            zip.finish().unwrap();
        }
        let value = tool_list(
            &ToolContext {
                root: dir.path().to_path_buf(),
                interactive: false,
                yolo: false,
                agent: "default".into(),
                todos: Vec::new(),
            },
            ListArgs {
                path: "sample.zip".into(),
                exclude: None,
                limit: 10,
            },
        )
        .unwrap();
        let items = value["items"].as_array().unwrap();
        assert!(items.iter().any(|item| item == "sample.zip::a.txt"));
        assert!(items.iter().any(|item| item == "sample.zip::nested/b.txt"));
    }

    #[test]
    fn todo_tool_persists_markdown_when_requested() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ToolContext {
            root: dir.path().to_path_buf(),
            interactive: false,
            yolo: false,
            agent: "default".into(),
            todos: Vec::new(),
        };
        let value = tool_todo(
            &mut ctx,
            TodoArgs {
                todos: vec![TodoItemInput {
                    id: Some("a".into()),
                    task: "ship it".into(),
                    status: "in_progress".into(),
                }],
                persist: true,
            },
        )
        .unwrap();
        assert_eq!(value["path"], TODO_FILE);
        assert_eq!(value["persisted"], true);
        let text = fs::read_to_string(dir.path().join(TODO_FILE)).unwrap();
        assert!(text.contains("- [~] a: ship it"));
    }
}
