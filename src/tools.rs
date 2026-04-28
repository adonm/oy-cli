use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt as _;
use glob::glob;
use globset::{Glob, GlobSet, GlobSetBuilder};
use grep_regex::RegexMatcher;
use grep_searcher::SearcherBuilder;
use grep_searcher::sinks::UTF8;
use ignore::WalkBuilder;
use regex::Regex;
use reqwest::StatusCode;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value, json};
use similar::{ChangeTag, TextDiff};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::net::{IpAddr, SocketAddr};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokei::{Config as TokeiConfig, Languages as TokeiLanguages, Sort as TokeiSort};
use tokio::io::AsyncReadExt as _;
use tokio::net::lookup_host;
use tokio::process::Command;
use tokio::time::timeout;
use toon_format::encode_default;
use url::Url;

use genai::chat::Tool;

use crate::config;

// === Public tool types and constants ===
pub const DEFAULT_LIMIT: usize = 910;
pub const DEFAULT_WEBFETCH_TIMEOUT_SECONDS: u64 = 60;
const MAX_BASH_TIMEOUT_SECONDS: u64 = 600;
const MAX_WEBFETCH_TIMEOUT_SECONDS: u64 = 120;
const MAX_BASH_OUTPUT_BYTES: usize = 200_000;
const MAX_WEBFETCH_BYTES: usize = 2 * 1024 * 1024;
const TODO_FILE: &str = "TODO.md";
const PREVIEW_ITEMS: usize = 20;
const NORMAL_PREVIEW_LINES: usize = 8;
const VERBOSE_PREVIEW_LINES: usize = 30;
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
        if self.read_only && matches!(tool, "replace" | "bash" | "todo_persist") {
            return Approval::Deny;
        }
        match tool {
            "todo" => Approval::Auto,
            "replace" | "todo_persist" => self.files_write,
            "bash" => self.shell,
            _ => Approval::Deny,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SearchMode {
    Auto,
    Regex,
    Literal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReplaceMode {
    Regex,
    Literal,
}

fn default_search_mode() -> SearchMode {
    SearchMode::Auto
}

fn default_replace_mode() -> ReplaceMode {
    ReplaceMode::Regex
}

fn deserialize_usize<'de, D>(deserializer: D) -> std::result::Result<usize, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Number {
        Integer(usize),
        String(String),
    }
    match Number::deserialize(deserializer)? {
        Number::Integer(value) => Ok(value),
        Number::String(value) => value.trim().parse::<usize>().map_err(|_| {
            serde::de::Error::custom(format!("expected unsigned integer, got {value:?}"))
        }),
    }
}

fn deserialize_u64<'de, D>(deserializer: D) -> std::result::Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Number {
        Integer(u64),
        String(String),
    }
    match Number::deserialize(deserializer)? {
        Number::Integer(value) => Ok(value),
        Number::String(value) => value.trim().parse::<u64>().map_err(|_| {
            serde::de::Error::custom(format!("expected unsigned integer, got {value:?}"))
        }),
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ListArgs {
    #[serde(default = "default_glob", alias = "root")]
    path: String,
    #[serde(default)]
    exclude: Option<ExcludeArg>,
    #[serde(default = "default_limit", deserialize_with = "deserialize_usize")]
    limit: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct ReadArgs {
    #[serde(alias = "file")]
    path: String,
    #[serde(
        default = "default_offset",
        alias = "start",
        deserialize_with = "deserialize_usize"
    )]
    offset: usize,
    #[serde(
        default = "default_limit",
        alias = "lines",
        deserialize_with = "deserialize_usize"
    )]
    limit: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct SearchArgs {
    #[serde(alias = "query", alias = "regex")]
    pattern: String,
    #[serde(default = "default_dot", alias = "root")]
    path: String,
    #[serde(default)]
    exclude: Option<ExcludeArg>,
    #[serde(default = "default_limit", deserialize_with = "deserialize_usize")]
    limit: usize,
    #[serde(default = "default_search_mode")]
    mode: SearchMode,
}

#[derive(Debug, Clone, Deserialize)]
struct ReplaceArgs {
    pattern: String,
    replacement: String,
    #[serde(default = "default_dot", alias = "root")]
    path: String,
    #[serde(default)]
    exclude: Option<ExcludeArg>,
    #[serde(default = "default_limit", deserialize_with = "deserialize_usize")]
    limit: usize,
    #[serde(default = "default_replace_mode")]
    mode: ReplaceMode,
}

#[derive(Debug, Clone, Deserialize)]
struct SlocArgs {
    #[serde(default = "default_dot", alias = "root")]
    path: String,
    #[serde(default)]
    exclude: Option<ExcludeArg>,
}

#[derive(Debug, Clone, Deserialize)]
struct BashArgs {
    #[serde(alias = "cmd")]
    command: String,
    #[serde(default = "default_bash_timeout", deserialize_with = "deserialize_u64")]
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
    #[serde(default = "default_web_timeout", deserialize_with = "deserialize_u64")]
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

// === Tool definitions and schemas ===
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

fn object<const N: usize>(properties: [(&str, Value); N], required: &[&str]) -> Value {
    let mut props = Map::new();
    for (name, schema) in properties {
        props.insert(name.to_string(), schema);
    }

    let mut schema = Map::new();
    schema.insert("type".to_string(), json!("object"));
    schema.insert("properties".to_string(), Value::Object(props));
    schema.insert("additionalProperties".to_string(), json!(false));
    if !required.is_empty() {
        schema.insert("required".to_string(), json!(required));
    }
    Value::Object(schema)
}

fn string() -> Value {
    json!({"type": "string"})
}

fn string_default(default: &str) -> Value {
    json!({"type": "string", "default": default})
}

fn string_enum(values: &[&str], default: &str) -> Value {
    json!({"type": "string", "enum": values, "default": default})
}

fn integer_default(default: impl Serialize) -> Value {
    json!({"type": ["integer", "string"], "default": default})
}

fn bool_default(default: bool) -> Value {
    json!({"type": "boolean", "default": default})
}

fn array_of(items: Value) -> Value {
    json!({"type": "array", "items": items})
}

fn nullable_string_array() -> Value {
    json!({"type": ["array", "null"], "items": string()})
}

fn describe(mut schema: Value, description: &str) -> Value {
    schema["description"] = json!(description);
    schema
}

fn exclude_schema() -> Value {
    json!({"anyOf": [string(), array_of(string()), {"type": "null"}]})
}

fn todo_item_schema() -> Value {
    object(
        [
            (
                "id",
                describe(
                    string(),
                    "Stable short id; optional, defaults to 1-based position.",
                ),
            ),
            ("task", string()),
            (
                "status",
                string_enum(&["pending", "in_progress", "done"], "pending"),
            ),
        ],
        &["task"],
    )
}

fn schema_list() -> Value {
    object(
        [
            ("path", string_default("*")),
            ("exclude", exclude_schema()),
            ("limit", integer_default(DEFAULT_LIMIT)),
        ],
        &[],
    )
}

fn schema_read() -> Value {
    object(
        [
            ("path", string()),
            ("offset", integer_default(1)),
            ("limit", integer_default(DEFAULT_LIMIT)),
        ],
        &["path"],
    )
}

fn schema_search() -> Value {
    object(
        [
            ("pattern", string()),
            ("path", string_default(".")),
            ("exclude", exclude_schema()),
            ("limit", integer_default(DEFAULT_LIMIT)),
            ("mode", string_enum(&["auto", "regex", "literal"], "auto")),
        ],
        &["pattern"],
    )
}

fn schema_sloc() -> Value {
    object(
        [
            (
                "path",
                describe(
                    string_default("."),
                    "Workspace path or whitespace-separated paths to count.",
                ),
            ),
            ("exclude", exclude_schema()),
        ],
        &[],
    )
}

fn schema_todo() -> Value {
    let item = todo_item_schema();
    object(
        [
            (
                "todos",
                describe(
                    array_of(item.clone()),
                    "Complete replacement todo list. Alias: items. Omit to return current list.",
                ),
            ),
            ("items", describe(array_of(item), "Alias for todos.")),
            (
                "persist",
                describe(
                    bool_default(false),
                    "Write to TODO.md; default false avoids git churn.",
                ),
            ),
        ],
        &[],
    )
}

fn schema_ask() -> Value {
    object(
        [("question", string()), ("choices", nullable_string_array())],
        &["question"],
    )
}

fn schema_webfetch() -> Value {
    object(
        [
            ("url", string()),
            ("method", string_default("GET")),
            (
                "headers",
                json!({"type": ["object", "null"], "additionalProperties": string()}),
            ),
            ("follow_redirects", bool_default(false)),
            (
                "timeout_seconds",
                integer_default(DEFAULT_WEBFETCH_TIMEOUT_SECONDS),
            ),
        ],
        &["url"],
    )
}

fn schema_replace() -> Value {
    object(
        [
            ("pattern", string()),
            ("replacement", string()),
            ("path", string_default(".")),
            ("exclude", exclude_schema()),
            ("limit", integer_default(DEFAULT_LIMIT)),
            ("mode", string_enum(&["regex", "literal"], "regex")),
        ],
        &["pattern", "replacement"],
    )
}

fn schema_bash() -> Value {
    object(
        [
            ("command", string()),
            ("timeout_seconds", integer_default(120)),
        ],
        &["command"],
    )
}

// === Invocation, summaries, and previews ===
pub fn tool_specs(ctx: &ToolContext) -> Vec<Tool> {
    TOOL_DEFS
        .iter()
        .filter(|def| tool_enabled(ctx, def))
        .map(spec)
        .collect()
}

fn parse_tool_args<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T> {
    serde_json::from_value(args).with_context(|| {
        "invalid tool arguments; use the documented argument names/types; numeric fields may be numbers or numeric strings"
    })
}

pub async fn invoke(ctx: &mut ToolContext, name: &str, args: Value) -> Result<Value> {
    note_tool(name, &args);
    let started = std::time::Instant::now();
    let result = match name {
        "list" => parse_tool_args(args).and_then(|args| tool_list(ctx, args)),
        "read" => parse_tool_args(args).and_then(|args| tool_read(ctx, args)),
        "search" => parse_tool_args(args).and_then(|args| tool_search(ctx, args)),
        "replace" => parse_tool_args(args).and_then(|args| tool_replace(ctx, args)),
        "sloc" => parse_tool_args(args).and_then(|args| tool_sloc(ctx, args)),
        "bash" => match parse_tool_args(args) {
            Ok(args) => tool_bash(ctx, args).await,
            Err(err) => Err(err),
        },
        "webfetch" => match parse_tool_args(args) {
            Ok(args) => tool_webfetch(ctx, args).await,
            Err(err) => Err(err),
        },
        "ask" => parse_tool_args(args).and_then(|args| tool_ask(ctx, args)),
        "todo" => parse_tool_args(args).and_then(|args| tool_todo(ctx, args)),
        other => bail!("unknown tool: {other}"),
    };
    if let Ok(value) = &result {
        crate::ui::tool_result(name, started.elapsed(), &preview_tool_output(name, value));
    } else if let Err(err) = &result {
        crate::ui::tool_error(name, started.elapsed(), err);
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
    compact_kvs(
        args,
        &[("pattern", 70), ("path", 50), ("mode", 12), ("exclude", 35)],
    )
}

fn summary_replace(args: &Value) -> String {
    compact_kvs(
        args,
        &[
            ("path", 45),
            ("mode", 12),
            ("pattern", 45),
            ("replacement", 45),
        ],
    )
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

fn bool_marker(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn truncation_flag(value: &Value) -> &'static str {
    bool_marker(value_bool(value, "truncated"))
}

fn verbose_preview(body: impl FnOnce() -> String) -> Option<String> {
    (!crate::ui::is_quiet()).then(body)
}

fn with_verbose(summary: String, body: impl FnOnce() -> String) -> String {
    let Some(body) = verbose_preview(body).filter(|body| !body.trim().is_empty()) else {
        return summary;
    };
    format!("{}\n{}", summary, limited_preview_body(&body))
}

fn limited_preview_body(body: &str) -> String {
    let max_lines = if crate::ui::is_verbose() {
        VERBOSE_PREVIEW_LINES
    } else {
        NORMAL_PREVIEW_LINES
    };
    crate::ui::clamp_lines(body, max_lines, PREVIEW_LINE_CHARS)
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

fn append_preview_lines(out: &mut String, text: &str, title: &str) {
    let max_lines = if crate::ui::is_verbose() {
        VERBOSE_PREVIEW_LINES
    } else {
        NORMAL_PREVIEW_LINES
    };
    let line_count = text.lines().count();
    let preview = text.lines().take(max_lines).collect::<Vec<_>>().join("\n");
    if preview.is_empty() {
        return;
    }
    let block = crate::ui::text_block(title, &preview);
    for line in block.lines() {
        let _ = write!(out, "\n{line}");
    }
    if line_count > max_lines {
        let _ = write!(out, "\n  … {} more preview lines", line_count - max_lines);
    }
}

fn preview_generic(value: &Value) -> String {
    if crate::ui::is_verbose() {
        crate::ui::clamp_lines(
            &encode_tool_output(value),
            VERBOSE_PREVIEW_LINES,
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
        "path={} · {} item{} · shown={} · truncated={}",
        value_str(value, "path"),
        total,
        plural(total),
        items.len().min(PREVIEW_ITEMS),
        truncation_flag(value)
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
    let summary = format!(
        "path={path} · lines {offset}-{end}/{line_count} · returned={shown}{more} · truncated={}",
        truncation_flag(value)
    );
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
        format!(
            "pattern=/{}/ · path={} · 0 matches · truncated={}",
            value_str(value, "pattern"),
            value_str(value, "path"),
            truncation_flag(value)
        )
    } else {
        format!(
            "pattern=/{}/ · path={} · {} {} · {} file{} · returned={} · truncated={}",
            value_str(value, "pattern"),
            value_str(value, "path"),
            total,
            if total == 1 { "match" } else { "matches" },
            files,
            plural(files),
            matches.len(),
            truncation_flag(value)
        )
    };
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
    format!(
        "{}:{}:{} {}",
        crate::ui::path(path),
        crate::ui::faint(line),
        crate::ui::faint(col),
        text
    )
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
    let summary = format!(
        "{} file{} changed · {} replacement{} · returned={} · truncated={}",
        files,
        plural(files),
        replacements,
        plural(replacements),
        changed.len(),
        truncation_flag(value)
    );
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
    let stdout = value
        .get("stdout_preview")
        .and_then(Value::as_str)
        .unwrap_or_else(|| value_str(value, "stdout"));
    let stderr = value
        .get("stderr_preview")
        .and_then(Value::as_str)
        .unwrap_or_else(|| value_str(value, "stderr"));
    let icon = if code == 0 {
        crate::ui::green("✓")
    } else {
        crate::ui::red("✗")
    };
    let mut summary = format!(
        "{icon} exit {code} · stdout {} line{} · stderr {} line{} · stdout-truncated={} · stderr-truncated={}",
        count_lines(stdout),
        plural(count_lines(stdout)),
        count_lines(stderr),
        plural(count_lines(stderr)),
        bool_marker(value_bool(value, "stdout_truncated")),
        bool_marker(value_bool(value, "stderr_truncated"))
    );
    if code != 0
        && let Some(first_stderr) = stderr.lines().find(|line| !line.trim().is_empty())
    {
        summary.push_str(&format!(
            " · {}",
            crate::ui::truncate_chars(first_stderr.trim(), 80)
        ));
    }
    with_verbose(summary, || {
        let mut out = String::new();
        for key in ["stdout", "stderr"] {
            let text = value_str(value, key);
            let truncated_key = format!("{key}_truncated");
            let truncated = value_bool(value, &truncated_key);
            if text.is_empty() {
                if truncated {
                    let _ = write!(
                        out,
                        "\n{}\n  … {key} truncated",
                        crate::ui::block_title(key)
                    );
                }
                continue;
            }
            append_preview_lines(&mut out, text, key);
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
    let text = value
        .get("text_preview")
        .and_then(Value::as_str)
        .unwrap_or_else(|| value_str(value, "text"));
    let format = value_str(value, "format");
    let kind = if format.is_empty() { "text" } else { format };
    let summary = format!(
        "HTTP {status} · {kind} · {} line{} · truncated={} · {url}",
        count_lines(text),
        plural(count_lines(text)),
        truncation_flag(value)
    );
    with_verbose(summary, || {
        let mut out = String::new();
        if !text.is_empty() {
            append_preview_lines(&mut out, text, kind);
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
    limited_preview_body(&preview)
}
fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

// === Todo formatting and persistence ===
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
    config::write_workspace_file(&path, todos_to_markdown(todos).as_bytes())
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

// === Tool implementations ===
fn tool_list(ctx: &ToolContext, args: ListArgs) -> Result<Value> {
    reject_out_of_workspace_path(&ctx.root, &args.path, None)?;
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
    let path = resolve_existing_path(ctx, &args.path)?;
    if path.is_dir() {
        bail!("read path is a directory: {}", args.path);
    }
    let Some(item) = read_text_file(&ctx.root, &path)? else {
        bail!("read path is not utf-8 text: {}", args.path);
    };
    let display_path = item.display_path;
    let text = item.text;
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
        "text": shown.join("\n"),
        "line_count": line_count,
        "truncated": truncated
    }))
}

fn search_matchers(
    pattern: &str,
    mode: SearchMode,
) -> Result<(RegexMatcher, Regex, &'static str, Value)> {
    match mode {
        SearchMode::Regex => Ok((
            RegexMatcher::new_line_matcher(pattern)
                .with_context(|| format!("invalid regex: {pattern}"))?,
            Regex::new(pattern).with_context(|| format!("invalid regex: {pattern}"))?,
            "regex",
            Value::Null,
        )),
        SearchMode::Literal => {
            let escaped = regex::escape(pattern);
            Ok((
                RegexMatcher::new_line_matcher(&escaped)?,
                Regex::new(&escaped)?,
                "literal",
                Value::Null,
            ))
        }
        SearchMode::Auto => match Regex::new(pattern) {
            Ok(regex) => Ok((
                RegexMatcher::new_line_matcher(pattern)
                    .with_context(|| format!("invalid regex: {pattern}"))?,
                regex,
                "regex",
                Value::Null,
            )),
            Err(err) => {
                let escaped = regex::escape(pattern);
                Ok((
                    RegexMatcher::new_line_matcher(&escaped)?,
                    Regex::new(&escaped)?,
                    "literal",
                    json!(format!(
                        "pattern was not valid regex; searched literally: {err}"
                    )),
                ))
            }
        },
    }
}

fn replace_matcher_and_replacement(args: &ReplaceArgs) -> Result<(Regex, String, &'static str)> {
    match args.mode {
        ReplaceMode::Regex => Ok((
            Regex::new(&args.pattern)
                .with_context(|| format!("invalid regex: {}", args.pattern))?,
            args.replacement.clone(),
            "regex",
        )),
        ReplaceMode::Literal => Ok((
            Regex::new(&regex::escape(&args.pattern))?,
            args.replacement.replace('$', "$$"),
            "literal",
        )),
    }
}

fn tool_search(ctx: &ToolContext, args: SearchArgs) -> Result<Value> {
    let (matcher, column_regex, mode, warning) = search_matchers(&args.pattern, args.mode)?;
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
        "mode": mode,
        "warning": warning,
        "path": args.path,
        "match_count": matches.len(),
        "matches": matches.iter().take(shown).cloned().collect::<Vec<_>>(),
        "truncated": matches.len() > shown,
        "exclude": args.exclude.as_ref().map(ExcludeArg::patterns),
        "errors": if errors.is_empty() { Value::Null } else { Value::Array(errors) }
    }))
}
fn tool_replace(ctx: &ToolContext, args: ReplaceArgs) -> Result<Value> {
    let (regex, replacement, mode) = replace_matcher_and_replacement(&args)?;
    let exclude = build_exclude_set(args.exclude.as_ref())?;
    let target = resolve_existing_path(ctx, &args.path)?;
    let approval_preview = if ctx.policy.approval("replace") == Approval::Ask && ctx.interactive {
        preview_replace_plan(ctx, &args, &regex, &replacement, &target, &exclude).ok()
    } else {
        None
    };
    require_mutation_approval(ctx, "replace", approval_preview.as_deref())?;
    let mut changed_files = Vec::new();
    let mut skipped = Vec::new();
    let mut errors = Vec::new();
    let mut replacement_count = 0usize;
    for path in walk_files(&ctx.root, &target, &exclude)? {
        match replace_file(&path, &regex, &replacement) {
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
        "mode": mode,
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
    let targets = resolve_existing_paths(ctx, &args.path)?;
    let exclude = args
        .exclude
        .as_ref()
        .map(ExcludeArg::patterns)
        .unwrap_or_default();
    let targets = targets
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    let target_refs = targets.iter().map(String::as_str).collect::<Vec<_>>();
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
    languages.get_statistics(&target_refs, &excluded, &config);
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

async fn tool_bash(ctx: &ToolContext, args: BashArgs) -> Result<Value> {
    if args.command.len() > config::max_bash_cmd_bytes() {
        bail!("command too large ({} bytes)", args.command.len());
    }
    let timeout_seconds = args.timeout_seconds.clamp(1, MAX_BASH_TIMEOUT_SECONDS);
    let approval_preview = format!(
        "workspace: {}\ntimeout: {timeout_seconds}s\ncommand:\n{}",
        ctx.root.display(),
        args.command.trim()
    );
    require_mutation_approval(ctx, "bash", Some(&approval_preview))?;
    let mut child = Command::new("bash")
        .arg("-c")
        .arg(&args.command)
        .current_dir(&ctx.root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child.stdout.take().context("failed to capture stdout")?;
    let stderr = child.stderr.take().context("failed to capture stderr")?;
    let stdout_task = tokio::spawn(read_child_output(stdout, MAX_BASH_OUTPUT_BYTES));
    let stderr_task = tokio::spawn(read_child_output(stderr, MAX_BASH_OUTPUT_BYTES));
    let status = match timeout(Duration::from_secs(timeout_seconds), child.wait()).await {
        Ok(status) => status?,
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            bail!("bash timed out after {timeout_seconds}s");
        }
    };
    let (stdout, stdout_truncated) = stdout_task.await??;
    let (stderr, stderr_truncated) = stderr_task.await??;
    let (stdout_preview, stdout_preview_truncated) = crate::ui::head_tail(&stdout, 12_000);
    let (stderr_preview, stderr_preview_truncated) = crate::ui::head_tail(&stderr, 8_000);
    Ok(json!({
        "command": args.command,
        "returncode": status.code().unwrap_or(-1),
        "stdout": stdout,
        "stderr": stderr,
        "stdout_preview": stdout_preview,
        "stderr_preview": stderr_preview,
        "stdout_truncated": stdout_truncated || stdout_preview_truncated,
        "stderr_truncated": stderr_truncated || stderr_preview_truncated,
        "stdout_capped": stdout_truncated,
        "stderr_capped": stderr_truncated
    }))
}

async fn read_child_output<R>(mut reader: R, max_bytes: usize) -> Result<(String, bool)>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut out = Vec::new();
    let mut truncated = false;
    let mut buf = [0u8; 1024];
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        let remaining = max_bytes.saturating_sub(out.len());
        if n > remaining {
            out.extend_from_slice(&buf[..remaining]);
            truncated = true;
        } else if remaining > 0 {
            out.extend_from_slice(&buf[..n]);
        } else {
            truncated = true;
        }
    }
    Ok((String::from_utf8_lossy(&out).to_string(), truncated))
}

async fn tool_webfetch(ctx: &ToolContext, args: WebfetchArgs) -> Result<Value> {
    let _ = ctx;
    let method = args.method.to_ascii_uppercase();
    if !matches!(method.as_str(), "GET" | "HEAD" | "OPTIONS") {
        bail!("Only GET/HEAD/OPTIONS are allowed, got {method}");
    }
    let url = validate_public_url(&args.url).await?;
    let resolved = public_socket_addrs(&url).await?;
    let client = webfetch_client(&url, &resolved, args.timeout_seconds)?;
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
    let mut response = request.send().await?;
    if args.follow_redirects {
        response = follow_public_redirects(&client, response, &method).await?;
    }
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

    let (body, body_capped) = read_limited_response(response, MAX_WEBFETCH_BYTES).await?;
    if is_text_content_type(&content_type) {
        let text = String::from_utf8_lossy(&body).to_string();
        let normalized = if content_type.contains("text/html")
            || text.trim_start().starts_with("<!DOCTYPE html")
            || text.trim_start().starts_with("<html")
        {
            html2md::parse_html(&text)
        } else {
            text
        };
        let (text_preview, preview_truncated) = crate::ui::head_tail(&normalized, 12_000);
        let truncated = preview_truncated || body_capped;
        return Ok(json!({
            "method": method,
            "url": final_url,
            "status_code": status.as_u16(),
            "reason_phrase": reason_phrase(status),
            "http_version": format!("{:?}", version),
            "headers": header_map,
            "text": normalized,
            "text_preview": text_preview,
            "format": if content_type.contains("html") { "markdown" } else { "text" },
            "truncated": truncated,
            "body_capped": body_capped
        }));
    }

    let bytes = body;
    Ok(json!({
        "method": method,
        "url": final_url,
        "status_code": status.as_u16(),
        "reason_phrase": reason_phrase(status),
        "http_version": format!("{:?}", version),
        "headers": header_map,
        "binary": true,
        "content_bytes": bytes.len(),
        "truncated": body_capped
    }))
}
async fn read_limited_response(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<(Vec<u8>, bool)> {
    let mut stream = response.bytes_stream();
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        let remaining = max_bytes.saturating_sub(out.len());
        if chunk.len() > remaining {
            out.extend_from_slice(&chunk[..remaining]);
            return Ok((out, true));
        }
        out.extend_from_slice(&chunk);
        if out.len() >= max_bytes {
            return Ok((out, true));
        }
    }
    Ok((out, false))
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
    if !args.todos.is_empty() {
        require_mutation_approval(ctx, "todo", Some("update the in-memory todo list"))?;
    }
    if args.persist {
        require_mutation_approval(ctx, "todo_persist", Some("write TODO.md in the workspace"))?;
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

// === Workspace filesystem boundary ===
fn reject_out_of_workspace_path(root: &Path, path: &str, resolved: Option<&Path>) -> Result<()> {
    let raw = Path::new(path);
    if raw.is_absolute() {
        bail!("path outside workspace is not allowed: {path} (absolute path)");
    }
    if raw.components().any(|c| matches!(c, Component::ParentDir)) {
        bail!("path outside workspace is not allowed: {path} (parent-directory path)");
    }
    if let Some(resolved) = resolved.filter(|resolved| !within_root(root, resolved)) {
        bail!(
            "path outside workspace is not allowed: {path} -> {}",
            resolved.display()
        );
    }
    Ok(())
}

fn resolve_existing_path(ctx: &ToolContext, path: &str) -> Result<PathBuf> {
    reject_out_of_workspace_path(&ctx.root, path, None)?;
    let joined = ctx.root.join(path);
    let resolved = joined
        .canonicalize()
        .with_context(|| format!("path does not exist: {path}"))?;
    reject_out_of_workspace_path(&ctx.root, path, Some(&resolved))?;
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

fn read_text_file(root: &Path, path: &Path) -> Result<Option<SearchText>> {
    let rel = rel_path(root, path);
    let raw = fs::read(path)?;
    Ok(crate::decode_utf8(raw).ok().map(|text| SearchText {
        display_path: rel,
        text,
    }))
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
    if let Some(item) = read_text_file(root, path)? {
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
    let text = match crate::decode_utf8(raw) {
        Ok(text) => text,
        Err(crate::TextDecodeError::Binary) => {
            return Ok(ReplaceOutcome::Skipped("binary file"));
        }
        Err(crate::TextDecodeError::NonUtf8) => bail!("cannot decode utf-8"),
    };
    let count = regex.find_iter(&text).count();
    if count == 0 {
        return Ok(ReplaceOutcome::Unchanged);
    }
    let updated = regex.replace_all(&text, replacement).into_owned();
    let diff = unified_diff(&path.to_string_lossy(), &text, &updated);
    config::write_workspace_file(path, updated.as_bytes())?;
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
    replacement: &str,
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
        let Ok(text) = crate::decode_utf8(raw) else {
            continue;
        };
        if !regex.is_match(&text) {
            continue;
        }
        let updated = regex.replace_all(&text, replacement).into_owned();
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

// === Public network boundary ===
async fn follow_public_redirects(
    initial_client: &reqwest::Client,
    mut response: reqwest::Response,
    method: &str,
) -> Result<reqwest::Response> {
    let _ = initial_client;
    for _ in 0..10 {
        if !response.status().is_redirection() {
            return Ok(response);
        }
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .context("redirect missing valid Location header")?;
        let next_url = response.url().join(location)?;
        validate_public_url_parts(&next_url)?;
        let resolved = public_socket_addrs(&next_url).await?;
        let client = webfetch_client(&next_url, &resolved, MAX_WEBFETCH_TIMEOUT_SECONDS)?;
        response = client.request(method.parse()?, next_url).send().await?;
    }
    bail!("too many redirects")
}

fn webfetch_client(
    url: &Url,
    resolved: &[SocketAddr],
    timeout_seconds: u64,
) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(
            timeout_seconds.clamp(1, MAX_WEBFETCH_TIMEOUT_SECONDS),
        ))
        .resolve_to_addrs(url.host_str().context("missing hostname")?, resolved)
        .build()?)
}

async fn validate_public_url(input: &str) -> Result<Url> {
    let url = Url::parse(input).with_context(|| format!("invalid URL: {input}"))?;
    validate_public_url_parts(&url)?;
    let _ = public_socket_addrs(&url).await?;
    Ok(url)
}

fn validate_public_url_parts(url: &Url) -> Result<()> {
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
    if let Ok(ip) = host.parse::<IpAddr>() {
        ensure_public_ip(ip)?;
    }
    Ok(())
}

async fn public_socket_addrs(url: &Url) -> Result<Vec<SocketAddr>> {
    validate_public_url_parts(url)?;
    let host = url.host_str().context("missing hostname")?;
    let port = url.port_or_known_default().unwrap_or(80);
    let addrs = lookup_host((host, port)).await?.collect::<Vec<_>>();
    if addrs.is_empty() {
        bail!("URL host resolved to no addresses: {host}");
    }
    for addr in &addrs {
        ensure_public_ip(addr.ip())?;
    }
    Ok(addrs)
}

fn ensure_public_ip(ip: IpAddr) -> Result<()> {
    if is_public_ip(ip) {
        Ok(())
    } else {
        bail!("URL resolves to non-public address ({ip})")
    }
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

// === Mutation approval boundary ===
fn require_mutation_approval(ctx: &ToolContext, tool: &str, preview: Option<&str>) -> Result<()> {
    match ctx.policy.approval(tool) {
        Approval::Auto => Ok(()),
        Approval::Deny => bail!("tool denied by policy: {tool}"),
        Approval::Ask if !ctx.interactive => bail!(
            "tool denied by policy: {tool} requires interactive approval or an auto-approve mode"
        ),
        Approval::Ask => approve_tool(tool, preview),
    }
}

fn approval_display_name(tool: &str) -> &str {
    match tool {
        "todo_persist" => "todo",
        other => other,
    }
}

fn approve_tool(tool: &str, preview: Option<&str>) -> Result<()> {
    let display_tool = approval_display_name(tool);
    if let Some(preview) = preview.filter(|s| !s.trim().is_empty()) {
        crate::ui::err_line(crate::ui::diff(preview).trim_end());
    }
    crate::ui::section("Approval required");
    crate::ui::kv("tool", display_tool);
    crate::ui::kv("default", "deny");
    if tool == "bash" {
        crate::ui::warn("shell commands run with your user permissions and inherited environment");
    }
    let choices = ["no".to_string(), "yes".to_string()];
    if crate::chat::ask(&format!("Approve {display_tool}?"), Some(&choices))? == "yes" {
        Ok(())
    } else {
        bail!("tool denied by user")
    }
}

// === Tests ===
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

    fn schema_for(name: &str) -> Value {
        let (_dir, ctx) = test_context(auto_policy(), true);
        tool_specs(&ctx)
            .into_iter()
            .find(|tool| tool.name.as_str() == name)
            .and_then(|tool| tool.schema)
            .unwrap_or_else(|| panic!("missing schema for {name}"))
    }

    #[test]
    fn tool_schemas_are_closed_objects_with_valid_required_fields() {
        let (_dir, ctx) = test_context(auto_policy(), true);
        for tool in tool_specs(&ctx) {
            let schema = tool
                .schema
                .unwrap_or_else(|| panic!("missing schema for {}", tool.name));
            assert_eq!(schema["type"], "object", "{} type", tool.name);
            assert_eq!(
                schema["additionalProperties"], false,
                "{} additionalProperties",
                tool.name
            );
            let props = schema["properties"]
                .as_object()
                .unwrap_or_else(|| panic!("missing properties for {}", tool.name));
            if let Some(required) = schema.get("required").and_then(Value::as_array) {
                for field in required {
                    let field = field.as_str().unwrap();
                    assert!(
                        props.contains_key(field),
                        "{} requires unknown {field}",
                        tool.name
                    );
                }
            }
        }
    }

    #[test]
    fn tool_schema_helpers_preserve_aliases_defaults_and_nullable_shapes() {
        let todo = schema_for("todo");
        assert_eq!(todo["properties"]["persist"]["default"], false);
        assert_eq!(
            todo["properties"]["items"]["items"]["required"],
            json!(["task"])
        );
        assert_eq!(
            todo["properties"]["todos"]["description"],
            "Complete replacement todo list. Alias: items. Omit to return current list."
        );

        let list = schema_for("list");
        assert_eq!(list["properties"]["path"]["default"], "*");
        assert_eq!(list["properties"]["exclude"]["anyOf"][1]["items"], string());

        let webfetch = schema_for("webfetch");
        assert_eq!(webfetch["required"], json!(["url"]));
        assert_eq!(
            webfetch["properties"]["headers"]["type"],
            json!(["object", "null"])
        );
    }

    #[test]
    fn schemas_document_lenient_numbers_and_match_modes() {
        let search = schema_for("search");
        assert_eq!(
            search["properties"]["limit"]["type"],
            json!(["integer", "string"])
        );
        assert_eq!(
            search["properties"]["mode"]["enum"],
            json!(["auto", "regex", "literal"])
        );

        let replace = schema_for("replace");
        assert_eq!(
            replace["properties"]["mode"]["enum"],
            json!(["regex", "literal"])
        );

        let bash = schema_for("bash");
        assert_eq!(
            bash["properties"]["timeout_seconds"]["type"],
            json!(["integer", "string"])
        );
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
                mode: ReplaceMode::Regex,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("requires interactive approval"));
        assert_eq!(fs::read_to_string(dir.path().join("a.txt")).unwrap(), "one");
    }

    #[test]
    fn read_only_allows_todo_memory_but_denies_persistence() {
        let (_dir, mut ctx) = test_context(ToolPolicy::read_only(), false);
        let value = tool_todo(
            &mut ctx,
            TodoArgs {
                todos: vec![TodoItemInput {
                    id: None,
                    task: "plan work".into(),
                    status: "pending".into(),
                }],
                persist: false,
            },
        )
        .unwrap();
        assert_eq!(value["count"], 1);
        assert_eq!(ctx.todos[0].task, "plan work");

        let err = tool_todo(
            &mut ctx,
            TodoArgs {
                todos: Vec::new(),
                persist: true,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("tool denied by policy"));
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
    fn file_tools_deny_out_of_workspace_paths_in_all_modes() {
        for policy in [
            ToolPolicy {
                read_only: false,
                files_write: Approval::Ask,
                shell: Approval::Ask,
                network: true,
            },
            auto_policy(),
            ToolPolicy::read_only(),
        ] {
            let (_dir, ctx) = test_context(policy, false);
            let err = tool_read(
                &ctx,
                ReadArgs {
                    path: "/etc/hosts".into(),
                    offset: 1,
                    limit: 1,
                },
            )
            .unwrap_err();
            assert!(err.to_string().contains("path outside workspace"));
        }
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
                mode: ReplaceMode::Regex,
            },
        )
        .unwrap();
        assert_eq!(value["replacement_count"], 1);
        assert_eq!(fs::read_to_string(dir.path().join("a.txt")).unwrap(), "two");
    }

    #[test]
    fn read_only_exposes_research_tools_but_not_mutation_tools() {
        let (_dir, ctx) = test_context(ToolPolicy::read_only(), false);
        let names = tool_specs(&ctx)
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        for expected in ["list", "read", "search", "sloc", "webfetch", "todo"] {
            assert!(
                names.iter().any(|name| name.as_str() == expected),
                "missing {expected}"
            );
        }
        for denied in ["replace", "bash"] {
            assert!(
                !names.iter().any(|name| name.as_str() == denied),
                "exposed {denied}"
            );
        }
    }

    #[test]
    fn sloc_accepts_space_separated_paths() {
        let (dir, ctx) = test_context(auto_policy(), false);
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/app.rs"), "fn app() {}\n").unwrap();
        fs::write(dir.path().join("README.md"), "# docs\n").unwrap();
        fs::write(dir.path().join("ignored.rs"), "fn ignored() {}\n").unwrap();

        let value = tool_sloc(
            &ctx,
            SlocArgs {
                path: "src README.md".into(),
                exclude: None,
            },
        )
        .unwrap();

        assert_eq!(value["path"], "src README.md");
        assert_eq!(value["output"]["Rust"]["code"], 1);
        assert_eq!(value["output"]["Markdown"]["comments"], 1);
        assert!(value["output"]["Total"].is_object());
    }

    #[test]
    fn search_accepts_space_separated_paths() {
        let (dir, ctx) = test_context(auto_policy(), false);
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/app.rs"), "fn app_hit() {}\n").unwrap();
        fs::write(dir.path().join("src/ui.rs"), "fn ui_hit() {}\n").unwrap();
        fs::write(dir.path().join("src/other.rs"), "fn other_hit() {}\n").unwrap();

        let value = tool_search(
            &ctx,
            SearchArgs {
                pattern: "fn (app|ui)_hit".into(),
                path: "src/app.rs src/ui.rs".into(),
                exclude: None,
                limit: 10,
                mode: SearchMode::Regex,
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
        assert!(paths.iter().any(|path| path == "src/app.rs"));
        assert!(paths.iter().any(|path| path == "src/ui.rs"));
        assert!(!paths.iter().any(|path| path == "src/other.rs"));
    }

    #[test]
    fn search_auto_falls_back_to_literal_for_invalid_regex() {
        let (dir, ctx) = test_context(auto_policy(), false);
        fs::write(
            dir.path().join("notes.txt"),
            "literal [text
",
        )
        .unwrap();

        let value = tool_search(
            &ctx,
            SearchArgs {
                pattern: "[text".into(),
                path: "notes.txt".into(),
                exclude: None,
                limit: 10,
                mode: SearchMode::Auto,
            },
        )
        .unwrap();

        assert_eq!(value["mode"], "literal");
        assert_eq!(value["match_count"], 1);
        assert!(
            value["warning"]
                .as_str()
                .unwrap()
                .contains("searched literally")
        );
    }

    #[test]
    fn replace_literal_treats_pattern_and_dollars_as_plain_text() {
        let (dir, ctx) = test_context(auto_policy(), false);
        fs::write(
            dir.path().join("a.txt"),
            "a+b $1
",
        )
        .unwrap();

        let value = tool_replace(
            &ctx,
            ReplaceArgs {
                pattern: "a+b".into(),
                replacement: "$1".into(),
                path: "a.txt".into(),
                exclude: None,
                limit: 10,
                mode: ReplaceMode::Literal,
            },
        )
        .unwrap();

        assert_eq!(value["mode"], "literal");
        assert_eq!(value["replacement_count"], 1);
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "$1 $1
"
        );
    }

    #[tokio::test]
    async fn invoke_accepts_numeric_strings_and_aliases() {
        let (dir, mut ctx) = test_context(auto_policy(), false);
        fs::write(
            dir.path().join("a.txt"),
            "one
two
three
",
        )
        .unwrap();

        let value = invoke(
            &mut ctx,
            "read",
            json!({"file": "a.txt", "start": "2", "lines": "1"}),
        )
        .await
        .unwrap();

        assert_eq!(value["offset"], 2);
        assert_eq!(value["limit"], 1);
        assert_eq!(value["text"], "two");
    }

    #[tokio::test]
    async fn bash_returns_full_output_and_bounded_preview() {
        let (_dir, ctx) = test_context(auto_policy(), false);
        let value = tool_bash(
            &ctx,
            BashArgs {
                command: "python3 - <<'PY'
print('x' * 13000)
PY"
                .into(),
                timeout_seconds: 5,
            },
        )
        .await
        .unwrap();

        assert_eq!(value["returncode"], 0);
        assert!(value["stdout"].as_str().unwrap().len() > 12_000);
        assert!(
            value["stdout_preview"].as_str().unwrap().len()
                < value["stdout"].as_str().unwrap().len()
        );
        assert_eq!(value["stdout_truncated"], true);
        assert_eq!(value["stdout_capped"], false);
    }

    #[test]
    fn search_file_treats_zip_as_binary_file() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("sample.zip");
        fs::write(&zip_path, b"PK\0\0not searched").unwrap();
        let matcher = RegexMatcher::new_line_matcher("not searched").unwrap();
        let column_regex = Regex::new("not searched").unwrap();
        let found = search_file(dir.path(), &zip_path, &matcher, &column_regex).unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn read_rejects_zip_virtual_member() {
        let (dir, ctx) = test_context(auto_policy(), false);
        fs::write(dir.path().join("sample.zip"), b"PK\0\0").unwrap();
        let err = tool_read(
            &ctx,
            ReadArgs {
                path: "sample.zip::docs/readme.txt".into(),
                offset: 1,
                limit: 10,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("path does not exist"));
    }

    #[test]
    fn list_does_not_expand_zip_members() {
        let (dir, ctx) = test_context(auto_policy(), false);
        fs::write(dir.path().join("sample.zip"), b"PK\0\0").unwrap();
        let value = tool_list(
            &ctx,
            ListArgs {
                path: "sample.zip".into(),
                exclude: None,
                limit: 10,
            },
        )
        .unwrap();
        assert_eq!(value["count"], 1);
        let items = value["items"].as_array().unwrap();
        assert_eq!(items, &vec![json!("sample.zip")]);
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
