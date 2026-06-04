//! Preview functions for planning tools: ask, todo, and think.

use serde_json::Value;

use super::common::*;
use crate::tools::PREVIEW_LINE_CHARS;

pub(crate) fn summary_ask(args: &Value) -> String {
    preview_value(args.get("question").unwrap_or(&Value::Null), 100)
}

pub(crate) fn preview_ask(value: &Value) -> String {
    let answer = value.as_str().unwrap_or_default();
    if answer.is_empty() {
        "<no selection>".to_string()
    } else {
        format!(
            "selected: {}",
            crate::ui::truncate_chars(answer, PREVIEW_LINE_CHARS)
        )
    }
}

pub(crate) fn summary_todo(args: &Value) -> String {
    todo_call_summary(args)
}

pub(crate) fn preview_todo(value: &Value) -> String {
    let preview = value_str(value, "preview");
    if !preview.is_empty() {
        return limited_preview_body(preview);
    }

    let items = value
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    limited_preview_body(&crate::tools::todo::format_todo_preview_from_values(items))
}

pub(crate) fn summary_think(args: &Value) -> String {
    let thought_num = args
        .get("thought_number")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total = args
        .get("total_thoughts")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    format!("thought {thought_num}/{total}")
}

pub(crate) fn preview_think(value: &Value) -> String {
    let number = value_usize(value, "thought_number");
    let total = value_usize(value, "total_thoughts");
    let next = value_bool(value, "next_thought_needed");
    let thought = value_str(value, "thought");
    let summary = format!(
        "thought {number}/{total}{}",
        if next { " · more to come" } else { " · done" }
    );
    with_verbose(summary, || {
        let mut out = String::new();
        append_preview_lines(&mut out, thought, "reasoning");
        out.trim_start().to_string()
    })
}
