//! Preview functions for process tools: bash.

use serde_json::Value;
use std::fmt::Write as _;

use super::common::*;

pub(crate) fn summary_bash(args: &Value) -> String {
    preview_value(args.get("command").unwrap_or(&Value::Null), 100)
}

pub(crate) fn preview_bash(value: &Value) -> String {
    let code = value
        .get("returncode")
        .and_then(Value::as_i64)
        .unwrap_or(-1);
    let stdout = output_preview(value, "stdout");
    let stderr = output_preview(value, "stderr");
    let stdout_truncated = value_bool(value, "stdout_truncated");
    let stderr_truncated = value_bool(value, "stderr_truncated");
    let stdout_capped = value_bool(value, "stdout_capped");
    let stderr_capped = value_bool(value, "stderr_capped");
    let icon = if code == 0 {
        crate::ui::green("\u{2713}")
    } else {
        crate::ui::red("\u{2717}")
    };
    let mut summary = format!(
        "{icon} exit {code} · stdout {} line{} · stderr {} line{} · stdout-truncated={} · stderr-truncated={}",
        count_lines(stdout),
        plural(count_lines(stdout)),
        count_lines(stderr),
        plural(count_lines(stderr)),
        bool_marker(stdout_truncated),
        bool_marker(stderr_truncated)
    );
    append_capped_flag(&mut summary, "stdout", stdout_capped);
    append_capped_flag(&mut summary, "stderr", stderr_capped);
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
        for (key, text, truncated) in [
            ("stdout", stdout, stdout_truncated),
            ("stderr", stderr, stderr_truncated),
        ] {
            if text.is_empty() {
                if truncated {
                    let _ = write!(
                        out,
                        "\n{}\n  … {key} truncated with no preview",
                        crate::ui::block_title(key)
                    );
                }
                continue;
            }
            append_preview_lines(&mut out, text, key);
            if truncated {
                let _ = write!(out, "\n  … {key} truncated; showing bounded preview");
            }
        }
        out.trim_start().to_string()
    })
}
