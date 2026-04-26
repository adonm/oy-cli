use std::fmt::{Display, Write as _};
use std::io::IsTerminal as _;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use console::style;
use std::sync::LazyLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::{LinesWithEndings, as_24_bit_terminal_escaped};
use termimad::MadSkin;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Quiet = 0,
    Normal = 1,
    Verbose = 2,
    Json = 3,
}

static OUTPUT_MODE: AtomicU8 = AtomicU8::new(OutputMode::Normal as u8);

static COLOR: LazyLock<bool> = LazyLock::new(|| {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    match std::env::var("OY_COLOR").ok().as_deref() {
        Some("always") => true,
        Some("never") => false,
        _ => std::io::stdout().is_terminal(),
    }
});

pub fn init_output_mode(mode: Option<OutputMode>) {
    let mode = mode
        .or_else(output_mode_from_env)
        .unwrap_or(OutputMode::Normal);
    set_output_mode(mode);
}

pub fn set_output_mode(mode: OutputMode) {
    OUTPUT_MODE.store(mode as u8, Ordering::Relaxed);
}

pub fn output_mode() -> OutputMode {
    match OUTPUT_MODE.load(Ordering::Relaxed) {
        0 => OutputMode::Quiet,
        2 => OutputMode::Verbose,
        3 => OutputMode::Json,
        _ => OutputMode::Normal,
    }
}

pub fn is_quiet() -> bool {
    matches!(output_mode(), OutputMode::Quiet | OutputMode::Json)
}

pub fn is_json() -> bool {
    matches!(output_mode(), OutputMode::Json)
}

pub fn is_verbose() -> bool {
    matches!(output_mode(), OutputMode::Verbose)
}

fn output_mode_from_env() -> Option<OutputMode> {
    if truthy_env("OY_QUIET") {
        return Some(OutputMode::Quiet);
    }
    if truthy_env("OY_VERBOSE") {
        return Some(OutputMode::Verbose);
    }
    match std::env::var("OY_OUTPUT")
        .ok()?
        .to_ascii_lowercase()
        .as_str()
    {
        "quiet" => Some(OutputMode::Quiet),
        "verbose" => Some(OutputMode::Verbose),
        "json" => Some(OutputMode::Json),
        "normal" => Some(OutputMode::Normal),
        _ => None,
    }
}

fn truthy_env(name: &str) -> bool {
    matches!(
        std::env::var(name).ok().as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

pub fn terminal_width() -> usize {
    terminal_size::terminal_size()
        .map(|(terminal_size::Width(width), _)| width as usize)
        .filter(|width| *width >= 40)
        .unwrap_or(100)
}

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEME: LazyLock<Theme> = LazyLock::new(|| {
    let themes = ThemeSet::load_defaults();
    themes
        .themes
        .get("base16-ocean.dark")
        .or_else(|| themes.themes.values().next())
        .cloned()
        .unwrap_or_default()
});

pub fn paint(code: &str, text: impl Display) -> String {
    if *COLOR {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

fn foreground_escaped(ranges: &[(syntect::highlighting::Style, &str)]) -> String {
    as_24_bit_terminal_escaped(ranges, false)
}

pub fn out(text: &str) {
    print!("{text}");
}

pub fn err(text: &str) {
    eprint!("{text}");
}

pub fn line(text: impl Display) {
    out(&format!("{text}\n"));
}

pub fn err_line(text: impl Display) {
    err(&format!("{text}\n"));
}

pub fn markdown(text: &str) {
    if *COLOR {
        MadSkin::default().print_text(text);
    } else {
        out(text);
    }
}

pub fn code(path: &str, text: &str, first_line: usize) -> String {
    let mut out = String::new();
    let syntax = Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(|ext| SYNTAXES.find_syntax_by_extension(ext))
        .unwrap_or_else(|| SYNTAXES.find_syntax_plain_text());
    let width = first_line
        .saturating_add(text.lines().count())
        .max(1)
        .to_string()
        .len();
    if *COLOR {
        let mut highlighter = HighlightLines::new(syntax, &THEME);
        for (idx, line) in LinesWithEndings::from(text).enumerate() {
            let escaped = highlighter
                .highlight_line(line, &SYNTAXES)
                .map(|ranges| foreground_escaped(&ranges[..]))
                .unwrap_or_else(|_| line.to_string());
            let _ = write!(
                out,
                "{} │ {}",
                style(format!("{:>width$}", first_line + idx)).dim(),
                escaped
            );
        }
    } else {
        for (idx, line) in text.lines().enumerate() {
            let _ = writeln!(out, "{:>width$} │ {line}", first_line + idx);
        }
    }
    out.trim_end().to_string()
}

pub fn diff(text: &str) -> String {
    if !*COLOR {
        return text.to_string();
    }
    let mut out = String::new();
    for line in text.lines() {
        let styled = if line.starts_with("+++") || line.starts_with("---") {
            style(line).bold().to_string()
        } else if line.starts_with('+') {
            style(line).green().to_string()
        } else if line.starts_with('-') {
            style(line).red().to_string()
        } else if line.starts_with("@@") {
            style(line).cyan().to_string()
        } else {
            line.to_string()
        };
        let _ = writeln!(out, "{styled}");
    }
    out.trim_end().to_string()
}

pub fn section(title: &str) {
    line(paint("1", title));
}

pub fn kv(key: &str, value: impl Display) {
    line(format_args!(
        "  {} {value}",
        paint("2", format_args!("{key:<11}"))
    ));
}

pub fn success(text: impl Display) {
    line(format_args!("{} {text}", paint("32", "✓")));
}

pub fn warn(text: impl Display) {
    line(format_args!("{} {text}", paint("33", "!")));
}

pub fn tool_batch(round: usize, count: usize) {
    if is_quiet() {
        return;
    }
    err_line(format_args!(
        "{} tools round {round} · {count} call{}",
        paint("35", "↻"),
        if count == 1 { "" } else { "s" }
    ));
}

pub fn tool_start(name: &str, detail: &str) {
    if is_quiet() {
        return;
    }
    if detail.is_empty() {
        err_line(format_args!("{} tool {name}", paint("36", "→")));
    } else {
        err_line(format_args!("{} tool {name} · {detail}", paint("36", "→")));
    }
}

pub fn tool_result(name: &str, elapsed: Duration, preview: &str) {
    if is_quiet() {
        return;
    }
    let preview = preview.trim_end();
    let head = format!(
        "{} tool {name} {}",
        paint("32", "←"),
        format_elapsed(elapsed)
    );
    let Some((first, rest)) = preview.split_once('\n') else {
        if preview.is_empty() {
            err_line(head);
        } else {
            err_line(format_args!("{head} · {preview}"));
        }
        return;
    };
    err_line(format_args!("{head} · {first}"));
    for line in rest.lines() {
        err_line(format_args!("  {line}"));
    }
}

pub fn tool_error(name: &str, elapsed: Duration, err: impl Display) {
    if is_quiet() {
        return;
    }
    err_line(format_args!(
        "{} tool {name} {} · {err:#}",
        paint("31", "✗"),
        format_elapsed(elapsed)
    ));
}

fn format_elapsed(elapsed: Duration) -> String {
    if elapsed.as_millis() < 1000 {
        format!("{}ms", elapsed.as_millis())
    } else {
        format!("{:.1}s", elapsed.as_secs_f64())
    }
}

pub fn compact_spaces(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn truncate_chars(text: &str, max: usize) -> String {
    truncate_width(text, max)
}

pub fn truncate_width(text: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
    let ellipsis = "…";
    let limit = max_width.saturating_sub(UnicodeWidthStr::width(ellipsis));
    let mut out = String::new();
    let mut width = 0usize;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_width > limit {
            break;
        }
        width += ch_width;
        out.push(ch);
    }
    out.push_str(ellipsis);
    out
}

pub fn compact_preview(text: &str, max: usize) -> String {
    truncate_width(&compact_spaces(text), max)
}

pub fn clamp_lines(text: &str, max_lines: usize, max_cols: usize) -> String {
    let mut out = String::new();
    let lines = text.lines().collect::<Vec<_>>();
    for line in lines.iter().take(max_lines) {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&truncate_width(line, max_cols));
    }
    if lines.len() > max_lines {
        let _ = write!(out, "\n… {} more lines", lines.len() - max_lines);
    }
    out
}

#[allow(dead_code)]
pub fn wrap_line(text: &str, indent: &str) -> String {
    let width = terminal_width().saturating_sub(indent.width()).max(20);
    textwrap::wrap(text, width)
        .into_iter()
        .map(|line| format!("{indent}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn head_tail(text: &str, max_chars: usize) -> (String, bool) {
    if text.chars().count() <= max_chars {
        return (text.to_string(), false);
    }
    let head_len = max_chars / 2;
    let tail_len = max_chars.saturating_sub(head_len);
    let head = text.chars().take(head_len).collect::<String>();
    let tail = text
        .chars()
        .rev()
        .take(tail_len)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    let hidden = text
        .chars()
        .count()
        .saturating_sub(head.chars().count() + tail.chars().count());
    (
        format!("{head}\n… [truncated {hidden} chars] …\n{tail}"),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use syntect::highlighting::{Color, FontStyle, Style};

    #[test]
    fn syntax_escape_does_not_emit_background_colors() {
        let style = Style {
            foreground: Color {
                r: 1,
                g: 2,
                b: 3,
                a: 0xff,
            },
            background: Color {
                r: 4,
                g: 5,
                b: 6,
                a: 0xff,
            },
            font_style: FontStyle::empty(),
        };
        let escaped = foreground_escaped(&[(style, "let")]);
        assert!(escaped.contains("38;2;1;2;3"));
        assert!(!escaped.contains("48;2"));
    }

    #[test]
    fn elapsed_format_is_compact() {
        assert_eq!(format_elapsed(Duration::from_millis(42)), "42ms");
        assert_eq!(format_elapsed(Duration::from_millis(1250)), "1.2s");
    }
}
