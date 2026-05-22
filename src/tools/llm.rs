//! Adapter from `oy`'s tool registry to the native LLM tool trait.
//!
//! The LLM backend sees only names and JSON strings; this module holds the
//! shared tool context, invokes registered tools, and encodes their output.

use crate::llm::{LlmTool, LlmToolFuture, LlmTools};
use anyhow::{Context, Result};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::ToolContext;
use super::registry::{self, ToolDef};

pub(crate) async fn llm_tools(ctx: Arc<Mutex<ToolContext>>) -> LlmTools {
    let defs = {
        let ctx = ctx.lock().await;
        registry::enabled_tool_defs(&ctx)
    };
    defs.into_iter()
        .map(|def| {
            Box::new(OyTool {
                def,
                ctx: ctx.clone(),
            }) as Box<dyn LlmTool>
        })
        .collect()
}

#[derive(Clone)]
struct OyTool {
    def: &'static ToolDef,
    ctx: Arc<Mutex<ToolContext>>,
}

impl LlmTool for OyTool {
    fn name(&self) -> &str {
        self.def.name()
    }

    fn call<'a>(&'a self, args: String) -> LlmToolFuture<'a> {
        Box::pin(async move { call_tool(self.ctx.clone(), self.def.name(), args).await })
    }
}

async fn call_tool(ctx: Arc<Mutex<ToolContext>>, name: &str, args: String) -> Result<String> {
    let args = serde_json::from_str::<Value>(&args)
        .with_context(|| format!("tool `{name}` supplied invalid JSON arguments"))?;
    let value = super::invoke_shared(ctx, name, args).await?;
    Ok(super::output::cap_model_visible_tool_output(
        &super::encode_tool_output(&value),
    ))
}
