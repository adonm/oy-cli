//! Shared helpers for tool previews: value extraction, formatting,
//! search-hit rendering, and line clamping.

use serde_json::Value;
use std::fmt::Write as _;

use crate::tools::{
    DEFAULT_LIMIT, NORMAL_PREVIEW_LINES, PREVIEW_LINE_CHARS, VERBOSE_PREVIEW_LINES,
};

pub(crate) fn tool_call_summary(name: &str, args: &Value) -> String {
    crate::tools::registry::find_def(name)
        .map(|def| (def.summary)(args))
        .unwrap_or_else(|| preview_value(args, 120))
}

pub(crate) fn tool_output(name: &str, value: &Value) -> String {
    crate::tools::registry::find_def(name)
        .map(|def| (def.output)(value))
        .unwrap_or_else(|| preview_generic(value))
}

pub(crate) fn compact_kvs(args: &Value, keys: &[(&str, usize)]) -> String {
    keys.iter()
        .filter_map(|(key, max)| {
            let value = args.get(*key)?;
            if value.is_null() || value == false || value == "" {
                return None;
            }
            if *key == "limit" && value.as_u64() == Some(DEFAULT_LIMIT as u64) {
                return None;
            }
            Some(format!(
                "{}={}",
                key.replace('_', "-"),
                preview_value(value, *max)
            ))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn todo_call_summary(args: &Value) -> String {
    let items = args
        .get("todos")
        .or_else(|| args.get("items"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if items.is_empty() {
        return "0 items".to_string();
    }
    let first = items
        .first()
        .map(|item| preview_value(item.get("task").unwrap_or(item), 56))
        .unwrap_or_default();
    if items.len() == 1 {
        format!("1 item · {first}")
    } else {
        format!("{} items · {first}", items.len())
    }
}

pub(crate) fn preview_value(value: &Value, max: usize) -> String {
    let raw = value
        .as_str()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| value.to_string());
    crate::ui::compact_preview(&raw, max)
}

pub(crate) fn value_str<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

pub(crate) fn value_usize(value: &Value, key: &str) -> usize {
    value.get(key).and_then(Value::as_u64).unwrap_or(0) as usize
}

pub(crate) fn value_bool(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

pub(crate) fn bool_marker(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

pub(crate) fn truncation_flag(value: &Value) -> &'static str {
    bool_marker(value_bool(value, "truncated"))
}

pub(crate) fn verbose_preview(body: impl FnOnce() -> String) -> Option<String> {
    (!crate::ui::is_quiet()).then(body)
}

pub(crate) fn with_verbose(summary: String, body: impl FnOnce() -> String) -> String {
    let Some(body) = verbose_preview(body).filter(|body| !body.trim().is_empty()) else {
        return summary;
    };
    format!("{}\n{}", summary, limited_preview_body(&body))
}

pub(crate) fn limited_preview_body(body: &str) -> String {
    let max_lines = if crate::ui::is_verbose() {
        VERBOSE_PREVIEW_LINES
    } else {
        NORMAL_PREVIEW_LINES
    };
    crate::ui::clamp_lines(body, max_lines, PREVIEW_LINE_CHARS)
}

pub(crate) fn count_lines(text: &str) -> usize {
    text.lines().count()
}

pub(crate) fn count_files_in_matches(matches: &[Value]) -> usize {
    matches
        .iter()
        .filter_map(|item| item.get("path").and_then(Value::as_str))
        .collect::<std::collections::BTreeSet<_>>()
        .len()
}

pub(crate) fn append_preview_lines(out: &mut String, text: &str, title: &str) {
    let max_lines = if crate::ui::is_verbose() {
        VERBOSE_PREVIEW_LINES
    } else {
        NORMAL_PREVIEW_LINES
    };
    let line_count = text.lines().count();
    let preview = text.lines().take(max_lines).collect::<Vec<_>>().join("\n");
    if preview.is_empty() {
        return;
    }
    let block = crate::ui::text_block(title, &preview);
    for line in block.lines() {
        let _ = write!(out, "\n{line}");
    }
    if line_count > max_lines {
        let _ = write!(out, "\n  … {} more preview lines", line_count - max_lines);
    }
}

pub(crate) fn preview_generic(value: &Value) -> String {
    if crate::ui::is_verbose() {
        crate::ui::clamp_lines(
            &crate::tools::encode_tool_output(value),
            VERBOSE_PREVIEW_LINES,
            PREVIEW_LINE_CHARS,
        )
    } else if !value_bool(value, "ok") && value.get("ok").is_some() {
        format!("error: {}", value_str(value, "error"))
    } else {
        preview_value(value, crate::ui::terminal_width().saturating_sub(4).max(40))
    }
}

pub(crate) fn append_search_hits<'a>(out: &mut String, matches: impl Iterator<Item = &'a Value>) {
    let matches = matches.collect::<Vec<_>>();
    let file_count = matches
        .iter()
        .filter_map(|item| item.get("path").and_then(Value::as_str))
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    if file_count == 0 {
        return;
    }

    let grouped = file_count < matches.len();
    let mut current_path = "";
    let mut current_hits = Vec::new();
    for item in matches {
        let path = value_str(item, "path");
        if path != current_path && !current_hits.is_empty() {
            append_search_hit_block(out, current_path, &current_hits, grouped);
            current_hits.clear();
        }
        current_path = path;
        current_hits.push(item);
    }
    if !current_hits.is_empty() {
        append_search_hit_block(out, current_path, &current_hits, grouped);
    }
}

pub(crate) fn append_search_hit_block(
    out: &mut String,
    path: &str,
    hits: &[&Value],
    grouped: bool,
) {
    let rendered_lines = hits
        .iter()
        .map(|item| {
            (
                value_usize(item, "line_number").max(1),
                value_str(item, "text"),
            )
        })
        .collect::<Vec<_>>();
    if rendered_lines.iter().all(|(_, text)| text.is_empty()) {
        append_fallback_search_hits(out, path, hits, grouped);
        return;
    }

    let block = crate::ui::code_lines(path, &rendered_lines);
    for line in block.lines() {
        let _ = write!(out, "\n  {line}");
    }
}

pub(crate) fn append_fallback_search_hits(
    out: &mut String,
    path: &str,
    hits: &[&Value],
    grouped: bool,
) {
    if grouped {
        let _ = write!(out, "\n  {}", crate::ui::path(path));
        for item in hits {
            let _ = write!(out, "\n    {}", format_search_hit_line(item));
        }
    } else {
        for item in hits {
            let _ = write!(
                out,
                "\n  {}:{}",
                crate::ui::path(path),
                format_search_hit_line(item)
            );
        }
    }
}

pub(crate) fn format_search_hit_line(item: &Value) -> String {
    let line = value_usize(item, "line_number");
    let col = value_usize(item, "column");
    let text = crate::ui::truncate_chars(value_str(item, "text"), PREVIEW_LINE_CHARS);
    format!(
        "{}:{} {text}",
        crate::ui::faint(line),
        crate::ui::faint(col)
    )
}

pub(crate) fn output_preview<'a>(value: &'a Value, key: &str) -> &'a str {
    let preview_key = format!("{key}_preview");
    value
        .get(&preview_key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| value_str(value, key))
}

pub(crate) fn append_capped_flag(summary: &mut String, key: &str, capped: bool) {
    if capped {
        let _ = write!(summary, " · {key}-capped=yes");
    }
}

pub(crate) fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}
