use genai::chat::Tool;
use serde_json::Value;

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
pub(super) struct ToolDef {
    pub name: &'static str,
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
        gate: ToolGate::Always,
        schema: super::schema::schema_list,
        summary: preview::summary_list,
        output: preview::preview_list,
    },
    ToolDef {
        name: "read",
        gate: ToolGate::Always,
        schema: super::schema::schema_read,
        summary: preview::summary_read,
        output: preview::preview_read,
    },
    ToolDef {
        name: "search",
        gate: ToolGate::Always,
        schema: super::schema::schema_search,
        summary: preview::summary_search,
        output: preview::preview_search,
    },
    ToolDef {
        name: "sloc",
        gate: ToolGate::Always,
        schema: super::schema::schema_sloc,
        summary: preview::summary_sloc,
        output: preview::preview_sloc,
    },
    ToolDef {
        name: "todo",
        gate: ToolGate::Always,
        schema: super::schema::schema_todo,
        summary: preview::summary_todo,
        output: preview::preview_todo,
    },
    ToolDef {
        name: "ask",
        gate: ToolGate::Interactive,
        schema: super::schema::schema_ask,
        summary: preview::summary_ask,
        output: preview::preview_ask,
    },
    ToolDef {
        name: "webfetch",
        gate: ToolGate::Network,
        schema: super::schema::schema_webfetch,
        summary: preview::summary_webfetch,
        output: preview::preview_webfetch,
    },
    ToolDef {
        name: "replace",
        gate: ToolGate::FilesWrite,
        schema: super::schema::schema_replace,
        summary: preview::summary_replace,
        output: preview::preview_replace,
    },
    ToolDef {
        name: "bash",
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

fn spec(def: &ToolDef) -> Tool {
    Tool::new(def.name)
        .with_description(crate::config::tool_description(def.name))
        .with_schema((def.schema)())
}

pub(crate) fn tool_specs(ctx: &ToolContext) -> Vec<Tool> {
    TOOL_DEFS
        .iter()
        .filter(|def| tool_enabled(ctx, def))
        .map(spec)
        .collect()
}
