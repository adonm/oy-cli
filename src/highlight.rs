use std::io::IsTerminal as _;
use std::sync::LazyLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::{LinesWithEndings, as_24_bit_terminal_escaped};

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEMES: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);
static COLOR_MODE: LazyLock<ColorMode> = LazyLock::new(detect_color_mode);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColorMode {
    Dark,
    Light,
}

pub fn stdout(text: &str) {
    print!("{}", for_stdout(text));
}

pub fn stderr(text: &str) {
    eprint!("{}", for_stderr(text));
}

pub fn for_stdout(text: &str) -> String {
    if std::io::stdout().is_terminal() {
        highlight(text, None)
    } else {
        text.to_string()
    }
}

pub fn for_stderr(text: &str) -> String {
    if std::io::stderr().is_terminal() {
        highlight(text, None)
    } else {
        text.to_string()
    }
}

#[allow(dead_code)]
pub fn for_path(text: &str, path: &str) -> String {
    if std::io::stdout().is_terminal() {
        highlight(text, syntax_for_path(path))
    } else {
        text.to_string()
    }
}

fn highlight(text: &str, syntax: Option<&'static SyntaxReference>) -> String {
    let syntax = syntax.unwrap_or_else(|| syntax_for_text(text));
    let mut highlighter = HighlightLines::new(syntax, theme());
    let mut out = String::with_capacity(text.len());
    for line in LinesWithEndings::from(text) {
        match highlighter.highlight_line(line, &SYNTAX_SET) {
            Ok(ranges) => out.push_str(&as_24_bit_terminal_escaped(&ranges[..], false)),
            Err(_) => out.push_str(line),
        }
    }
    out
}

fn theme() -> &'static Theme {
    let names = match *COLOR_MODE {
        ColorMode::Dark => ["base16-ocean.dark", "Solarized (dark)", "InspiredGitHub"],
        ColorMode::Light => ["InspiredGitHub", "Solarized (light)", "base16-ocean.dark"],
    };
    names
        .iter()
        .find_map(|name| THEMES.themes.get(*name))
        .or_else(|| THEMES.themes.values().next())
        .expect("syntect default theme set is empty")
}

fn detect_color_mode() -> ColorMode {
    if let Ok(value) = std::env::var("OY_THEME") {
        match value.trim().to_ascii_lowercase().as_str() {
            "light" => return ColorMode::Light,
            "dark" => return ColorMode::Dark,
            _ => {}
        }
    }
    if let Ok(value) = std::env::var("COLORFGBG") {
        if let Some(mode) = color_mode_from_colorfgbg(&value) {
            return mode;
        }
    }
    if env_contains("TERMINAL_THEME", "light")
        || env_contains("TERM_THEME", "light")
        || env_contains("ITERM_PROFILE", "light")
    {
        return ColorMode::Light;
    }
    if env_contains("TERMINAL_THEME", "dark")
        || env_contains("TERM_THEME", "dark")
        || env_contains("ITERM_PROFILE", "dark")
    {
        return ColorMode::Dark;
    }
    ColorMode::Dark
}

fn color_mode_from_colorfgbg(value: &str) -> Option<ColorMode> {
    let bg = value
        .rsplit([';', ':'])
        .next()
        .and_then(|v| v.parse::<u8>().ok())?;
    Some(if matches!(bg, 0..=6 | 8) {
        ColorMode::Dark
    } else {
        ColorMode::Light
    })
}

fn env_contains(name: &str, needle: &str) -> bool {
    std::env::var(name)
        .map(|value| value.to_ascii_lowercase().contains(needle))
        .unwrap_or(false)
}

fn syntax_for_path(path: &str) -> Option<&'static SyntaxReference> {
    SYNTAX_SET
        .find_syntax_for_file(path)
        .ok()
        .flatten()
        .or_else(|| {
            path.rsplit_once('.')
                .and_then(|(_, ext)| SYNTAX_SET.find_syntax_by_extension(ext))
        })
}

fn syntax_for_text(text: &str) -> &'static SyntaxReference {
    if looks_like_json(text) {
        return SYNTAX_SET
            .find_syntax_by_extension("json")
            .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());
    }
    if looks_like_toml(text) {
        return SYNTAX_SET
            .find_syntax_by_extension("toml")
            .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());
    }
    if looks_like_shell(text) {
        return SYNTAX_SET
            .find_syntax_by_extension("sh")
            .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());
    }
    SYNTAX_SET
        .find_syntax_by_token("Markdown")
        .or_else(|| SYNTAX_SET.find_syntax_by_extension("md"))
        .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text())
}

fn looks_like_json(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with('{') || trimmed.starts_with('[')
}

fn looks_like_toml(text: &str) -> bool {
    text.lines()
        .take(8)
        .any(|line| line.trim_start().starts_with('[') && line.trim_end().ends_with(']'))
}

fn looks_like_shell(text: &str) -> bool {
    text.lines().take(4).any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with('$') || trimmed.starts_with("exit ") || trimmed.starts_with("stdout:")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorfgbg_detects_light_background() {
        assert_eq!(color_mode_from_colorfgbg("15;0"), Some(ColorMode::Dark));
        assert_eq!(color_mode_from_colorfgbg("0;15"), Some(ColorMode::Light));
    }
}
