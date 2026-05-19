//! Tool output encoding and progress hooks.
//!
//! This module is the narrow handoff from tool invocation into human previews
//! and transcript-safe encoded values.

use serde_json::Value;
use toon_format::encode_default;

use super::preview;

pub fn encode_tool_output(value: &Value) -> String {
    encode_default(value).unwrap_or_else(|_| {
        serde_json::to_string_pretty(value).unwrap_or_else(|_| String::from("{}"))
    })
}

pub(crate) fn note_tool(name: &str, args: &Value) {
    let detail = tool_call_summary(name, args);
    crate::ui::tool_start(name, &detail);
}

fn tool_call_summary(name: &str, args: &Value) -> String {
    preview::tool_call_summary(name, args)
}

pub fn preview_tool_output(name: &str, value: &Value) -> String {
    preview::tool_output(name, value)
}
