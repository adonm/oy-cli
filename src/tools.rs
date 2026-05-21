//! Tool schema registry, dispatch, previews, todos, and the
//! filesystem/network/mutation approval boundaries.
//!
//! All tool capability is defined in this module and its children.
//! [`ToolContext`] carries approval policy and mutable state through
//! every invocation; the registry in [`registry`] is the single source
//! of tool schemas visible to the model.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

mod args;
mod llm;
mod network;
mod output;
mod policy;
mod preview;
mod registry;
mod schema;
mod shell;
#[cfg(test)]
mod tests;
mod todo;
mod workspace;

use args::AskArgs;
pub(crate) use llm::llm_tools;
use output::note_tool;
pub(crate) use output::{encode_tool_output, preview_tool_output};
pub(crate) use policy::{
    Approval, FileAccess, NetworkAccess, ToolPolicy, require_mutation_approval,
};
pub(crate) use registry::tool_specs;
use shell::tool_bash;

// === Public tool types and constants ===
pub const DEFAULT_LIMIT: usize = 2000;
const TODO_FILE: &str = "TODO.md";
const PREVIEW_ITEMS: usize = 40;
const NORMAL_PREVIEW_LINES: usize = 12;
const VERBOSE_PREVIEW_LINES: usize = 50;
const PREVIEW_LINE_CHARS: usize = 240;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    #[default]
    Pending,
    InProgress,
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub task: String,
    #[serde(default)]
    pub status: TodoStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolContext {
    pub root: PathBuf,
    pub interactive: bool,
    pub policy: ToolPolicy,
    pub todos: Vec<TodoItem>,
    pub external_side_effects: bool,
}

// === Invocation, summaries, and previews ===
fn parse_tool_args<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T> {
    serde_json::from_value(args).with_context(|| {
        "invalid tool arguments; use the documented argument names/types; numeric fields may be numbers or numeric strings"
    })
}

#[cfg(test)]
pub async fn invoke(ctx: &mut ToolContext, name: &str, args: Value) -> Result<Value> {
    invoke_inner(ctx, name, args).await
}

pub async fn invoke_shared(
    shared: Arc<Mutex<ToolContext>>,
    name: &str,
    args: Value,
) -> Result<Value> {
    let mut ctx = shared.lock().expect("tool context mutex poisoned").clone();
    let result = invoke_inner(&mut ctx, name, args).await;
    let mut shared = shared.lock().expect("tool context mutex poisoned");
    if result.is_ok() {
        shared.todos = ctx.todos;
    }
    shared.external_side_effects |= ctx.external_side_effects;
    result
}

async fn invoke_inner(ctx: &mut ToolContext, name: &str, args: Value) -> Result<Value> {
    note_tool(name, &args);
    if tool_may_have_external_side_effect(name, &args) {
        ctx.external_side_effects = true;
    }
    let started = std::time::Instant::now();
    let result = match name {
        "list" => parse_tool_args(args).and_then(|args| workspace::tool_list(ctx, args)),
        "read" => parse_tool_args(args).and_then(|args| workspace::tool_read(ctx, args)),
        "search" => parse_tool_args(args).and_then(|args| workspace::tool_search(ctx, args)),
        "replace" => parse_tool_args(args).and_then(|args| workspace::tool_replace(ctx, args)),
        "patch" => parse_tool_args(args).and_then(|args| workspace::tool_patch(ctx, args)),
        "sloc" => parse_tool_args(args).and_then(|args| workspace::tool_sloc(ctx, args)),
        "bash" => match parse_tool_args(args) {
            Ok(args) => tool_bash(ctx, args).await,
            Err(err) => Err(err),
        },
        "webfetch" => match parse_tool_args(args) {
            Ok(args) => network::tool_webfetch(ctx, args).await,
            Err(err) => Err(err),
        },
        "ask" => parse_tool_args(args).and_then(|args| tool_ask(ctx, args)),
        "todo" => parse_tool_args(args).and_then(|args| todo::tool_todo(ctx, args)),
        other => bail!("unknown tool: {other}"),
    };
    if let Ok(value) = &result {
        crate::ui::tool_result(name, started.elapsed(), &preview_tool_output(name, value));
    } else if let Err(err) = &result {
        crate::ui::tool_error(name, started.elapsed(), err);
    }
    result
}

fn tool_may_have_external_side_effect(name: &str, args: &Value) -> bool {
    match name {
        "bash" | "replace" | "patch" => true,
        "todo" => args
            .get("persist")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        _ => false,
    }
}

pub(crate) use todo::format_todos;

// === Process and interactive tool implementations ===
fn tool_ask(ctx: &ToolContext, args: AskArgs) -> Result<Value> {
    if !ctx.interactive {
        bail!("Cannot ask: interactive prompting is unavailable");
    }
    Ok(Value::String(crate::chat::ask(
        &args.question,
        args.choices.as_deref(),
    )?))
}
