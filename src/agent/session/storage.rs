//! Saved-session persistence via config-path helpers.
//!
//! Sessions are serialised to private files in `~/.config/oy-rust/sessions/`.
//! [`load_saved`] restores a previous session with compaction applied.

use anyhow::{Context, Result, bail};
use std::path::Path;

use super::{Session, Transcript};
use crate::config::{self, SafetyMode};

pub fn load_saved(
    name: Option<&str>,
    interactive: bool,
    mode: SafetyMode,
) -> Result<Option<Session>> {
    let Some(path) = config::resolve_saved_session(name)? else {
        return Ok(None);
    };
    let saved = config::load_session_file(&path)?;
    let transcript: Transcript = serde_json::from_value(saved.transcript)
        .with_context(|| format!("invalid saved transcript in {}", path.display()))?;
    let root = config::oy_root()?;
    ensure_saved_workspace_matches(&path, &saved.workspace_root, &root)?;
    let mode = saved.mode.unwrap_or(mode);
    let system_prompt = config::system_prompt(interactive, mode);
    Ok(Some(Session {
        root,
        model: saved.model,
        system_prompt,
        interactive,
        mode,
        transcript,
        todos: saved.todos,
    }))
}

fn ensure_saved_workspace_matches(
    session_path: &Path,
    saved_root: &Path,
    current_root: &Path,
) -> Result<()> {
    if saved_root == current_root {
        return Ok(());
    }
    bail!(
        "saved session {} belongs to workspace {}; current workspace is {}",
        session_path.display(),
        saved_root.display(),
        current_root.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_workspace_mismatch_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let current = tempfile::tempdir().unwrap();
        let session_path = dir.path().join("session.json");
        let saved_root = dir.path().to_path_buf();

        let err =
            ensure_saved_workspace_matches(&session_path, &saved_root, current.path()).unwrap_err();

        assert!(err.to_string().contains("belongs to workspace"));
    }
}
