use kdam::Animation;
use std::borrow::Cow;
use std::fmt::{Display, Write as _};
use std::io::IsTerminal as _;
use std::num::NonZeroU16;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::as_24_bit_terminal_escaped;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Controls how much user-facing output `oy` writes while it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    /// Suppress normal progress output.
    Quiet = 0,
    /// Show standard human-readable progress output.
    Normal = 1,
    /// Show fuller tool previews and diagnostic context.
    Verbose = 2,
    /// Prefer machine-readable JSON where a command supports it.
    Json = 3,
}

static OUTPUT_MODE: AtomicU8 = AtomicU8::new(OutputMode::Normal as u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColorMode {
    Auto,
    Always,
    Never,
}

static COLOR_MODE: LazyLock<ColorMode> = LazyLock::new(color_mode_from_env);

pub fn init_output_mode(mode: Option<OutputMode>) {
    let mode = mode
        .or_else(output_mode_from_env)
        .unwrap_or(OutputMode::Normal);
    set_output_mode(mode);
}

/// Sets the process-wide output mode used by CLI rendering helpers.
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

fn color_mode_from_env() -> ColorMode {
    color_mode_from_values(
        std::env::var_os("NO_COLOR").is_some(),
        std::env::var("OY_COLOR").ok().as_deref(),
    )
}

fn color_mode_from_values(no_color: bool, oy_color: Option<&str>) -> ColorMode {
    if no_color {
        return ColorMode::Never;
    }
    match oy_color.map(str::to_ascii_lowercase).as_deref() {
        Some("always" | "1" | "true" | "yes" | "on") => ColorMode::Always,
        Some("never" | "0" | "false" | "no" | "off") => ColorMode::Never,
        _ => ColorMode::Auto,
    }
}

pub fn color_enabled() -> bool {
    color_enabled_for_stdout(std::io::stdout().is_terminal())
}

fn color_enabled_for_stdout(stdout_is_terminal: bool) -> bool {
    color_enabled_for_mode(*COLOR_MODE, stdout_is_terminal)
}

fn color_enabled_for_mode(mode: ColorMode, stdout_is_terminal: bool) -> bool {
    match mode {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => stdout_is_terminal,
    }
}

pub fn terminal_width() -> usize {
    terminal_size::terminal_size()
        .map(|(terminal_size::Width(width), _)| width as usize)
        .filter(|width| *width >= 40)
        .unwrap_or(100)
}

pub fn paint(code: &str, text: impl Display) -> String {
    if color_enabled() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

pub fn faint(text: impl Display) -> String {
    paint("2", text)
}

pub fn bold(text: impl Display) -> String {
    paint("1", text)
}

pub fn cyan(text: impl Display) -> String {
    paint("36", text)
}

pub fn green(text: impl Display) -> String {
    paint("32", text)
}

pub fn yellow(text: impl Display) -> String {
    paint("33", text)
}

pub fn red(text: impl Display) -> String {
    paint("31", text)
}

pub fn magenta(text: impl Display) -> String {
    paint("35", text)
}

pub fn status_text(ok: bool, text: impl Display) -> String {
    if ok { green(text) } else { red(text) }
}

pub fn bool_text(value: bool) -> String {
    status_text(value, value)
}

pub fn path(text: impl Display) -> String {
    paint("1;36", text)
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
    out(&render_markdown(text));
}

fn render_markdown(text: &str) -> String {
    if !color_enabled() {
        return text.to_string();
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
            paint("1;35", line)
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

pub fn section(title: &str) {
    line(bold(title));
}

pub fn kv(key: &str, value: impl Display) {
    line(format_args!(
        "  {} {value}",
        faint(format_args!("{key:<11}"))
    ));
}

pub fn success(text: impl Display) {
    line(format_args!("{} {text}", green("✓")));
}

pub fn warn(text: impl Display) {
    line(format_args!("{} {text}", yellow("!")));
}

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
    line(progress_line(
        label,
        current,
        total,
        &detail.to_string(),
        elapsed,
    ));
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
    let percentage = current as f32 / total as f32;
    Animation::FillUp.fmt_render(
        NonZeroU16::new(width.max(1)).expect("progress bar width is non-zero"),
        percentage,
        &None,
    )
}

pub fn tool_batch(round: usize, count: usize) {
    if is_quiet() {
        return;
    }
    err_line(tool_batch_line(round, count));
}

pub fn tool_start(name: &str, detail: &str) {
    if is_quiet() {
        return;
    }
    err_line(tool_start_line(name, detail));
}

pub fn tool_result(name: &str, elapsed: Duration, preview: &str) {
    if is_quiet() {
        return;
    }
    let preview = preview.trim_end();
    let head = tool_result_head(name, elapsed);
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
    err_line(format_args!(
        "  {} {name} {} · {err:#}",
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

fn tool_batch_line(round: usize, count: usize) -> String {
    format!("{} tools r{round} ×{count}", magenta("↻"))
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

pub fn compact_spaces(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn truncate_chars(text: &str, max: usize) -> String {
    truncate_width(text, max)
}

pub fn truncate_width(text: &str, max_width: usize) -> String {
    if ansi_stripped_width(text) <= max_width {
        return text.to_string();
    }
    truncate_plain_width(text, max_width)
}

fn truncate_plain_width(text: &str, max_width: usize) -> String {
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

fn ansi_stripped_width(text: &str) -> usize {
    let mut width = 0usize;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
        } else {
            width += UnicodeWidthChar::width(ch).unwrap_or(0);
        }
    }
    width
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

    fn color_mode_name(mode: ColorMode) -> &'static str {
        match mode {
            ColorMode::Auto => "auto",
            ColorMode::Always => "always",
            ColorMode::Never => "never",
        }
    }

    #[test]
    fn color_mode_env_parsing() {
        assert_eq!(color_mode_name(color_mode_from_values(false, None)), "auto");
        assert_eq!(
            color_mode_name(color_mode_from_values(false, Some("always"))),
            "always"
        );
        assert_eq!(
            color_mode_name(color_mode_from_values(false, Some("on"))),
            "always"
        );
        assert_eq!(
            color_mode_name(color_mode_from_values(false, Some("off"))),
            "never"
        );
        assert_eq!(
            color_mode_name(color_mode_from_values(true, Some("always"))),
            "never"
        );
    }

    #[test]
    fn color_auto_requires_terminal() {
        assert!(!color_enabled_for_mode(ColorMode::Auto, false));
        assert!(color_enabled_for_mode(ColorMode::Auto, true));
        assert!(color_enabled_for_mode(ColorMode::Always, false));
        assert!(!color_enabled_for_mode(ColorMode::Never, true));
    }

    #[test]
    fn elapsed_format_is_compact() {
        assert_eq!(format_duration(Duration::from_millis(42)), "42ms");
        assert_eq!(format_duration(Duration::from_millis(1250)), "1.2s");
    }

    #[test]
    fn progress_line_shows_bar_count_detail_and_elapsed() {
        set_output_mode(OutputMode::Normal);
        assert_eq!(progress_bar(2, 4, 8), "|████▂   |");
        assert_eq!(
            progress_line("review", 2, 4, "chunk 3", Duration::from_millis(1250)),
            "  |█████████▂        | 2/4 review · chunk 3 · 1.2s"
        );
    }

    #[test]
    fn tool_progress_lines_are_dense() {
        set_output_mode(OutputMode::Normal);
        assert_eq!(tool_batch_line(2, 3), "↻ tools r2 ×3");
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
