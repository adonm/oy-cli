//! Structured reasoning tool for step-by-step problem solving.
//!
//! Allows the model to record explicit reasoning steps with numbered thoughts,
//! revisions, and branching comparisons.

use anyhow::Result;
use serde::Serialize;
use serde_json::Value;

use super::ToolContext;
use super::args::ThinkArgs;

#[derive(Debug, Clone, Serialize)]
pub(super) struct Thought {
    pub number: usize,
    pub content: String,
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revises_thought: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch_from_thought: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct ThinkOutput {
    pub thought: Thought,
    pub total_thoughts: usize,
    pub next_thought_needed: bool,
}

pub(super) fn tool_think(_ctx: &mut ToolContext, args: ThinkArgs) -> Result<Value> {
    let thought = Thought {
        number: args.thought_number,
        content: args.thought,
        mode: args.mode,
        revises_thought: args.revises_thought,
        branch_from_thought: args.branch_from_thought,
        branch_id: args.branch_id,
    };

    let output = ThinkOutput {
        thought,
        total_thoughts: args.total_thoughts,
        next_thought_needed: args.next_thought_needed,
    };

    Ok(serde_json::to_value(output)?)
}
