//! Text renderers: markdown output, code/text previews, block titles,
//! and diff-coloured patch rendering.

use std::fmt::Write as _;

use bat::line_range::{LineRange, LineRanges};
use bat::{Input, PrettyPrinter, StripAnsiMode, WrappingMode};

use super::text::truncate_width;
use super::{
    bold, color_enabled, cyan, faint, green, path, red, sanitize_terminal, terminal_width,
};

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
    let sanitized = sanitize_terminal(text);
    if let Some(rendered) =
        render_plain_with_bat("markdown.md", sanitized.as_bytes(), terminal_width())
    {
        return preserve_trailing(text, rendered);
    }
    if !color_enabled() {
        return sanitized;
    }
    let mut in_fence = false;
    let mut out = String::new();
    for line in sanitized.lines() {
        let trimmed = line.trim_start();
        let rendered = if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            faint(line)
        } else if in_fence {
            cyan(line)
        } else if trimmed.starts_with('#') {
            super::paint("1;35", line)
        } else if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            cyan(line)
        } else {
            line.to_string()
        };
        let _ = writeln!(out, "{rendered}");
    }
    preserve_trailing(text, out)
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
    let rendered = render_with_bat_grid(title, content.as_bytes(), ranges, max_width, false)
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
    let rendered = render_with_bat(title, content.as_bytes(), ranges, max_width)
        .unwrap_or_else(|| fallback_preview_text(text));
    out.push_str(rendered.trim_end());
    out.trim_end().to_string()
}

fn render_with_bat(title: &str, bytes: &[u8], ranges: LineRanges, width: usize) -> Option<String> {
    render_with_bat_colored(title, bytes, ranges, width, color_enabled())
}

fn render_with_bat_colored(
    title: &str,
    bytes: &[u8],
    ranges: LineRanges,
    width: usize,
    colored: bool,
) -> Option<String> {
    render_with_bat_grid_colored(title, bytes, ranges, width, true, colored)
}

fn render_with_bat_grid(
    title: &str,
    bytes: &[u8],
    ranges: LineRanges,
    width: usize,
    grid: bool,
) -> Option<String> {
    render_with_bat_grid_colored(title, bytes, ranges, width, grid, color_enabled())
}

fn render_with_bat_grid_colored(
    title: &str,
    bytes: &[u8],
    ranges: LineRanges,
    width: usize,
    grid: bool,
    colored: bool,
) -> Option<String> {
    let mut out = String::new();
    PrettyPrinter::new()
        .input(Input::from_bytes(bytes).name(title))
        .colored_output(colored)
        .true_color(colored)
        .line_numbers(true)
        .grid(grid)
        .header(false)
        .rule(false)
        .strip_ansi(StripAnsiMode::Always)
        .wrapping_mode(WrappingMode::Character)
        .line_ranges(ranges)
        .term_width(width)
        .print_with_writer(Some(&mut out))
        .ok()?;
    Some(out)
}

fn render_plain_with_bat(title: &str, bytes: &[u8], width: usize) -> Option<String> {
    render_plain_with_bat_colored(title, bytes, width, color_enabled())
}

fn render_plain_with_bat_colored(
    title: &str,
    bytes: &[u8],
    width: usize,
    colored: bool,
) -> Option<String> {
    let mut out = String::new();
    PrettyPrinter::new()
        .input(Input::from_bytes(bytes).name(title))
        .colored_output(colored)
        .true_color(colored)
        .line_numbers(false)
        .grid(false)
        .header(false)
        .rule(false)
        .strip_ansi(StripAnsiMode::Always)
        .wrapping_mode(WrappingMode::Character)
        .term_width(width)
        .print_with_writer(Some(&mut out))
        .ok()?;
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
    if text.is_empty() {
        String::new()
    } else {
        sanitize_terminal(text)
    }
}

fn fallback_preview_lines(lines: &[(usize, &str)]) -> String {
    let mut out = String::new();
    for (line, text) in lines {
        let _ = writeln!(
            out,
            "{:>4} │ {}",
            line,
            sanitize_terminal(text).trim_end_matches('\n')
        );
    }
    out
}

pub fn diff(text: &str) -> String {
    let sanitized = sanitize_terminal(text);
    if let Some(rendered) =
        render_plain_with_bat("changes.diff", sanitized.as_bytes(), terminal_width())
    {
        return preserve_trailing(text, rendered);
    }
    if !color_enabled() {
        return sanitized;
    }
    let mut out = String::new();
    for line in sanitized.lines() {
        let rendered = if line.starts_with("+++") || line.starts_with("---") {
            bold(line)
        } else if line.starts_with("@@") {
            cyan(line)
        } else if line.starts_with('+') {
            green(line)
        } else if line.starts_with('-') {
            red(line)
        } else {
            line.to_string()
        };
        let _ = writeln!(out, "{rendered}");
    }
    preserve_trailing(text, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{OutputMode, set_output_mode};

    fn strip_ansi(text: &str) -> String {
        strip_ansi_escapes::strip_str(text)
    }

    #[test]
    fn code_preview_uses_bat_line_numbers_and_requested_start_line() {
        set_output_mode(OutputMode::Normal);
        let preview = strip_ansi(&code("demo.rs", "\tfn main() {}\n\t\tprintln!(\"hi\");", 7));
        assert!(preview.starts_with("── demo.rs\n"), "{preview}");
        assert!(preview.contains("   7"), "{preview}");
        assert!(preview.contains("   8"), "{preview}");
        assert!(preview.contains("fn main()"), "{preview}");
        assert!(preview.contains("println!"), "{preview}");
        assert!(!preview.contains("   1"), "{preview}");
    }

    #[test]
    fn code_preview_wraps_long_lines_with_bat() {
        set_output_mode(OutputMode::Normal);
        let preview = strip_ansi(&code(
            "src/audit.rs",
            r#"        .filter(|line| !line.starts_with(&format!("> {}", prompts::AUDIT_TRANSPARENCY_PREFIX)))"#,
            394,
        ));
        assert!(preview.contains("394"), "{preview}");
        assert!(preview.contains("filter"), "{preview}");
        assert!(preview.lines().count() > 2, "{preview}");
        assert!(
            !preview.contains('…'),
            "bat should wrap instead of ellipsizing: {preview}"
        );
    }

    #[test]
    fn text_block_uses_bat_for_numbering() {
        set_output_mode(OutputMode::Normal);
        let preview = strip_ansi(&text_block("stdout", "alpha\nbeta"));
        assert!(preview.starts_with("── stdout\n"), "{preview}");
        assert!(preview.contains("1"), "{preview}");
        assert!(preview.contains("2"), "{preview}");
        assert!(preview.contains("alpha"), "{preview}");
        assert!(preview.contains("beta"), "{preview}");
    }

    #[test]
    fn bat_preview_strips_untrusted_escape_bytes() {
        set_output_mode(OutputMode::Normal);
        let preview = strip_ansi(&code("README.md", "# ok\n\x1b[2J", 1));
        assert!(preview.contains("# ok"), "{preview}");
        assert!(!preview.contains("\x1b[2J"), "{preview}");
        assert!(!preview.contains("␛[2J"), "{preview}");
    }

    #[test]
    fn bat_preview_keeps_line_range_to_requested_slice() {
        set_output_mode(OutputMode::Normal);
        let preview = strip_ansi(&code(
            "src/demo.rs",
            "let a = 1;\nlet b = 2;\nlet c = 3;",
            40,
        ));
        assert!(preview.contains("40"), "{preview}");
        assert!(preview.contains("41"), "{preview}");
        assert!(preview.contains("42"), "{preview}");
        assert!(!preview.contains("39"), "{preview}");
        assert!(!preview.contains("43"), "{preview}");
    }

    #[test]
    fn code_lines_uses_bat_line_numbers_without_repeating_titles() {
        let preview = strip_ansi(&code_lines(
            "demo.rs",
            &[(7, "fn main() {}"), (9, "println!(\"hi\");")],
        ));
        assert!(preview.starts_with("── demo.rs\n"), "{preview}");
        assert!(preview.contains("   7 fn main()"), "{preview}");
        assert!(preview.contains("   9 println!"), "{preview}");
        assert_eq!(preview.matches("── demo.rs").count(), 1, "{preview}");
        assert!(!preview.contains("   8"), "{preview}");
    }

    #[test]
    fn bat_preview_auto_detects_language_from_filename() {
        let ranges = LineRanges::from(vec![LineRange::new(1, 1)]);
        let rust = render_with_bat_colored("demo.rs", b"fn main() {}\n", ranges.clone(), 80, true)
            .expect("bat should render Rust-named input");
        let plain = render_with_bat_colored("demo.unknown", b"fn main() {}\n", ranges, 80, true)
            .expect("bat should render unknown-named input");

        assert_eq!(strip_ansi(&rust), strip_ansi(&plain));
        assert_ne!(rust, plain, "bat should syntax-highlight .rs differently");
    }

    #[test]
    fn bat_plain_render_auto_detects_language_from_shebang() {
        let script = render_plain_with_bat_colored(
            "script",
            b"#!/usr/bin/env python\nprint('hi')\n",
            80,
            true,
        )
        .expect("bat should render shebang input");
        let plain = render_plain_with_bat_colored("script", b"print('hi')\n", 80, true)
            .expect("bat should render plain input");

        assert_ne!(script, plain, "bat should use first-line shebang detection");
        assert_eq!(strip_ansi(&script), "#!/usr/bin/env python\nprint('hi')\n");
    }

    #[test]
    fn diff_preview_sanitizes_escape_bytes_even_without_color() {
        set_output_mode(OutputMode::Normal);
        assert_eq!(diff(" context \x1b[2J\n"), " context ␛[2J\n");
    }
}
