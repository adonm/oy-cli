use std::fmt::Display;
use std::io::IsTerminal as _;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU8, Ordering};

mod progress;
mod render;
mod text;

pub use progress::{format_duration, progress, tool_error, tool_result, tool_start};
pub use render::{block_title, code, diff, markdown, text_block};
pub use text::{clamp_lines, compact_preview, compact_spaces, head_tail, truncate_chars};

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
    let text = text.to_string();
    if text.contains('\x1b') {
        return sanitize_terminal(&text);
    }
    if color_enabled() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text
    }
}

/// Strip terminal escape sequences to prevent injection from untrusted input.
fn sanitize_terminal(text: &str) -> String {
    text.replace('\x1b', "␛")
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
    fn miri_smoke_ui_color_decisions() {
        assert_eq!(color_mode_name(color_mode_from_values(false, None)), "auto");
        assert_eq!(color_mode_name(color_mode_from_values(true, None)), "never");
        assert!(!color_enabled_for_mode(ColorMode::Auto, false));
        assert!(color_enabled_for_mode(ColorMode::Auto, true));
        assert!(color_enabled_for_mode(ColorMode::Always, false));
    }

    #[test]
    fn color_auto_requires_terminal() {
        assert!(!color_enabled_for_mode(ColorMode::Auto, false));
        assert!(color_enabled_for_mode(ColorMode::Auto, true));
        assert!(color_enabled_for_mode(ColorMode::Always, false));
        assert!(!color_enabled_for_mode(ColorMode::Never, true));
    }
}
