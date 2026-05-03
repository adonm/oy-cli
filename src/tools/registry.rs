use genai::chat::Tool;
use serde_json::Value;

use super::policy::{Approval, NetworkAccess};
use super::{ToolContext, schema};

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
}

const TOOL_DEFS: &[ToolDef] = &[
    ToolDef {
        name: "list",
        gate: ToolGate::Always,
        schema: schema::schema_list,
    },
    ToolDef {
        name: "read",
        gate: ToolGate::Always,
        schema: schema::schema_read,
    },
    ToolDef {
        name: "search",
        gate: ToolGate::Always,
        schema: schema::schema_search,
    },
    ToolDef {
        name: "sloc",
        gate: ToolGate::Always,
        schema: schema::schema_sloc,
    },
    ToolDef {
        name: "todo",
        gate: ToolGate::Always,
        schema: schema::schema_todo,
    },
    ToolDef {
        name: "ask",
        gate: ToolGate::Interactive,
        schema: schema::schema_ask,
    },
    ToolDef {
        name: "webfetch",
        gate: ToolGate::Network,
        schema: schema::schema_webfetch,
    },
    ToolDef {
        name: "replace",
        gate: ToolGate::FilesWrite,
        schema: schema::schema_replace,
    },
    ToolDef {
        name: "bash",
        gate: ToolGate::Shell,
        schema: schema::schema_bash,
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
