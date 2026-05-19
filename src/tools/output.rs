//! Tool output encoding and progress hooks.
//!
//! This module is the narrow handoff from tool invocation into human previews
//! and transcript-safe encoded values.

use serde_json::Value;
use toon_format::encode_default;

use super::preview;

const MODEL_VISIBLE_TOOL_OUTPUT_MAX_BYTES: usize = 64 * 1024;

pub fn encode_tool_output(value: &Value) -> String {
    encode_default(value).unwrap_or_else(|_| {
        serde_json::to_string_pretty(value).unwrap_or_else(|_| String::from("{}"))
    })
}

pub(crate) fn cap_model_visible_tool_output(output: &str) -> String {
    cap_model_visible_tool_output_at(output, MODEL_VISIBLE_TOOL_OUTPUT_MAX_BYTES)
}

fn cap_model_visible_tool_output_at(output: &str, max_bytes: usize) -> String {
    if output.len() <= max_bytes {
        return output.to_string();
    }
    if max_bytes == 0 {
        return String::new();
    }

    let header = format!(
        "[tool output truncated — original {} bytes; showing head/tail; re-run tool for full output]\n",
        output.len()
    );
    let separator = "\n\n[... middle omitted ...]\n\n";
    let overhead = header.len() + separator.len();
    if overhead >= max_bytes {
        return prefix_at_char_boundary(&header, max_bytes).to_string();
    }

    let body_budget = max_bytes - overhead;
    let head_budget = body_budget / 2;
    let tail_budget = body_budget - head_budget;
    format!(
        "{}{}{}{}",
        header,
        prefix_at_char_boundary(output, head_budget),
        separator,
        suffix_at_char_boundary(output, tail_budget)
    )
}

fn prefix_at_char_boundary(text: &str, max_bytes: usize) -> &str {
    if max_bytes >= text.len() {
        return text;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn suffix_at_char_boundary(text: &str, max_bytes: usize) -> &str {
    if max_bytes >= text.len() {
        return text;
    }
    let mut start = text.len() - max_bytes;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_visible_tool_output_preserves_head_and_tail_within_cap() {
        let output = format!(
            "{}{}{}",
            "head-".repeat(20),
            "middle-".repeat(40),
            "tail-".repeat(20)
        );

        let capped = cap_model_visible_tool_output_at(&output, 240);

        assert!(capped.len() <= 240);
        assert!(capped.contains("tool output truncated"));
        assert!(capped.contains("head-head"));
        assert!(capped.contains("tail-tail"));
        assert!(capped.contains("middle omitted"));
    }

    #[test]
    fn model_visible_tool_output_does_not_split_utf8() {
        let output = format!("{}{}", "🙂".repeat(80), "done");

        let capped = cap_model_visible_tool_output_at(&output, 160);

        assert!(capped.len() <= 160);
        assert!(capped.is_char_boundary(capped.len()));
        assert!(capped.ends_with("done"));
    }
}
