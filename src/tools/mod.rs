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
use similar::{ChangeTag, TextDiff};
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
    pub policy: ToolPolicy,
    pub todos: Vec<TodoItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Approval {
    Deny,
    Ask,
    Auto,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ToolPolicy {
    pub read_only: bool,
    pub files_write: Approval,
    pub shell: Approval,
    pub network: bool,
}

impl ToolPolicy {
    pub fn read_only() -> Self {
        Self {
            read_only: true,
            files_write: Approval::Deny,
            shell: Approval::Deny,
            network: true,
        }
    }

    pub fn approval(self, tool: &str) -> Approval {
        if self.read_only && matches!(tool, "replace" | "bash" | "todo") {
            return Approval::Deny;
        }
        match tool {
            "replace" | "todo" => self.files_write,
            "bash" => self.shell,
            _ => Approval::Deny,
        }
    }

    fn path_approval(self) -> Approval {
        if self.files_write == Approval::Auto && self.shell == Approval::Auto {
            Approval::Auto
        } else {
            Approval::Ask
        }
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolGate {
    Always,
    Interactive,
    Network,
    FilesWrite,
    Shell,
}

struct ToolDef {
    name: &'static str,
    gate: ToolGate,
    schema: fn() -> Value,
    summary: fn(&Value) -> String,
    preview: fn(&Value) -> String,
}

const TOOL_DEFS: &[ToolDef] = &[
    ToolDef {
        name: "list",
        gate: ToolGate::Always,
        schema: schema_list,
        summary: summary_list,
        preview: preview_list,
    },
    ToolDef {
        name: "read",
        gate: ToolGate::Always,
        schema: schema_read,
        summary: summary_read,
        preview: preview_read,
    },
    ToolDef {
        name: "search",
        gate: ToolGate::Always,
        schema: schema_search,
        summary: summary_search,
        preview: preview_search,
    },
    ToolDef {
        name: "sloc",
        gate: ToolGate::Always,
        schema: schema_sloc,
        summary: summary_sloc,
        preview: preview_sloc,
    },
    ToolDef {
        name: "todo",
        gate: ToolGate::Always,
        schema: schema_todo,
        summary: summary_todo,
        preview: preview_todo,
    },
    ToolDef {
        name: "ask",
        gate: ToolGate::Interactive,
        schema: schema_ask,
        summary: summary_ask,
        preview: preview_ask,
    },
    ToolDef {
        name: "webfetch",
        gate: ToolGate::Network,
        schema: schema_webfetch,
        summary: summary_webfetch,
        preview: preview_webfetch,
    },
    ToolDef {
        name: "replace",
        gate: ToolGate::FilesWrite,
        schema: schema_replace,
        summary: summary_replace,
        preview: preview_replace,
    },
    ToolDef {
        name: "bash",
        gate: ToolGate::Shell,
        schema: schema_bash,
        summary: summary_bash,
        preview: preview_bash,
    },
];

fn tool_def(name: &str) -> Option<&'static ToolDef> {
    TOOL_DEFS.iter().find(|def| def.name == name)
}

fn tool_enabled(ctx: &ToolContext, def: &ToolDef) -> bool {
    match def.gate {
        ToolGate::Always => true,
        ToolGate::Interactive => ctx.interactive,
        ToolGate::Network => ctx.policy.network,
        ToolGate::FilesWrite => !ctx.policy.read_only && ctx.policy.files_write != Approval::Deny,
        ToolGate::Shell => !ctx.policy.read_only && ctx.policy.shell != Approval::Deny,
    }
}

fn spec(def: &ToolDef) -> Tool {
    Tool::new(def.name)
        .with_description(crate::config::tool_description(def.name))
        .with_schema((def.schema)())
}

fn object(properties: Value, required: &[&str]) -> Value {
    let mut schema = json!({
        "type": "object",
        "properties": properties,
        "additionalProperties": false
    });
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    schema
}

fn exclude_schema() -> Value {
    json!({"anyOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}, {"type": "null"}]})
}

fn todo_item_schema() -> Value {
    object(
        json!({
            "id": {"type": "string", "description": "Stable short id; optional, defaults to 1-based position."},
            "task": {"type": "string"},
            "status": {"type": "string", "enum": ["pending", "in_progress", "done"], "default": "pending"}
        }),
        &["task"],
    )
}

fn schema_list() -> Value {
    object(
        json!({
            "path": {"type": "string", "default": "*"},
            "exclude": exclude_schema(),
            "limit": {"type": "integer", "default": DEFAULT_LIMIT}
        }),
        &[],
    )
}

fn schema_read() -> Value {
    object(
        json!({
            "path": {"type": "string"},
            "offset": {"type": "integer", "default": 1},
            "limit": {"type": "integer", "default": DEFAULT_LIMIT}
        }),
        &["path"],
    )
}

fn schema_search() -> Value {
    object(
        json!({
            "pattern": {"type": "string"},
            "path": {"type": "string", "default": "."},
            "exclude": exclude_schema(),
            "limit": {"type": "integer", "default": DEFAULT_LIMIT}
        }),
        &["pattern"],
    )
}

fn schema_sloc() -> Value {
    object(
        json!({
            "path": {"type": "string", "default": "."},
            "exclude": exclude_schema()
        }),
        &[],
    )
}

fn schema_todo() -> Value {
    object(
        json!({
            "todos": {"type": "array", "description": "Complete replacement todo list. Alias: items. Omit to return current list.", "items": todo_item_schema()},
            "items": {"type": "array", "description": "Alias for todos.", "items": todo_item_schema()},
            "persist": {"type": "boolean", "default": false, "description": "Write to TODO.md; default false avoids git churn."}
        }),
        &[],
    )
}

fn schema_ask() -> Value {
    object(
        json!({
            "question": {"type": "string"},
            "choices": {"type": ["array", "null"], "items": {"type": "string"}}
        }),
        &["question"],
    )
}

fn schema_webfetch() -> Value {
    object(
        json!({
            "url": {"type": "string"},
            "method": {"type": "string", "default": "GET"},
            "headers": {"type": ["object", "null"], "additionalProperties": {"type": "string"}},
            "follow_redirects": {"type": "boolean", "default": false},
            "timeout_seconds": {"type": "integer", "default": DEFAULT_WEBFETCH_TIMEOUT_SECONDS}
        }),
        &["url"],
    )
}

fn schema_replace() -> Value {
    object(
        json!({
            "pattern": {"type": "string"},
            "replacement": {"type": "string"},
            "path": {"type": "string", "default": "."},
            "exclude": exclude_schema(),
            "limit": {"type": "integer", "default": DEFAULT_LIMIT}
        }),
        &["pattern", "replacement"],
    )
}

fn schema_bash() -> Value {
    object(
        json!({
            "command": {"type": "string"},
            "timeout_seconds": {"type": "integer", "default": 120}
        }),
        &["command"],
    )
}

pub fn tool_specs(ctx: &ToolContext) -> Vec<Tool> {
    TOOL_DEFS
        .iter()
        .filter(|def| tool_enabled(ctx, def))
        .map(spec)
        .collect()
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
        crate::ui::tool_result(&preview_tool_output(name, value));
    } else if let Err(err) = &result {
        crate::ui::tool_error(name, err);
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
    crate::ui::tool_start(name, &detail);
}

fn tool_call_summary(name: &str, args: &Value) -> String {
    tool_def(name)
        .map(|def| (def.summary)(args))
        .unwrap_or_else(|| preview_value(args, 120))
}

fn summary_list(args: &Value) -> String {
    compact_kvs(args, &[("path", 60), ("exclude", 40)])
}

fn summary_read(args: &Value) -> String {
    compact_kvs(args, &[("path", 70), ("offset", 12), ("limit", 12)])
}

fn summary_search(args: &Value) -> String {
    compact_kvs(args, &[("pattern", 70), ("path", 50), ("exclude", 35)])
}

fn summary_replace(args: &Value) -> String {
    compact_kvs(args, &[("path", 45), ("pattern", 45), ("replacement", 45)])
}

fn summary_sloc(args: &Value) -> String {
    compact_kvs(args, &[("path", 70), ("exclude", 40)])
}

fn summary_bash(args: &Value) -> String {
    preview_value(args.get("command").unwrap_or(&Value::Null), 100)
}

fn summary_webfetch(args: &Value) -> String {
    compact_kvs(args, &[("method", 8), ("url", 100)])
}

fn summary_ask(args: &Value) -> String {
    preview_value(args.get("question").unwrap_or(&Value::Null), 100)
}

fn summary_todo(args: &Value) -> String {
    todo_call_summary(args)
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
    tool_def(name)
        .map(|def| (def.preview)(value))
        .unwrap_or_else(|| preview_generic(value))
}

fn preview_value(value: &Value, max: usize) -> String {
    let raw = value
        .as_str()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| value.to_string());
    crate::ui::compact_preview(&raw, max)
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

fn verbose_preview(body: impl FnOnce() -> String) -> Option<String> {
    crate::ui::is_verbose().then(body)
}

fn with_verbose(summary: String, body: impl FnOnce() -> String) -> String {
    if let Some(body) = verbose_preview(body).filter(|body| !body.trim().is_empty()) {
        format!("{summary}\n{body}")
    } else {
        summary
    }
}

fn count_lines(text: &str) -> usize {
    text.lines().count()
}

fn count_files_in_matches(matches: &[Value]) -> usize {
    matches
        .iter()
        .filter_map(|item| item.get("path").and_then(Value::as_str))
        .collect::<std::collections::BTreeSet<_>>()
        .len()
}

fn append_preview_lines(out: &mut String, text: &str, indent: &str) {
    let line_count = text.lines().count();
    for line in text.lines().take(PREVIEW_LINES) {
        let _ = write!(
            out,
            "\n{indent}{}",
            crate::ui::truncate_chars(line, PREVIEW_LINE_CHARS)
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
    if crate::ui::is_verbose() {
        crate::ui::clamp_lines(
            &encode_tool_output(value),
            PREVIEW_LINES,
            PREVIEW_LINE_CHARS,
        )
    } else if !value_bool(value, "ok") && value.get("ok").is_some() {
        format!("error: {}", value_str(value, "error"))
    } else {
        preview_value(value, crate::ui::terminal_width().saturating_sub(4).max(40))
    }
}

fn preview_list(value: &Value) -> String {
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let total = value_usize(value, "count");
    let summary = format!(
        "{} item{} in {}",
        total,
        plural(total),
        value_str(value, "path")
    );
    with_verbose(summary, || {
        let mut out = String::new();
        for item in items.iter().take(PREVIEW_ITEMS) {
            let _ = write!(
                out,
                "\n  {}",
                crate::ui::truncate_chars(item.as_str().unwrap_or(""), PREVIEW_LINE_CHARS)
            );
        }
        let shown = items.len().min(PREVIEW_ITEMS);
        if total > shown || value_bool(value, "truncated") {
            let remaining = total.saturating_sub(shown);
            let _ = write!(out, "\n  … {remaining} more item{}", plural(remaining));
        }
        out.trim_start().to_string()
    })
}
fn preview_read(value: &Value) -> String {
    let path = value_str(value, "path");
    let offset = value_usize(value, "offset");
    let line_count = value_usize(value, "line_count");
    let text = value_str(value, "text");
    let shown = text.lines().count();
    let end = offset.saturating_add(shown).saturating_sub(1);
    let more = if value_bool(value, "truncated") {
        format!(" · {} more", line_count.saturating_sub(end))
    } else {
        String::new()
    };
    let summary = format!("{path}:{offset}-{end} · {shown}/{line_count} lines{more}");
    with_verbose(summary, || {
        let mut out = String::new();
        if text.is_empty() {
            out.push_str("  <empty>");
        } else {
            out.push_str(&crate::ui::code(path, text, offset));
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
    })
}
fn preview_search(value: &Value) -> String {
    let matches = value
        .get("matches")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let total = value_usize(value, "match_count");
    let files = count_files_in_matches(matches);
    let summary = if total == 0 {
        format!("0 matches for /{}/", value_str(value, "pattern"))
    } else {
        format!(
            "{} {} in {} file{} for /{}/",
            total,
            if total == 1 { "match" } else { "matches" },
            files,
            plural(files),
            value_str(value, "pattern")
        )
    };
    if !crate::ui::is_verbose() {
        return match matches.first() {
            Some(item) => format!("{summary}\n  {}", format_search_hit(item)),
            None => summary,
        };
    }
    with_verbose(summary, || {
        let mut out = String::new();
        for item in matches.iter().take(PREVIEW_ITEMS) {
            let _ = write!(out, "\n  {}", format_search_hit(item));
        }
        if value_bool(value, "truncated") {
            let _ = write!(
                out,
                "\n  … {} more matches",
                total.saturating_sub(matches.len().min(PREVIEW_ITEMS))
            );
        }
        out.trim_start().to_string()
    })
}

fn format_search_hit(item: &Value) -> String {
    let path = value_str(item, "path");
    let line = value_usize(item, "line_number");
    let col = value_usize(item, "column");
    let text = crate::ui::truncate_chars(value_str(item, "text"), PREVIEW_LINE_CHARS);
    format!("{path}:{line}:{col}: {text}")
}
fn preview_replace(value: &Value) -> String {
    let changed = value
        .get("changed_files")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let total_files = value_usize(value, "changed_file_count");
    let files = total_files.max(changed.len());
    let replacements = value_usize(value, "replacement_count");
    let mut summary = format!(
        "{} file{} changed · {} replacement{}",
        files,
        plural(files),
        replacements,
        plural(replacements)
    );
    if !crate::ui::is_verbose() && !changed.is_empty() && changed.len() <= 3 {
        let names = changed
            .iter()
            .map(|item| value_str(item, "path"))
            .collect::<Vec<_>>()
            .join(", ");
        summary.push_str(&format!(" · {names}"));
    }
    with_verbose(summary, || {
        let mut out = String::new();
        if changed.is_empty() {
            out.push_str("  <no changes>");
        } else {
            for item in changed.iter().take(PREVIEW_ITEMS) {
                let _ = write!(
                    out,
                    "\n  {} · {} repl",
                    value_str(item, "path"),
                    value_usize(item, "replacements")
                );
            }
            if value_bool(value, "truncated") || files > changed.len() {
                let _ = write!(
                    out,
                    "\n  … {} more files",
                    files.saturating_sub(changed.len())
                );
            }
        }
        if let Some(diff) = value
            .get("diff")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&crate::ui::diff(diff));
        }
        out.trim_start().to_string()
    })
}
fn preview_bash(value: &Value) -> String {
    let code = value
        .get("returncode")
        .and_then(Value::as_i64)
        .unwrap_or(-1);
    let stdout = value_str(value, "stdout");
    let stderr = value_str(value, "stderr");
    let icon = if code == 0 {
        crate::ui::paint("32", "✓")
    } else {
        crate::ui::paint("31", "✗")
    };
    let mut summary = format!(
        "{icon} exit {code} · stdout {} line{} · stderr {} line{}",
        count_lines(stdout),
        plural(count_lines(stdout)),
        count_lines(stderr),
        plural(count_lines(stderr))
    );
    if code != 0 {
        if let Some(first_stderr) = stderr.lines().find(|line| !line.trim().is_empty()) {
            summary.push_str(&format!(
                " · {}",
                crate::ui::truncate_chars(first_stderr.trim(), 80)
            ));
        }
    }
    with_verbose(summary, || {
        let mut out = String::new();
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
        out.trim_start().to_string()
    })
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
            "HTTP {status} · binary · {} bytes · {url}",
            value_usize(value, "content_bytes")
        );
    }
    let text = value_str(value, "text");
    let format = value_str(value, "format");
    let kind = if format.is_empty() { "text" } else { format };
    let summary = format!(
        "HTTP {status} · {kind} · {} line{} · {url}",
        count_lines(text),
        plural(count_lines(text))
    );
    with_verbose(summary, || {
        let mut out = String::new();
        if !text.is_empty() {
            let preview = crate::ui::clamp_lines(text, PREVIEW_LINES, PREVIEW_LINE_CHARS);
            for line in preview.lines() {
                let _ = write!(out, "\n  {line}");
            }
        }
        if value_bool(value, "truncated") {
            out.push_str("\n  … response body truncated for model context");
        }
        out.trim_start().to_string()
    })
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
    let summary = format!(
        "{}: {total} code · {comments} comments · {blanks} blank",
        value_str(value, "path")
    );
    with_verbose(summary, || {
        let mut out = String::new();
        for (name, code) in langs.into_iter().take(PREVIEW_ITEMS) {
            let _ = write!(out, "\n  {name}: {code}");
        }
        out.trim_start().to_string()
    })
}
fn preview_ask(value: &Value) -> String {
    let answer = value.as_str().unwrap_or_default();
    if answer.is_empty() {
        "<no selection>".to_string()
    } else {
        format!(
            "selected: {}",
            crate::ui::truncate_chars(answer, PREVIEW_LINE_CHARS)
        )
    }
}

fn preview_todo(value: &Value) -> String {
    let preview = value_str(value, "preview").to_string().if_empty_then(|| {
        let items = value
            .get("items")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        format_todo_preview_from_values(items)
    });
    if crate::ui::is_verbose() {
        preview
    } else {
        preview.lines().next().unwrap_or_default().to_string()
    }
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
    require_path_pattern_approval(ctx, &args.path)?;
    let exclude = build_exclude_set(args.exclude.as_ref())?;
    let shown_limit = args.limit.max(1);
    if args.path.contains("::") {
        let items = list_archive_virtual(ctx, &args.path, &exclude)?;
        return Ok(json!({
            "path": args.path,
            "items": items.iter().take(shown_limit).cloned().collect::<Vec<_>>(),
            "count": items.len(),
            "truncated": items.len() > shown_limit,
            "exclude": args.exclude.as_ref().map(ExcludeArg::patterns)
        }));
    }
    let target_for_archive = resolve_existing_path(ctx, &args.path).ok();
    if target_for_archive
        .as_ref()
        .is_some_and(|path| path.is_file() && is_archive_path(path))
    {
        let items = list_archive_virtual(ctx, &format!("{}::", args.path), &exclude)?;
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
        let pattern = if Path::new(&args.path).is_absolute() {
            args.path.clone()
        } else {
            ctx.root.join(&args.path).to_string_lossy().to_string()
        };
        let mut out = glob(&pattern)?
            .filter_map(|entry| entry.ok())
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
    let (display_path, text) = read_virtual_text(ctx, &args.path)?;
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
    let targets = resolve_existing_paths(ctx, &args.path)?;
    let mut matches = Vec::new();
    let mut errors = Vec::new();
    for target in &targets {
        for path in walk_files(&ctx.root, target, &exclude)? {
            match search_file(&ctx.root, &path, &matcher, &column_regex) {
                Ok(mut found) => matches.append(&mut found),
                Err(err) => errors
                    .push(json!({"path": rel_path(&ctx.root, &path), "message": err.to_string()})),
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
    let regex =
        Regex::new(&args.pattern).with_context(|| format!("invalid regex: {}", args.pattern))?;
    let exclude = build_exclude_set(args.exclude.as_ref())?;
    let target = resolve_existing_path(ctx, &args.path)?;
    let approval_preview = if ctx.policy.approval("replace") == Approval::Ask && ctx.interactive {
        preview_replace_plan(ctx, &args, &regex, &target, &exclude).ok()
    } else {
        None
    };
    require_mutation_approval(ctx, "replace", approval_preview.as_deref())?;
    let mut changed_files = Vec::new();
    let mut skipped = Vec::new();
    let mut errors = Vec::new();
    let mut replacement_count = 0usize;
    for path in walk_files(&ctx.root, &target, &exclude)? {
        match replace_file(&path, &regex, &args.replacement) {
            Ok(ReplaceOutcome::Changed { count, diff }) => {
                changed_files.push(json!({
                    "path": rel_path(&ctx.root, &path),
                    "replacements": count,
                    "diff": diff
                }));
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
        "diff": combined_diff(&changed_files),
        "truncated": changed_files.len() > shown,
        "exclude": args.exclude.as_ref().map(ExcludeArg::patterns),
        "skipped": skipped,
        "errors": errors
    }))
}

fn tool_sloc(ctx: &ToolContext, args: SlocArgs) -> Result<Value> {
    let target = resolve_existing_path(ctx, &args.path)?;
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
    require_mutation_approval(ctx, "bash", None)?;
    if args.command.len() > config::max_bash_cmd_bytes() {
        bail!("command too large ({} bytes)", args.command.len());
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
    let (stdout, stdout_truncated) = crate::ui::head_tail(&stdout, 6000);
    let (stderr, stderr_truncated) = crate::ui::head_tail(&stderr, 4000);
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
        let (text, truncated) = crate::ui::head_tail(&normalized, 12000);
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
    Ok(Value::String(crate::chat::ask(
        &args.question,
        args.choices.as_deref(),
    )?))
}

fn tool_todo(ctx: &mut ToolContext, args: TodoArgs) -> Result<Value> {
    if args.persist {
        require_mutation_approval(ctx, "todo", None)?;
    }
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
        let id = Some(crate::ui::compact_spaces(&item.id))
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| (index + 1).to_string());
        let task = crate::ui::compact_spaces(&item.task);
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

fn path_scope_issue(root: &Path, path: &str, resolved: Option<&Path>) -> Option<String> {
    let raw = Path::new(path);
    if raw.is_absolute() {
        return Some("absolute path".to_string());
    }
    if raw.components().any(|c| matches!(c, Component::ParentDir)) {
        return Some("parent-directory path".to_string());
    }
    if let Some(resolved) = resolved.filter(|resolved| !within_root(root, resolved)) {
        return Some(format!("outside workspace: {}", resolved.display()));
    }
    None
}

fn require_path_pattern_approval(ctx: &ToolContext, path: &str) -> Result<()> {
    if let Some(issue) = path_scope_issue(&ctx.root, path, None) {
        require_path_approval(ctx, path, &issue)?;
    }
    Ok(())
}

fn require_resolved_path_approval(
    ctx: &ToolContext,
    requested: &str,
    resolved: &Path,
) -> Result<()> {
    if let Some(issue) = path_scope_issue(&ctx.root, requested, Some(resolved)) {
        require_path_approval(ctx, requested, &issue)?;
    }
    Ok(())
}

fn require_path_approval(ctx: &ToolContext, requested: &str, issue: &str) -> Result<()> {
    match ctx.policy.path_approval() {
        Approval::Auto => Ok(()),
        Approval::Deny => bail!("path denied by policy: {requested} ({issue})"),
        Approval::Ask if !ctx.interactive => bail!(
            "path requires interactive approval or an auto-approve agent: {requested} ({issue})"
        ),
        Approval::Ask => {
            crate::ui::err_line(format_args!(
                "approve path outside workspace? {requested} ({issue}) [y/N]"
            ));
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            if matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                Ok(())
            } else {
                bail!("path denied by user")
            }
        }
    }
}

#[derive(Debug, Clone)]
struct VirtualPath {
    archive: PathBuf,
    member: Option<String>,
}

fn resolve_virtual_path(ctx: &ToolContext, path: &str) -> Result<VirtualPath> {
    let (archive_path, member) = path
        .split_once("::")
        .map(|(archive, member)| (archive, Some(member.trim_start_matches('/').to_string())))
        .unwrap_or((path, None));
    if member.as_ref().is_some_and(|m| m.contains("..")) {
        bail!("invalid archive member path: {path}");
    }
    Ok(VirtualPath {
        archive: resolve_existing_path(ctx, archive_path)?,
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

fn resolve_existing_path(ctx: &ToolContext, path: &str) -> Result<PathBuf> {
    require_path_pattern_approval(ctx, path)?;
    let joined = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        ctx.root.join(path)
    };
    let resolved = joined
        .canonicalize()
        .with_context(|| format!("path does not exist: {path}"))?;
    require_resolved_path_approval(ctx, path, &resolved)?;
    Ok(resolved)
}

fn resolve_existing_paths(ctx: &ToolContext, path: &str) -> Result<Vec<PathBuf>> {
    match resolve_existing_path(ctx, path) {
        Ok(path) => Ok(vec![path]),
        Err(full_path_error) => {
            let parts = path.split_whitespace().collect::<Vec<_>>();
            if parts.len() <= 1 {
                return Err(full_path_error);
            }
            let mut out = Vec::new();
            for part in parts {
                out.push(resolve_existing_path(ctx, part)?);
            }
            out.sort();
            out.dedup();
            Ok(out)
        }
    }
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

fn list_archive_virtual(ctx: &ToolContext, path: &str, exclude: &GlobSet) -> Result<Vec<String>> {
    let virtual_path = resolve_virtual_path(ctx, path)?;
    if !is_archive_path(&virtual_path.archive) {
        bail!("list archive path is not an archive: {path}");
    }
    let rel = rel_path(&ctx.root, &virtual_path.archive);
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

fn read_virtual_text(ctx: &ToolContext, path: &str) -> Result<(String, String)> {
    let virtual_path = resolve_virtual_path(ctx, path)?;
    if virtual_path.archive.is_dir() {
        bail!("read path is a directory: {path}");
    }
    let rel = rel_path(&ctx.root, &virtual_path.archive);
    if let Some(member) = virtual_path.member.as_ref() {
        let member = normalize_member(member);
        let item = archive_member_text(&virtual_path.archive, &rel, &member)?
            .with_context(|| format!("archive member not found: {rel}::{member}"))?;
        return Ok((item.display_path, item.text));
    }
    let mut texts = file_texts(&ctx.root, &virtual_path.archive)?;
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
        "text": crate::ui::truncate_chars(line.trim_end_matches(['\r', '\n']), 1000)
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
    Changed { count: usize, diff: String },
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
    let diff = unified_diff(&path.to_string_lossy(), &text, &updated);
    fs::write(path, updated)?;
    Ok(ReplaceOutcome::Changed { count, diff })
}

fn unified_diff(path: &str, old: &str, new: &str) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut out = String::new();
    let _ = writeln!(out, "--- {path}");
    let _ = writeln!(out, "+++ {path}");
    for group in diff.grouped_ops(3) {
        for op in group {
            for change in diff.iter_changes(&op) {
                let sign = match change.tag() {
                    ChangeTag::Delete => '-',
                    ChangeTag::Insert => '+',
                    ChangeTag::Equal => ' ',
                };
                let _ = write!(out, "{sign}{change}");
            }
        }
    }
    crate::ui::head_tail(&out, 12000).0
}

fn combined_diff(files: &[Value]) -> String {
    let text = files
        .iter()
        .filter_map(|item| item.get("diff").and_then(Value::as_str))
        .filter(|diff| !diff.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    crate::ui::head_tail(&text, 12000).0
}

fn preview_replace_plan(
    ctx: &ToolContext,
    args: &ReplaceArgs,
    regex: &Regex,
    target: &Path,
    exclude: &GlobSet,
) -> Result<String> {
    let mut changed = Vec::new();
    for path in walk_files(&ctx.root, target, exclude)? {
        if path.is_symlink() {
            continue;
        }
        let raw = match fs::read(&path) {
            Ok(raw) => raw,
            Err(_) => continue,
        };
        if raw.contains(&0) {
            continue;
        }
        let Ok(text) = String::from_utf8(raw) else {
            continue;
        };
        if !regex.is_match(&text) {
            continue;
        }
        let updated = regex
            .replace_all(&text, args.replacement.as_str())
            .into_owned();
        changed.push(json!({
            "path": rel_path(&ctx.root, &path),
            "replacements": regex.find_iter(&text).count(),
            "diff": unified_diff(&rel_path(&ctx.root, &path), &text, &updated)
        }));
        if changed.len() >= args.limit.clamp(1, PREVIEW_ITEMS) {
            break;
        }
    }
    Ok(combined_diff(&changed))
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

fn require_mutation_approval(ctx: &ToolContext, tool: &str, preview: Option<&str>) -> Result<()> {
    match ctx.policy.approval(tool) {
        Approval::Auto => Ok(()),
        Approval::Deny => bail!("tool denied by policy: {tool}"),
        Approval::Ask if !ctx.interactive => bail!(
            "tool denied by policy: {tool} requires interactive approval or an auto-approve agent"
        ),
        Approval::Ask => {
            if let Some(preview) = preview.filter(|s| !s.trim().is_empty()) {
                crate::ui::err_line(crate::ui::diff(preview).trim_end());
            }
            crate::ui::err_line(format_args!("approve {tool}? [y/N]"));
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            let answer = line.trim().to_ascii_lowercase();
            if matches!(answer.as_str(), "y" | "yes") {
                Ok(())
            } else {
                bail!("tool denied by user")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_context(policy: ToolPolicy, interactive: bool) -> (tempfile::TempDir, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext {
            root: dir.path().to_path_buf(),
            interactive,
            policy,
            todos: Vec::new(),
        };
        (dir, ctx)
    }

    fn auto_policy() -> ToolPolicy {
        ToolPolicy {
            read_only: false,
            files_write: Approval::Auto,
            shell: Approval::Auto,
            network: true,
        }
    }

    #[test]
    fn non_interactive_default_denies_replace() {
        let (dir, ctx) = test_context(
            ToolPolicy {
                read_only: false,
                files_write: Approval::Ask,
                shell: Approval::Ask,
                network: true,
            },
            false,
        );
        fs::write(dir.path().join("a.txt"), "one").unwrap();
        let err = tool_replace(
            &ctx,
            ReplaceArgs {
                pattern: "one".into(),
                replacement: "two".into(),
                path: "a.txt".into(),
                exclude: None,
                limit: 10,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("requires interactive approval"));
        assert_eq!(fs::read_to_string(dir.path().join("a.txt")).unwrap(), "one");
    }

    #[tokio::test]
    async fn non_interactive_default_denies_bash() {
        let (_dir, ctx) = test_context(
            ToolPolicy {
                read_only: false,
                files_write: Approval::Ask,
                shell: Approval::Ask,
                network: true,
            },
            false,
        );
        let err = tool_bash(
            &ctx,
            BashArgs {
                command: "echo nope".into(),
                timeout_seconds: 1,
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("requires interactive approval"));
    }

    #[test]
    fn default_non_interactive_denies_out_of_workspace_read() {
        let (_dir, ctx) = test_context(
            ToolPolicy {
                read_only: false,
                files_write: Approval::Ask,
                shell: Approval::Ask,
                network: true,
            },
            false,
        );
        let err = tool_read(
            &ctx,
            ReadArgs {
                path: "/etc/hosts".into(),
                offset: 1,
                limit: 1,
            },
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("path requires interactive approval")
        );
    }

    #[test]
    fn auto_policy_allows_out_of_workspace_read() {
        let (_dir, ctx) = test_context(auto_policy(), false);
        let value = tool_read(
            &ctx,
            ReadArgs {
                path: "/etc/hosts".into(),
                offset: 1,
                limit: 1,
            },
        )
        .unwrap();
        assert_eq!(value["path"], "/etc/hosts");
    }

    #[test]
    fn auto_policy_allows_replace() {
        let (dir, ctx) = test_context(auto_policy(), false);
        fs::write(dir.path().join("a.txt"), "one").unwrap();
        let value = tool_replace(
            &ctx,
            ReplaceArgs {
                pattern: "one".into(),
                replacement: "two".into(),
                path: "a.txt".into(),
                exclude: None,
                limit: 10,
            },
        )
        .unwrap();
        assert_eq!(value["replacement_count"], 1);
        assert_eq!(fs::read_to_string(dir.path().join("a.txt")).unwrap(), "two");
    }

    #[test]
    fn read_only_exposes_read_network_but_not_mutation_tools() {
        let (_dir, ctx) = test_context(ToolPolicy::read_only(), false);
        let names = tool_specs(&ctx)
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        assert!(names.iter().any(|name| name == "read"));
        assert!(names.iter().any(|name| name == "webfetch"));
        assert!(!names.iter().any(|name| name == "replace"));
        assert!(!names.iter().any(|name| name == "bash"));
    }

    #[test]
    fn search_accepts_space_separated_paths() {
        let (dir, ctx) = test_context(auto_policy(), false);
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/cli.rs"), "fn cli_hit() {}\n").unwrap();
        fs::write(dir.path().join("src/ui.rs"), "fn ui_hit() {}\n").unwrap();
        fs::write(dir.path().join("src/other.rs"), "fn other_hit() {}\n").unwrap();

        let value = tool_search(
            &ctx,
            SearchArgs {
                pattern: "fn (cli|ui)_hit".into(),
                path: "src/cli.rs src/ui.rs".into(),
                exclude: None,
                limit: 10,
            },
        )
        .unwrap();

        assert_eq!(value["match_count"], 2);
        let paths = value["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["path"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        assert!(paths.iter().any(|path| path == "src/cli.rs"));
        assert!(paths.iter().any(|path| path == "src/ui.rs"));
        assert!(!paths.iter().any(|path| path == "src/other.rs"));
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
                policy: ToolPolicy {
                    read_only: false,
                    files_write: Approval::Auto,
                    shell: Approval::Auto,
                    network: true,
                },
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
                policy: ToolPolicy {
                    read_only: false,
                    files_write: Approval::Auto,
                    shell: Approval::Auto,
                    network: true,
                },
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
            policy: ToolPolicy {
                read_only: false,
                files_write: Approval::Auto,
                shell: Approval::Auto,
                network: true,
            },
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
