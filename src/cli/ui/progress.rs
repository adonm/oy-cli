use std::fmt::Display;
use std::time::Duration;

use super::{
    cyan, err_line, faint, green, is_quiet, line, red, sanitize_terminal,
    sanitize_terminal_for_display,
};

pub fn progress(
    label: &str,
    current: usize,
    total: usize,
    detail: impl Display,
    elapsed: Duration,
) {
    if is_quiet() {
        return;
    }
    let detail = sanitize_terminal(&detail.to_string());
    line(progress_line(label, current, total, &detail, elapsed));
}

fn progress_line(
    label: &str,
    current: usize,
    total: usize,
    detail: &str,
    elapsed: Duration,
) -> String {
    let total = total.max(1);
    let current = current.min(total);
    let head = format!(
        "  {} {current}/{total} {}",
        progress_bar(current, total, 18),
        cyan(label)
    );
    if detail.trim().is_empty() {
        format!("{head} · {}", faint(format_duration(elapsed)))
    } else {
        format!("{head} · {detail} · {}", faint(format_duration(elapsed)))
    }
}

fn progress_bar(current: usize, total: usize, width: u16) -> String {
    let total = total.max(1);
    let current = current.min(total);
    let width = width.max(1) as usize;
    let filled = (current as f64 / total as f64 * width as f64).round() as usize;
    let empty = width.saturating_sub(filled);
    format!("|{}{}|", "█".repeat(filled), " ".repeat(empty))
}

pub fn tool_start(name: &str, detail: &str) {
    if is_quiet() {
        return;
    }
    err_line(tool_start_line(
        &sanitize_terminal(name),
        &sanitize_terminal(detail),
    ));
}

pub fn tool_result(name: &str, elapsed: Duration, preview: &str) {
    if is_quiet() {
        return;
    }
    let preview = sanitize_terminal_for_display(preview);
    let preview = preview.trim_end();
    let head = tool_result_head(&sanitize_terminal(name), elapsed);
    let Some((first, rest)) = preview.split_once('\n') else {
        if preview.is_empty() {
            err_line(head);
        } else {
            err_line(format_args!("{head} · {first}", first = preview));
        }
        return;
    };
    err_line(format_args!("{head} · {first}"));
    for line in rest.lines() {
        err_line(format_args!("    {line}"));
    }
}

pub fn tool_error(name: &str, elapsed: Duration, err: impl Display) {
    if is_quiet() {
        return;
    }
    let name = sanitize_terminal(name);
    let err = sanitize_terminal(&err.to_string());
    err_line(format_args!(
        "  {} {name} {} · {err}",
        red("✗"),
        format_duration(elapsed)
    ));
}

pub fn format_duration(elapsed: Duration) -> String {
    if elapsed.as_millis() < 1000 {
        format!("{}ms", elapsed.as_millis())
    } else {
        format!("{:.1}s", elapsed.as_secs_f64())
    }
}

fn tool_start_line(name: &str, detail: &str) -> String {
    if detail.is_empty() {
        format!("  {} {name}", cyan("→"))
    } else {
        format!("  {} {name} · {detail}", cyan("→"))
    }
}

fn tool_result_head(name: &str, elapsed: Duration) -> String {
    format!("  {} {name} {}", green("✓"), format_duration(elapsed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{OutputMode, set_output_mode};

    #[test]
    fn elapsed_format_is_compact() {
        assert_eq!(format_duration(Duration::from_millis(42)), "42ms");
        assert_eq!(format_duration(Duration::from_millis(1250)), "1.2s");
    }

    #[test]
    fn progress_line_shows_bar_count_detail_and_elapsed() {
        set_output_mode(OutputMode::Normal);
        assert_eq!(progress_bar(2, 4, 8), "|████    |");
        assert_eq!(
            progress_line("review", 2, 4, "chunk 3", Duration::from_millis(1250)),
            "  |█████████         | 2/4 review · chunk 3 · 1.2s"
        );
    }

    #[test]
    fn tool_progress_lines_are_dense() {
        set_output_mode(OutputMode::Normal);
        assert_eq!(
            tool_start_line("read", "path=src/main.rs"),
            "  → read · path=src/main.rs"
        );
        assert_eq!(
            tool_result_head("read", Duration::from_millis(42)),
            "  ✓ read 42ms"
        );
    }

    #[test]
    fn tool_progress_sanitizes_untrusted_terminal_escapes() {
        set_output_mode(OutputMode::Normal);
        assert_eq!(
            tool_start_line("read", &sanitize_terminal("path=\x1b[2JREADME.md")),
            "  → read · path=␛[2JREADME.md"
        );
        assert_eq!(sanitize_terminal("ok\n\x1b]52;c;bad"), "ok\n␛]52;c;bad");
    }
}
