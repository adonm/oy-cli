use rig::completion::ToolDefinition;
use rig::tool::{ToolDyn, ToolError};
use serde_json::Value;
use std::sync::{Arc, Mutex};

use super::ToolContext;
use super::policy::{Approval, NetworkAccess};

// === Tool dispatch registry ===
//
// Adding a new tool:
// 1. Add its entry to `TOOL_DEFS` below (name, gate, schema fn, summary fn, output fn)
// 2. Add the tool implementation in the appropriate module
// 3. Add the invoke dispatch in `tools.rs::invoke()`
// 4. Done — schema exposure, preview rendering, and policy gating are all driven from here.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolGate {
    Always,
    Interactive,
    Network,
    FilesWrite,
    Shell,
}

/// A tool's definition: everything needed to expose it to the model and render results.
#[derive(Clone, Copy)]
pub(super) struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub gate: ToolGate,
    pub schema: fn() -> Value,
    pub summary: fn(&Value) -> String,
    pub output: fn(&Value) -> String,
}

/// Look up a tool definition by name.
pub(super) fn find_def(name: &str) -> Option<&'static ToolDef> {
    TOOL_DEFS.iter().find(|def| def.name == name)
}

// Import preview functions so we can reference them in TOOL_DEFS.
use super::preview;

const TOOL_DEFS: &[ToolDef] = &[
    ToolDef {
        name: "list",
        description: "List workspace paths. Use first for discovery. `path` is a workspace-relative glob and defaults to `*`. Returns items, count, and truncation state.",
        gate: ToolGate::Always,
        schema: super::schema::schema_list,
        summary: preview::summary_list,
        output: preview::preview_list,
    },
    ToolDef {
        name: "read",
        description: "Read one UTF-8 text file. Prefer narrow `offset`/`limit` slices over full-file reads.",
        gate: ToolGate::Always,
        schema: super::schema::schema_read,
        summary: preview::summary_read,
        output: preview::preview_read,
    },
    ToolDef {
        name: "search",
        description: "Search workspace text with ripgrep-style Rust regex. Use `mode=literal` for exact strings.",
        gate: ToolGate::Always,
        schema: super::schema::schema_search,
        summary: preview::summary_search,
        output: preview::preview_search,
    },
    ToolDef {
        name: "sloc",
        description: "Count source lines with tokei for repository sizing. `path` may be one path or whitespace-separated paths.",
        gate: ToolGate::Always,
        schema: super::schema::schema_sloc,
        summary: preview::summary_sloc,
        output: preview::preview_sloc,
    },
    ToolDef {
        name: "todo",
        description: "Manage the in-memory todo list. Available in read-only modes; persistence to TODO.md is opt-in and requires write approval.",
        gate: ToolGate::Always,
        schema: super::schema::schema_todo,
        summary: preview::summary_todo,
        output: preview::preview_todo,
    },
    ToolDef {
        name: "ask",
        description: "Ask the user in interactive runs. Reserve for genuine ambiguity or irreversible choices.",
        gate: ToolGate::Interactive,
        schema: super::schema::schema_ask,
        summary: preview::summary_ask,
        output: preview::preview_ask,
    },
    ToolDef {
        name: "webfetch",
        description: "Fetch public web pages/files. Follows public redirects by default; blocks localhost/private IPs and sensitive headers.",
        gate: ToolGate::Network,
        schema: super::schema::schema_webfetch,
        summary: preview::summary_webfetch,
        output: preview::preview_webfetch,
    },
    ToolDef {
        name: "replace",
        description: "Replace workspace text with Rust regex captures, or exact text with `mode=literal`. Inspect/search before changing.",
        gate: ToolGate::FilesWrite,
        schema: super::schema::schema_replace,
        summary: preview::summary_replace,
        output: preview::preview_replace,
    },
    ToolDef {
        name: "patch",
        description: "Apply a unified/git diff to existing UTF-8 workspace files. Use for coordinated multi-file edits; inspect first and keep patches focused.",
        gate: ToolGate::FilesWrite,
        schema: super::schema::schema_patch,
        summary: preview::summary_patch,
        output: preview::preview_patch,
    },
    ToolDef {
        name: "bash",
        description: "Run a shell command in the workspace. Use only when file tools are insufficient or when you must run/check something.",
        gate: ToolGate::Shell,
        schema: super::schema::schema_bash,
        summary: preview::summary_bash,
        output: preview::preview_bash,
    },
];

fn tool_enabled(ctx: &ToolContext, def: &ToolDef) -> bool {
    match def.gate {
        ToolGate::Always => true,
        ToolGate::Interactive => ctx.interactive,
        ToolGate::Network => ctx.policy.network == NetworkAccess::Enabled,
        ToolGate::FilesWrite => ctx.policy.files_write() != Approval::Deny,
        ToolGate::Shell => ctx.policy.shell != Approval::Deny,
    }
}

fn spec(def: &ToolDef) -> ToolDefinition {
    ToolDefinition {
        name: def.name.to_string(),
        description: def.description.to_string(),
        parameters: (def.schema)(),
    }
}

#[cfg(test)]
pub(crate) fn tool_specs(ctx: &ToolContext) -> Vec<ToolDefinition> {
    enabled_tool_defs(ctx).into_iter().map(spec).collect()
}

pub(crate) fn rig_tools(ctx: Arc<Mutex<ToolContext>>) -> Vec<Box<dyn ToolDyn>> {
    let defs = {
        let ctx = ctx.lock().expect("tool context mutex poisoned");
        enabled_tool_defs(&ctx)
    };
    defs.into_iter()
        .map(|def| {
            Box::new(OyTool {
                def,
                ctx: ctx.clone(),
            }) as Box<dyn ToolDyn>
        })
        .collect()
}

fn enabled_tool_defs(ctx: &ToolContext) -> Vec<&'static ToolDef> {
    TOOL_DEFS
        .iter()
        .filter(|def| tool_enabled(ctx, def))
        .collect()
}

#[derive(Clone)]
struct OyTool {
    def: &'static ToolDef,
    ctx: Arc<Mutex<ToolContext>>,
}

impl ToolDyn for OyTool {
    fn name(&self) -> String {
        self.def.name.to_string()
    }

    fn definition<'a>(
        &'a self,
        _prompt: String,
    ) -> rig::wasm_compat::WasmBoxedFuture<'a, ToolDefinition> {
        Box::pin(async move { spec(self.def) })
    }

    fn call<'a>(
        &'a self,
        args: String,
    ) -> rig::wasm_compat::WasmBoxedFuture<'a, std::result::Result<String, ToolError>> {
        Box::pin(async move { call_tool(self.ctx.clone(), self.def.name, args).await })
    }
}

async fn call_tool(
    ctx: Arc<Mutex<ToolContext>>,
    name: &str,
    args: String,
) -> std::result::Result<String, ToolError> {
    let args = serde_json::from_str::<Value>(&args).map_err(ToolError::JsonError)?;
    let value = super::invoke_shared(ctx, name, args)
        .await
        .map_err(|err| ToolError::ToolCallError(err.into()))?;
    Ok(super::encode_tool_output(&value))
}
