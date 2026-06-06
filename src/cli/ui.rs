//! Minimal terminal output helpers for the OpenCode wrapper.

use std::fmt::Display;
use std::io::IsTerminal as _;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU8, Ordering};

mod text;

pub use text::truncate_chars;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Quiet = 0,
    Normal = 1,
    Verbose = 2,
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

pub fn set_output_mode(mode: OutputMode) {
    OUTPUT_MODE.store(mode as u8, Ordering::Relaxed);
}

pub fn is_json() -> bool {
    matches!(output_mode(), OutputMode::Json)
}

fn output_mode() -> OutputMode {
    match OUTPUT_MODE.load(Ordering::Relaxed) {
        0 => OutputMode::Quiet,
        2 => OutputMode::Verbose,
        3 => OutputMode::Json,
        _ => OutputMode::Normal,
    }
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
    if std::env::var_os("NO_COLOR").is_some() {
        return ColorMode::Never;
    }
    match std::env::var("OY_COLOR")
        .ok()
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        Some("always" | "1" | "true" | "yes" | "on") => ColorMode::Always,
        Some("never" | "0" | "false" | "no" | "off") => ColorMode::Never,
        _ => ColorMode::Auto,
    }
}

fn color_enabled() -> bool {
    match *COLOR_MODE {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => std::io::stdout().is_terminal(),
    }
}

pub fn paint(code: &str, text: impl Display) -> String {
    let text = text.to_string();
    if color_enabled() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text
    }
}

pub fn faint(text: impl Display) -> String {
    paint("2", text)
}

pub fn green(text: impl Display) -> String {
    paint("32", text)
}

pub fn red(text: impl Display) -> String {
    paint("31", text)
}

pub fn status_text(ok: bool, text: impl Display) -> String {
    if ok { green(text) } else { red(text) }
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
    line(paint("1", title));
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
