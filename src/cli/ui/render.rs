//! Text renderers: markdown output, code/text previews, block titles,
//! and diff-coloured patch rendering.

use std::fmt::Write as _;

use bat::line_range::{LineRange, LineRanges};
use bat::{Input, PrettyPrinter};

use super::text::truncate_width;
use super::{path, terminal_width};

/// Preserve whether the original `text` ended with a newline:
/// if it did, return `out` as-is; otherwise strip `out`'s trailing newline.
fn preserve_trailing(text: &str, out: String) -> String {
    if text.ends_with('\n') {
        out
    } else {
        out.trim_end_matches('\n').to_string()
    }
}

pub fn markdown(text: &str) {
    super::out(&render_markdown(text));
}

fn render_markdown(text: &str) -> String {
    if let Some(rendered) = render_bat("markdown.md", text.as_bytes(), None) {
        return preserve_trailing(text, rendered);
    }
    text.to_string()
}

pub fn code(path: &str, text: &str, first_line: usize) -> String {
    preview_block(path, text, first_line)
}

pub fn code_lines(path: &str, lines: &[(usize, &str)]) -> String {
    if lines.is_empty() {
        return preview_block(path, "", 1);
    }

    let title = if path.is_empty() { "text" } else { path };
    let max_width = terminal_width().saturating_sub(4).max(40);
    let mut out = String::new();
    let _ = writeln!(out, "{}", truncate_width(&block_title(title), max_width));

    let first_line = lines
        .iter()
        .map(|(line, _)| *line)
        .min()
        .unwrap_or(1)
        .max(1);
    let last_line = lines
        .iter()
        .map(|(line, _)| *line)
        .max()
        .unwrap_or(first_line)
        .max(first_line);
    let mut content = String::new();
    for line_number in first_line..=last_line {
        if let Some((_, text)) = lines.iter().find(|(line, _)| *line == line_number) {
            content.push_str(text);
        }
        content.push('\n');
    }
    let content = offset_preview_content(&content, first_line);
    let ranges = LineRanges::from(
        lines
            .iter()
            .map(|(line, _)| LineRange::new((*line).max(1), (*line).max(1)))
            .collect::<Vec<_>>(),
    );
    let rendered = render_bat(title, content.as_bytes(), Some(ranges))
        .unwrap_or_else(|| fallback_preview_lines(lines));
    out.push_str(rendered.trim_end());
    out.trim_end().to_string()
}

pub fn text_block(title: &str, text: &str) -> String {
    preview_block(title, text, 1)
}

pub fn block_title(title: &str) -> String {
    path(format_args!("── {title}"))
}

fn preview_block(title: &str, text: &str, first_line: usize) -> String {
    let title = if title.is_empty() { "text" } else { title };
    let max_width = terminal_width().saturating_sub(4).max(40);
    let mut out = String::new();
    let _ = writeln!(out, "{}", truncate_width(&block_title(title), max_width));

    let content = offset_preview_content(text, first_line);
    let last_line = first_line.saturating_add(text.lines().count().max(1).saturating_sub(1));
    let ranges = LineRanges::from(vec![LineRange::new(first_line.max(1), last_line.max(1))]);
    let rendered = render_bat(title, content.as_bytes(), Some(ranges))
        .unwrap_or_else(|| fallback_preview_text(text));
    out.push_str(rendered.trim_end());
    out.trim_end().to_string()
}

fn render_bat(title: &str, bytes: &[u8], ranges: Option<LineRanges>) -> Option<String> {
    let mut out = String::new();
    let mut printer = PrettyPrinter::new();
    printer.input(Input::from_bytes(bytes).name(title));
    if let Some(ranges) = ranges {
        printer.line_ranges(ranges);
    }
    printer.print_with_writer(Some(&mut out)).ok()?;
    Some(out)
}

fn offset_preview_content(text: &str, first_line: usize) -> String {
    let mut content = "\n".repeat(first_line.saturating_sub(1));
    content.push_str(text);
    if text.is_empty() {
        content.push('\n');
    }
    content
}

fn fallback_preview_text(text: &str) -> String {
    text.to_string()
}

fn fallback_preview_lines(lines: &[(usize, &str)]) -> String {
    let mut out = String::new();
    for (line, text) in lines {
        let _ = writeln!(out, "{:>4} │ {}", line, text.trim_end_matches('\n'));
    }
    out
}

pub fn diff(text: &str) -> String {
    if let Some(rendered) = render_bat("changes.diff", text.as_bytes(), None) {
        return preserve_trailing(text, rendered);
    }
    text.to_string()
}
