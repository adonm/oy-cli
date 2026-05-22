//! Tool schema registry, dispatch, previews, todos, and the
//! filesystem/network/mutation approval boundaries.
//!
//! All tool capability is defined in this module and its children.
//! [`ToolContext`] carries immutable tool environment plus explicit mutable
//! state through every invocation; the registry in [`registry`] is the single
//! source of tool schemas, dispatch, previews, gates, and effect classification.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

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
pub(crate) use output::{encode_tool_output, preview_tool_output};
pub(crate) use policy::{
    Approval, FileAccess, NetworkAccess, ToolPolicy, require_mutation_approval,
};
pub(crate) use registry::tool_specs;
#[cfg(test)]
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
pub struct ToolEnv {
    pub root: PathBuf,
    pub interactive: bool,
    pub policy: ToolPolicy,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolState {
    pub todos: Vec<TodoItem>,
    pub external_side_effects: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolContext {
    pub env: ToolEnv,
    pub state: ToolState,
}

impl ToolContext {
    pub fn new(root: PathBuf, interactive: bool, policy: ToolPolicy, todos: Vec<TodoItem>) -> Self {
        Self {
            env: ToolEnv {
                root,
                interactive,
                policy,
            },
            state: ToolState {
                todos,
                external_side_effects: false,
            },
        }
    }

    pub fn root(&self) -> &std::path::Path {
        &self.env.root
    }

    pub fn interactive(&self) -> bool {
        self.env.interactive
    }

    pub fn policy(&self) -> ToolPolicy {
        self.env.policy
    }

    pub fn todos(&self) -> &[TodoItem] {
        &self.state.todos
    }

    pub fn todos_mut(&mut self) -> &mut Vec<TodoItem> {
        &mut self.state.todos
    }

    pub fn mark_external_side_effect(&mut self) {
        self.state.external_side_effects = true;
    }

    #[cfg(test)]
    pub fn external_side_effects(&self) -> bool {
        self.state.external_side_effects
    }
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
    let mut ctx = shared.lock().await;
    invoke_inner(&mut ctx, name, args).await
}

async fn invoke_inner(ctx: &mut ToolContext, name: &str, args: Value) -> Result<Value> {
    let def = registry::find_def(name).ok_or_else(|| anyhow::anyhow!("unknown tool: {name}"))?;
    output::note_tool(def.name(), &args);
    if (def.external_side_effect)(&args) {
        ctx.mark_external_side_effect();
    }
    let started = std::time::Instant::now();
    let result = def.executor.invoke(ctx, args).await;
    if let Ok(value) = &result {
        crate::ui::tool_result(
            def.name(),
            started.elapsed(),
            &preview_tool_output(def.name(), value),
        );
    } else if let Err(err) = &result {
        crate::ui::tool_error(def.name(), started.elapsed(), err);
    }
    result
}

pub(crate) use todo::format_todos;

// === Process and interactive tool implementations ===
fn tool_ask(ctx: &ToolContext, args: AskArgs) -> Result<Value> {
    if !ctx.interactive() {
        bail!("Cannot ask: interactive prompting is unavailable");
    }
    Ok(Value::String(crate::chat::ask(
        &args.question,
        args.choices.as_deref(),
    )?))
}
