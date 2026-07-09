//! Deterministic tool helpers exposed through the oy MCP server.

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;

mod args;
mod external;
pub(crate) mod policy;
mod workspace;

pub(crate) use policy::{Approval, ToolPolicy};

pub(crate) fn has_external_sloc_counter() -> bool {
    workspace::has_tokei()
}

pub(crate) fn has_external_outline_tool() -> bool {
    workspace::has_universal_ctags()
}

pub(crate) fn has_external_security_scanner() -> bool {
    workspace::has_sighthound()
}

#[derive(Debug, Clone)]
pub struct ToolContext {
    root: PathBuf,
}

impl ToolContext {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }
}

fn parse_tool_args<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T> {
    serde_json::from_value(args).context("invalid tool arguments")
}

pub(crate) async fn invoke_read_only_deterministic(
    root: PathBuf,
    name: &str,
    args: Value,
) -> Result<Value> {
    let ctx = ToolContext::new(root);
    match name {
        "outline" => parse_tool_args(args).and_then(|args| workspace::tool_outline(&ctx, args)),
        "sloc" => parse_tool_args(args).and_then(|args| workspace::tool_sloc(&ctx, args)),
        "sighthound" => {
            parse_tool_args(args).and_then(|args| workspace::tool_sighthound(&ctx, args))
        }
        other => anyhow::bail!("unknown deterministic tool: {other}"),
    }
}
