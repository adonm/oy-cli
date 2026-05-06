use std::borrow::Cow;
use std::fmt::Write as _;
use std::sync::LazyLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::as_24_bit_terminal_escaped;
use unicode_width::UnicodeWidthChar;

use super::text::{ansi_stripped_width, truncate_width};
use super::{bold, color_enabled, cyan, faint, green, path, red, terminal_width};

pub fn markdown(text: &str) {
    super::out(&render_markdown(text));
}

fn render_markdown(text: &str) -> String {
    let text = strip_escapes(text);
    if !color_enabled() {
        return text;
    }
    let mut in_fence = false;
    let mut out = String::new();
    for line in text.lines() {
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
    if text.ends_with('\n') {
        out
    } else {
        out.trim_end_matches('\n').to_string()
    }
}

fn strip_escapes(text: &str) -> String {
    if text.contains('\x1b') {
        text.replace('\x1b', "␛")
    } else {
        text.to_string()
    }
}

pub fn code(path: &str, text: &str, first_line: usize) -> String {
    numbered_block(path, &normalize_code_preview_text(text), first_line)
}

pub fn text_block(title: &str, text: &str) -> String {
    numbered_block(title, text, 1)
}

pub fn block_title(title: &str) -> String {
    path(format_args!("── {title}"))
}

#[cfg(test)]
fn numbered_line(line_number: usize, width: usize, text: &str) -> String {
    numbered_line_with_max_width(line_number, width, text, usize::MAX)
}

fn numbered_line_with_max_width(
    line_number: usize,
    width: usize,
    text: &str,
    max_width: usize,
) -> String {
    let text = normalize_code_preview_text(text);
    let prefix = format!(
        "{} {} ",
        faint(format_args!("{line_number:>width$}")),
        faint("│")
    );
    let available = max_width
        .saturating_sub(ansi_stripped_width(&prefix))
        .max(1);
    format!("{prefix}{}", truncate_width(&text, available))
}

fn normalize_code_preview_text(text: &str) -> Cow<'_, str> {
    const TAB_WIDTH: usize = 4;
    if !text.contains('\t') {
        return Cow::Borrowed(text);
    }

    let mut out = String::with_capacity(text.len());
    let mut column = 0usize;
    for ch in text.chars() {
        match ch {
            '\t' => {
                let spaces = TAB_WIDTH - (column % TAB_WIDTH);
                out.extend(std::iter::repeat_n(' ', spaces));
                column += spaces;
            }
            '\n' | '\r' => {
                out.push(ch);
                column = 0;
            }
            _ => {
                out.push(ch);
                column += UnicodeWidthChar::width(ch).unwrap_or(0);
            }
        }
    }
    Cow::Owned(out)
}

fn numbered_block(title: &str, text: &str, first_line: usize) -> String {
    let title = if title.is_empty() { "text" } else { title };
    let line_count = text.lines().count().max(1);
    let width = first_line
        .saturating_add(line_count.saturating_sub(1))
        .max(1)
        .to_string()
        .len();
    let max_width = terminal_width().saturating_sub(4).max(40);
    let code_width = max_width.saturating_sub(width + 3).max(1);
    let mut out = String::new();
    let _ = writeln!(out, "{}", truncate_width(&block_title(title), max_width));
    if text.is_empty() {
        let _ = writeln!(
            out,
            "{}",
            numbered_line_with_max_width(first_line, width, "", max_width)
        );
    } else {
        let display_text = text
            .lines()
            .map(|line| truncate_width(line, code_width))
            .collect::<Vec<_>>()
            .join("\n");
        let highlighted = highlighted_block(title, &display_text);
        let lines = highlighted.as_deref().unwrap_or(&display_text).lines();
        for (idx, line) in lines.enumerate() {
            let _ = writeln!(
                out,
                "{}",
                numbered_line_with_max_width(first_line + idx, width, line, max_width)
            );
        }
    }
    out.trim_end().to_string()
}

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

fn highlighted_block(title: &str, text: &str) -> Option<String> {
    if !color_enabled() {
        return None;
    }
    let syntax = syntax_for_title(title)?;
    let theme = terminal_theme()?;
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut out = String::new();
    for line in text.lines() {
        let ranges = highlighter.highlight_line(line, &SYNTAX_SET).ok()?;
        let _ = writeln!(out, "{}", as_24_bit_terminal_escaped(&ranges, false));
    }
    Some(if text.ends_with('\n') {
        out
    } else {
        out.trim_end_matches('\n').to_string()
    })
}

fn syntax_for_title(title: &str) -> Option<&'static syntect::parsing::SyntaxReference> {
    let syntaxes = &*SYNTAX_SET;
    let name = title.rsplit('/').next().unwrap_or(title);
    if let Some(ext) = name.rsplit_once('.').map(|(_, ext)| ext) {
        syntaxes.find_syntax_by_extension(ext)
    } else {
        syntaxes.find_syntax_by_token(name)
    }
    .or_else(|| syntaxes.find_syntax_by_name(title))
}

fn terminal_theme() -> Option<&'static Theme> {
    THEME_SET
        .themes
        .get("base16-ocean.dark")
        .or_else(|| THEME_SET.themes.values().next())
}

pub fn diff(text: &str) -> String {
    if !color_enabled() {
        return text.to_string();
    }
    let mut out = String::new();
    for line in text.lines() {
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
    if text.ends_with('\n') {
        out
    } else {
        out.trim_end_matches('\n').to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{OutputMode, set_output_mode};
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn numbered_line_expands_tabs_to_stable_columns() {
        set_output_mode(OutputMode::Normal);
        assert_eq!(numbered_line(7, 1, "\tlet x = 1;"), "7 │     let x = 1;");
        assert_eq!(numbered_line(8, 1, "ab\tcd"), "8 │ ab  cd");
        assert_eq!(
            code("demo.rs", "\tfn main() {}\n\t\tprintln!(\"hi\");", 1),
            "── demo.rs\n1 │     fn main() {}\n2 │         println!(\"hi\");"
        );
    }

    #[test]
    fn numbered_line_clamps_long_read_lines_to_preview_width() {
        set_output_mode(OutputMode::Normal);
        let line = numbered_line_with_max_width(
            394,
            3,
            r#"        .filter(|line| !line.starts_with(&format!("> {}", prompts::AUDIT_TRANSPARENCY_PREFIX)))"#,
            40,
        );
        assert!(UnicodeWidthStr::width(line.as_str()) <= 40, "{line}");
        assert!(line.starts_with("394 │ "));
        assert!(line.ends_with('…'));
        assert!(!line.contains('\n'));
    }

    #[test]
    fn code_preview_lines_fit_tool_result_indent_width() {
        set_output_mode(OutputMode::Normal);
        let preview = code(
            "src/audit.rs",
            r#"pub(crate) fn with_transparency_line(report: &str, snippet: &str) -> String {
    .filter(|line| !line.starts_with(&format!("> {}", prompts::AUDIT_TRANSPARENCY_PREFIX)))"#,
            390,
        );
        let max_width = terminal_width().saturating_sub(4).max(40);
        for line in preview.lines() {
            assert!(
                UnicodeWidthStr::width(line) <= max_width,
                "line exceeded {max_width}: {line}"
            );
        }
    }
}
