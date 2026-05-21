//! Terminal title and zellij pane-name updates.
//!
//! Title updates are best-effort UI side effects for human terminal sessions.
//! They are disabled for quiet/JSON output and escape all user-controlled text
//! before writing OSC sequences.

use std::fmt::Display;
use std::io::{IsTerminal as _, Write as _};
use std::sync::{LazyLock, Mutex};

use super::OutputMode;

static TITLE_STATE: LazyLock<Mutex<TitleState>> =
    LazyLock::new(|| Mutex::new(TitleState::default()));

#[derive(Debug, Default)]
struct TitleState {
    stack: Vec<String>,
}

#[derive(Debug)]
pub struct TitleGuard {
    active: bool,
}

impl Drop for TitleGuard {
    fn drop(&mut self) {
        if self.active {
            pop_title_scope();
        }
    }
}

pub fn title_scope(title: impl Display) -> TitleGuard {
    if !title_enabled() {
        return TitleGuard { active: false };
    }
    let title = title_text(&title.to_string());
    let mut state = TITLE_STATE.lock().expect("title state mutex poisoned");
    let first = state.stack.is_empty();
    state.stack.push(title.clone());
    drop(state);

    if first {
        osc_push_title();
    }
    set_title(&title);
    TitleGuard { active: true }
}

pub fn title_progress(title: impl Display) {
    if !title_enabled() {
        return;
    }
    let title = title_text(&title.to_string());
    let mut state = TITLE_STATE.lock().expect("title state mutex poisoned");
    let Some(current) = state.stack.last_mut() else {
        return;
    };
    if current == &title {
        return;
    }
    *current = title.clone();
    drop(state);

    set_title(&title);
}

fn pop_title_scope() {
    let mut state = TITLE_STATE.lock().expect("title state mutex poisoned");
    state.stack.pop();
    let previous = state.stack.last().cloned();
    drop(state);

    if let Some(previous) = previous {
        set_title(&previous);
        return;
    }

    osc_pop_title();
    zellij_undo_rename_pane();
}

fn set_title(title: &str) {
    osc_set_title(title);
    zellij_rename_pane(title);
}

fn title_enabled() -> bool {
    title_enabled_for(
        super::output_mode(),
        std::io::stderr().is_terminal(),
        std::env::var("OY_TITLE").ok().as_deref(),
    )
}

fn title_enabled_for(mode: OutputMode, stderr_is_terminal: bool, env: Option<&str>) -> bool {
    if matches!(mode, OutputMode::Quiet | OutputMode::Json) {
        return false;
    }
    match env.map(str::to_ascii_lowercase).as_deref() {
        Some("0" | "false" | "no" | "off" | "never") => return false,
        Some("1" | "true" | "yes" | "on" | "always") => return true,
        _ => {}
    }
    stderr_is_terminal && matches!(mode, OutputMode::Normal | OutputMode::Verbose)
}

fn osc_set_title(title: &str) {
    write_osc(format_args!("\x1b]2;{title}\x07"));
}

fn osc_push_title() {
    write_osc(format_args!("\x1b]22;0\x07"));
}

fn osc_pop_title() {
    write_osc(format_args!("\x1b]23;0\x07"));
}

fn write_osc(args: std::fmt::Arguments<'_>) {
    let _ = std::io::stderr().write_fmt(args);
    let _ = std::io::stderr().flush();
}

fn zellij_rename_pane(title: &str) {
    if std::env::var_os("ZELLIJ").is_none() {
        return;
    }
    let _ = std::process::Command::new("zellij")
        .args(["action", "rename-pane", title])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

fn zellij_undo_rename_pane() {
    if std::env::var_os("ZELLIJ").is_none() {
        return;
    }
    let _ = std::process::Command::new("zellij")
        .args(["action", "undo-rename-pane"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

fn title_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(120));
    let mut previous_space = false;
    for ch in text.chars() {
        let next = if ch == '\x1b' || ch.is_control() {
            ' '
        } else {
            ch
        };
        if next.is_whitespace() {
            if !previous_space && !out.is_empty() {
                out.push(' ');
                previous_space = true;
            }
        } else {
            out.push(next);
            previous_space = false;
        }
        if out.len() >= 120 {
            break;
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_enabled_requires_human_output_by_default() {
        assert!(title_enabled_for(OutputMode::Normal, true, None));
        assert!(title_enabled_for(OutputMode::Verbose, true, None));
        assert!(!title_enabled_for(OutputMode::Quiet, true, None));
        assert!(!title_enabled_for(OutputMode::Json, true, None));
        assert!(!title_enabled_for(OutputMode::Normal, false, None));
        assert!(title_enabled_for(OutputMode::Normal, false, Some("always")));
        assert!(!title_enabled_for(OutputMode::Json, false, Some("always")));
        assert!(!title_enabled_for(OutputMode::Normal, true, Some("off")));
    }

    #[test]
    fn title_text_escapes_control_sequences_and_compacts_spaces() {
        assert_eq!(
            title_text("oy\nrun \x1b]2;bad\x07 README.md"),
            "oy run ]2;bad README.md"
        );
        assert!(title_text(&"x".repeat(200)).len() <= 120);
    }
}
