//! Preview functions for workspace tools: list, read, search, replace,
//! patch, sloc, and outline.

use serde_json::Value;
use std::fmt::Write as _;

use super::common::*;
use crate::tools::{PREVIEW_ITEMS, PREVIEW_LINE_CHARS};

pub(crate) fn summary_list(args: &Value) -> String {
    compact_kvs(args, &[("path", 60), ("exclude", 40)])
}

pub(crate) fn preview_list(value: &Value) -> String {
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let total = value_usize(value, "count");
    let summary = format!(
        "path={} · {} item{} · shown={} · truncated={}",
        value_str(value, "path"),
        total,
        plural(total),
        items.len().min(PREVIEW_ITEMS),
        truncation_flag(value)
    );
    with_verbose(summary, || {
        let mut out = String::new();
        for item in items.iter().take(PREVIEW_ITEMS) {
            let _ = write!(
                out,
                "\n  {}",
                crate::ui::truncate_chars(item.as_str().unwrap_or(""), PREVIEW_LINE_CHARS)
            );
        }
        let shown = items.len().min(PREVIEW_ITEMS);
        if total > shown || value_bool(value, "truncated") {
            let remaining = total.saturating_sub(shown);
            let _ = write!(out, "\n  … {remaining} more item{}", plural(remaining));
        }
        out.trim_start().to_string()
    })
}

pub(crate) fn summary_read(args: &Value) -> String {
    compact_kvs(args, &[("path", 70), ("offset", 12), ("limit", 12)])
}

pub(crate) fn preview_read(value: &Value) -> String {
    let path = value_str(value, "path");
    let offset = value_usize(value, "offset");
    let line_count = value_usize(value, "line_count");
    let text = value_str(value, "text");
    let shown = text.lines().count();
    let end = offset.saturating_add(shown).saturating_sub(1);
    let more = if value_bool(value, "truncated") {
        format!(" · {} more", line_count.saturating_sub(end))
    } else {
        String::new()
    };
    let summary = format!(
        "path={path} · lines {offset}-{end}/{line_count} · returned={shown}{more} · truncated={}",
        truncation_flag(value)
    );
    with_verbose(summary, || {
        let mut out = String::new();
        if text.is_empty() {
            out.push_str("  <empty>");
        } else {
            out.push_str(&crate::ui::code(path, text, offset));
        }
        if value_bool(value, "truncated") {
            let hidden = line_count.saturating_sub(end);
            let _ = write!(
                out,
                "\n  … read truncated: {hidden} more line{} available",
                plural(hidden)
            );
        }
        out
    })
}

pub(crate) fn summary_read_multiple_files(args: &Value) -> String {
    if let Some(files) = args.get("files").and_then(|v| v.as_array()) {
        format!("{} files", files.len())
    } else {
        "0 files".to_string()
    }
}

pub(crate) fn preview_read_multiple_files(output: &Value) -> String {
    if let Some(files) = output.get("files").and_then(|v| v.as_array()) {
        let total_lines: usize = files
            .iter()
            .filter_map(|f| f.get("line_count").and_then(|v| v.as_u64()))
            .map(|v| v as usize)
            .sum();
        format!("{} files · {} total lines", files.len(), total_lines)
    } else {
        "0 files".to_string()
    }
}

pub(crate) fn summary_search(args: &Value) -> String {
    compact_kvs(
        args,
        &[("pattern", 70), ("path", 50), ("mode", 12), ("exclude", 35)],
    )
}

pub(crate) fn preview_search(value: &Value) -> String {
    let matches = value
        .get("matches")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let total = value_usize(value, "match_count");
    let files = value
        .get("file_count")
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .unwrap_or_else(|| count_files_in_matches(matches));
    let read_path = value_str(value, "read_path");
    let summary = if total == 0 {
        format!(
            "pattern=/{}/ · path={} · 0 matches · truncated={}",
            value_str(value, "pattern"),
            value_str(value, "path"),
            truncation_flag(value)
        )
    } else {
        format!(
            "pattern=/{}/ · path={} · {} {} · {} file{} · returned={} · truncated={}",
            value_str(value, "pattern"),
            value_str(value, "path"),
            total,
            if total == 1 { "match" } else { "matches" },
            files,
            plural(files),
            matches.len(),
            truncation_flag(value)
        )
    };
    with_verbose(summary, || {
        let mut out = String::new();
        if !read_path.is_empty() {
            let _ = write!(out, "\n  → Read {read_path}");
        }
        append_search_hits(&mut out, matches.iter().take(PREVIEW_ITEMS));
        if value_bool(value, "truncated") {
            let _ = write!(
                out,
                "\n  … {} more matches",
                total.saturating_sub(matches.len().min(PREVIEW_ITEMS))
            );
        }
        out.trim_start().to_string()
    })
}

pub(crate) fn summary_replace(args: &Value) -> String {
    compact_kvs(
        args,
        &[
            ("path", 45),
            ("mode", 12),
            ("pattern", 45),
            ("replacement", 45),
        ],
    )
}

pub(crate) fn preview_replace(value: &Value) -> String {
    let changed = value
        .get("changed_files")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let total_files = value_usize(value, "changed_file_count");
    let files = total_files.max(changed.len());
    let replacements = value_usize(value, "replacement_count");
    let summary = format!(
        "{} file{} changed · {} replacement{} · returned={} · truncated={}",
        files,
        plural(files),
        replacements,
        plural(replacements),
        changed.len(),
        truncation_flag(value)
    );
    with_verbose(summary, || {
        let mut out = String::new();
        if changed.is_empty() {
            out.push_str("  <no changes>");
        } else {
            for item in changed.iter().take(PREVIEW_ITEMS) {
                let _ = write!(
                    out,
                    "\n  {} · {} replacement{}",
                    crate::ui::path(value_str(item, "path")),
                    value_usize(item, "replacements"),
                    plural(value_usize(item, "replacements"))
                );
            }
            if value_bool(value, "truncated") || files > changed.len() {
                let _ = write!(
                    out,
                    "\n  … {} more files",
                    files.saturating_sub(changed.len())
                );
            }
        }
        if let Some(diff) = value
            .get("diff")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&crate::ui::diff(diff));
        }
        out.trim_start().to_string()
    })
}

pub(crate) fn summary_patch(args: &Value) -> String {
    compact_kvs(args, &[("strip", 8), ("limit", 12), ("patch", 100)])
}

pub(crate) fn preview_patch(value: &Value) -> String {
    let changed = value
        .get("changed_files")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let total_files = value_usize(value, "changed_file_count");
    let files = total_files.max(changed.len());
    let patches = value_usize(value, "patch_count");
    let summary = format!(
        "{} patch{} applied · {} file{} changed · returned={} · truncated={}",
        patches,
        plural(patches),
        files,
        plural(files),
        changed.len(),
        truncation_flag(value)
    );
    with_verbose(summary, || {
        let mut out = String::new();
        if changed.is_empty() {
            out.push_str("  <no changes>");
        } else {
            for item in changed.iter().take(PREVIEW_ITEMS) {
                let _ = write!(out, "\n  {}", value_str(item, "path"));
            }
            if value_bool(value, "truncated") || files > changed.len() {
                let _ = write!(
                    out,
                    "\n  … {} more files",
                    files.saturating_sub(changed.len())
                );
            }
        }
        if let Some(diff) = value
            .get("diff")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&crate::ui::diff(diff));
        }
        out.trim_start().to_string()
    })
}

pub(crate) fn summary_sloc(args: &Value) -> String {
    compact_kvs(args, &[("path", 70), ("exclude", 40)])
}

pub(crate) fn preview_sloc(value: &Value) -> String {
    let total = value
        .pointer("/output/Total/code")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let comments = value
        .pointer("/output/Total/comments")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let blanks = value
        .pointer("/output/Total/blanks")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mut langs = value
        .get("output")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter(|(name, _)| name.as_str() != "Total")
                .filter_map(|(name, stats)| {
                    stats
                        .get("code")
                        .and_then(Value::as_u64)
                        .map(|code| (name, code))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    langs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    let summary = format!(
        "{}: {total} code · {comments} comments · {blanks} blank",
        value_str(value, "path")
    );
    with_verbose(summary, || {
        let mut out = String::new();
        for (name, code) in langs.into_iter().take(PREVIEW_ITEMS) {
            let _ = write!(out, "\n  {name}: {code}");
        }
        out.trim_start().to_string()
    })
}

#[cfg(feature = "outline")]
pub(crate) fn summary_outline(args: &Value) -> String {
    let path = args.get("path").and_then(Value::as_str).unwrap_or("");
    let depth = args.get("depth").and_then(Value::as_u64).unwrap_or(2);
    format!("path={} depth={}", path, depth)
}

#[cfg(feature = "outline")]
pub(crate) fn preview_outline(value: &Value) -> String {
    let path = value_str(value, "path");
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let summary = format!("path={} · {} items", path, items.len());
    with_verbose(summary, || {
        let mut out = String::new();
        for item in items.iter().take(PREVIEW_ITEMS) {
            let kind = item.get("kind").and_then(Value::as_str).unwrap_or("");
            let name = item.get("name").and_then(Value::as_str).unwrap_or("");
            let line = item.get("line").and_then(Value::as_u64).unwrap_or(0);
            let depth = item.get("depth").and_then(Value::as_u64).unwrap_or(0);
            let indent = "  ".repeat(depth as usize);
            let _ = write!(out, "\n  {}{}{} ({})", indent, kind, name, line);
        }
        if items.len() > PREVIEW_ITEMS {
            let _ = write!(out, "\n  … {} more items", items.len() - PREVIEW_ITEMS);
        }
        out.trim_start().to_string()
    })
}
