use serde_json::Value;
use std::fmt::Write as _;

use super::{
    DEFAULT_LIMIT, NORMAL_PREVIEW_LINES, PREVIEW_ITEMS, PREVIEW_LINE_CHARS, VERBOSE_PREVIEW_LINES,
};

pub(super) fn tool_call_summary(name: &str, args: &Value) -> String {
    super::registry::find_def(name)
        .map(|def| (def.summary)(args))
        .unwrap_or_else(|| preview_value(args, 120))
}

pub(crate) fn tool_output(name: &str, value: &Value) -> String {
    super::registry::find_def(name)
        .map(|def| (def.output)(value))
        .unwrap_or_else(|| preview_generic(value))
}

pub(super) fn summary_list(args: &Value) -> String {
    compact_kvs(args, &[("path", 60), ("exclude", 40)])
}

pub(super) fn summary_read(args: &Value) -> String {
    compact_kvs(args, &[("path", 70), ("offset", 12), ("limit", 12)])
}

pub(super) fn summary_search(args: &Value) -> String {
    compact_kvs(
        args,
        &[("pattern", 70), ("path", 50), ("mode", 12), ("exclude", 35)],
    )
}

pub(super) fn summary_replace(args: &Value) -> String {
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

pub(super) fn summary_sloc(args: &Value) -> String {
    compact_kvs(args, &[("path", 70), ("exclude", 40)])
}

pub(super) fn summary_bash(args: &Value) -> String {
    preview_value(args.get("command").unwrap_or(&Value::Null), 100)
}

pub(super) fn summary_webfetch(args: &Value) -> String {
    compact_kvs(args, &[("method", 8), ("url", 100)])
}

pub(super) fn summary_ask(args: &Value) -> String {
    preview_value(args.get("question").unwrap_or(&Value::Null), 100)
}

pub(super) fn summary_todo(args: &Value) -> String {
    todo_call_summary(args)
}

fn compact_kvs(args: &Value, keys: &[(&str, usize)]) -> String {
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

fn todo_call_summary(args: &Value) -> String {
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

pub(super) fn preview_value(value: &Value, max: usize) -> String {
    let raw = value
        .as_str()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| value.to_string());
    crate::ui::compact_preview(&raw, max)
}

fn value_str<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

fn value_usize(value: &Value, key: &str) -> usize {
    value.get(key).and_then(Value::as_u64).unwrap_or(0) as usize
}

fn value_bool(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn bool_marker(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn truncation_flag(value: &Value) -> &'static str {
    bool_marker(value_bool(value, "truncated"))
}

fn verbose_preview(body: impl FnOnce() -> String) -> Option<String> {
    (!crate::ui::is_quiet()).then(body)
}

fn with_verbose(summary: String, body: impl FnOnce() -> String) -> String {
    let Some(body) = verbose_preview(body).filter(|body| !body.trim().is_empty()) else {
        return summary;
    };
    format!("{}\n{}", summary, limited_preview_body(&body))
}

fn limited_preview_body(body: &str) -> String {
    let max_lines = if crate::ui::is_verbose() {
        VERBOSE_PREVIEW_LINES
    } else {
        NORMAL_PREVIEW_LINES
    };
    crate::ui::clamp_lines(body, max_lines, PREVIEW_LINE_CHARS)
}

fn count_lines(text: &str) -> usize {
    text.lines().count()
}

fn count_files_in_matches(matches: &[Value]) -> usize {
    matches
        .iter()
        .filter_map(|item| item.get("path").and_then(Value::as_str))
        .collect::<std::collections::BTreeSet<_>>()
        .len()
}

fn append_preview_lines(out: &mut String, text: &str, title: &str) {
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

pub(super) fn preview_generic(value: &Value) -> String {
    if crate::ui::is_verbose() {
        crate::ui::clamp_lines(
            &super::encode_tool_output(value),
            VERBOSE_PREVIEW_LINES,
            PREVIEW_LINE_CHARS,
        )
    } else if !value_bool(value, "ok") && value.get("ok").is_some() {
        format!("error: {}", value_str(value, "error"))
    } else {
        preview_value(value, crate::ui::terminal_width().saturating_sub(4).max(40))
    }
}

pub(super) fn preview_list(value: &Value) -> String {
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
pub(super) fn preview_read(value: &Value) -> String {
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
pub(super) fn preview_search(value: &Value) -> String {
    let matches = value
        .get("matches")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let total = value_usize(value, "match_count");
    let files = count_files_in_matches(matches);
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
        for item in matches.iter().take(PREVIEW_ITEMS) {
            let _ = write!(out, "\n  {}", format_search_hit(item));
        }
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

fn format_search_hit(item: &Value) -> String {
    let path = value_str(item, "path");
    let line = value_usize(item, "line_number");
    let col = value_usize(item, "column");
    let text = crate::ui::truncate_chars(value_str(item, "text"), PREVIEW_LINE_CHARS);
    format!(
        "{}:{}:{} {}",
        crate::ui::path(path),
        crate::ui::faint(line),
        crate::ui::faint(col),
        text
    )
}
pub(super) fn preview_replace(value: &Value) -> String {
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
                    "\n  {} · {} repl",
                    value_str(item, "path"),
                    value_usize(item, "replacements")
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
pub(super) fn preview_bash(value: &Value) -> String {
    let code = value
        .get("returncode")
        .and_then(Value::as_i64)
        .unwrap_or(-1);
    let stdout = value
        .get("stdout_preview")
        .and_then(Value::as_str)
        .unwrap_or_else(|| value_str(value, "stdout"));
    let stderr = value
        .get("stderr_preview")
        .and_then(Value::as_str)
        .unwrap_or_else(|| value_str(value, "stderr"));
    let icon = if code == 0 {
        crate::ui::green("✓")
    } else {
        crate::ui::red("✗")
    };
    let mut summary = format!(
        "{icon} exit {code} · stdout {} line{} · stderr {} line{} · stdout-truncated={} · stderr-truncated={}",
        count_lines(stdout),
        plural(count_lines(stdout)),
        count_lines(stderr),
        plural(count_lines(stderr)),
        bool_marker(value_bool(value, "stdout_truncated")),
        bool_marker(value_bool(value, "stderr_truncated"))
    );
    if code != 0
        && let Some(first_stderr) = stderr.lines().find(|line| !line.trim().is_empty())
    {
        summary.push_str(&format!(
            " · {}",
            crate::ui::truncate_chars(first_stderr.trim(), 80)
        ));
    }
    with_verbose(summary, || {
        let mut out = String::new();
        for key in ["stdout", "stderr"] {
            let text = value_str(value, key);
            let truncated_key = format!("{key}_truncated");
            let truncated = value_bool(value, &truncated_key);
            if text.is_empty() {
                if truncated {
                    let _ = write!(
                        out,
                        "\n{}\n  … {key} truncated",
                        crate::ui::block_title(key)
                    );
                }
                continue;
            }
            append_preview_lines(&mut out, text, key);
            if truncated {
                let _ = write!(out, "\n  … {key} truncated for model context");
            }
        }
        out.trim_start().to_string()
    })
}
pub(super) fn preview_webfetch(value: &Value) -> String {
    let status = value
        .get("status_code")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let url = value_str(value, "url");
    if value
        .get("binary")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return format!(
            "HTTP {status} · binary · {} bytes · {url}",
            value_usize(value, "content_bytes")
        );
    }
    let text = value
        .get("text_preview")
        .and_then(Value::as_str)
        .unwrap_or_else(|| value_str(value, "text"));
    let format = value_str(value, "format");
    let kind = if format.is_empty() { "text" } else { format };
    let summary = format!(
        "HTTP {status} · {kind} · {} line{} · truncated={} · {url}",
        count_lines(text),
        plural(count_lines(text)),
        truncation_flag(value)
    );
    with_verbose(summary, || {
        let mut out = String::new();
        if !text.is_empty() {
            append_preview_lines(&mut out, text, kind);
        }
        if value_bool(value, "truncated") {
            out.push_str("\n  … response body truncated for model context");
        }
        out.trim_start().to_string()
    })
}
pub(super) fn preview_sloc(value: &Value) -> String {
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
pub(super) fn preview_ask(value: &Value) -> String {
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

pub(super) fn preview_todo(value: &Value) -> String {
    let preview = value_str(value, "preview");
    if !preview.is_empty() {
        return limited_preview_body(preview);
    }

    let items = value
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    limited_preview_body(&super::todo::format_todo_preview_from_values(items))
}
pub(super) fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    static OUTPUT_MODE_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn tool_preview_normal_snapshot() {
        let _guard = OUTPUT_MODE_TEST_LOCK
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        crate::ui::set_output_mode(crate::ui::OutputMode::Normal);
        let value = json!({
            "pattern": "run_prompt",
            "path": "src",
            "match_count": 6,
            "matches": [
                {"path": "src/session.rs", "line_number": 283, "column": 1, "text": "pub async fn run_prompt(...)"},
                {"path": "src/app.rs", "line_number": 40, "column": 9, "text": "Run(RunArgs),"},
                {"path": "src/chat.rs", "line_number": 110, "column": 18, "text": "run_prompt from chat"},
                {"path": "src/model.rs", "line_number": 88, "column": 5, "text": "resolve model before run_prompt"},
                {"path": "src/tools.rs", "line_number": 500, "column": 13, "text": "tool output for run_prompt"},
                {"path": "src/ui.rs", "line_number": 410, "column": 22, "text": "session::run_prompt(...)"}
            ],
            "truncated": false
        });
        insta::assert_snapshot!(tool_output("search", &value));
        crate::ui::set_output_mode(crate::ui::OutputMode::Normal);
    }

    #[test]
    fn tool_preview_verbose_snapshot() {
        let _guard = OUTPUT_MODE_TEST_LOCK
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        crate::ui::set_output_mode(crate::ui::OutputMode::Verbose);
        let value = json!({
            "path": "src/main.rs",
            "offset": 1,
            "limit": 3,
            "text": "fn main() {\n    println!(\"hi\");\n}",
            "line_count": 10,
            "truncated": true
        });
        insta::assert_snapshot!(tool_output("read", &value));
        crate::ui::set_output_mode(crate::ui::OutputMode::Normal);
    }

    #[test]
    fn tool_preview_bash_failure_snapshot() {
        let _guard = OUTPUT_MODE_TEST_LOCK
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        crate::ui::set_output_mode(crate::ui::OutputMode::Normal);
        let value = json!({
            "returncode": 2,
            "stdout": "",
            "stderr": "missing file\nusage: demo <path>\ntry --help\nexample: demo Cargo.toml\nerror code E2\nignored tail\n",
            "stdout_truncated": false,
            "stderr_truncated": false
        });
        insta::assert_snapshot!(tool_output("bash", &value));
        crate::ui::set_output_mode(crate::ui::OutputMode::Normal);
    }

    #[test]
    fn tool_preview_replace_normal_snapshot() {
        let _guard = OUTPUT_MODE_TEST_LOCK
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        crate::ui::set_output_mode(crate::ui::OutputMode::Normal);
        let value = json!({
            "changed_file_count": 6,
            "replacement_count": 9,
            "changed_files": [
                {"path": "src/lib.rs", "replacements": 1},
                {"path": "src/main.rs", "replacements": 2},
                {"path": "src/app.rs", "replacements": 1},
                {"path": "src/config.rs", "replacements": 2},
                {"path": "src/tools.rs", "replacements": 1},
                {"path": "README.md", "replacements": 2}
            ],
            "truncated": false
        });
        insta::assert_snapshot!(tool_output("replace", &value));
        crate::ui::set_output_mode(crate::ui::OutputMode::Normal);
    }

    #[test]
    fn tool_preview_webfetch_snapshot() {
        let _guard = OUTPUT_MODE_TEST_LOCK
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        crate::ui::set_output_mode(crate::ui::OutputMode::Normal);
        let value = json!({
            "status_code": 200,
            "url": "https://example.com/docs",
            "text": "# docs\nhello\ninstall\nconfigure\nrun\nextra\n",
            "format": "markdown",
            "truncated": false
        });
        insta::assert_snapshot!(tool_output("webfetch", &value));
        crate::ui::set_output_mode(crate::ui::OutputMode::Normal);
    }
}
