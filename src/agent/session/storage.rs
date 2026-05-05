use anyhow::{Context, Result, bail};
use std::path::Path;

use super::{Session, Transcript};
use crate::config::{self, SafetyMode};
use crate::tools::ToolPolicy;

pub fn load_saved(
    name: Option<&str>,
    interactive: bool,
    mode: SafetyMode,
    policy: ToolPolicy,
) -> Result<Option<Session>> {
    let Some(path) = config::resolve_saved_session(name)? else {
        return Ok(None);
    };
    let saved = config::load_session_file(&path)?;
    let transcript: Transcript = serde_json::from_value(saved.transcript)
        .with_context(|| format!("invalid saved transcript in {}", path.display()))?;
    let root = config::oy_root()?;
    ensure_saved_workspace_matches(&path, saved.workspace_root.as_deref(), &root)?;
    let mode = saved.mode.unwrap_or(mode);
    let system_prompt = config::system_prompt(interactive, mode);
    Ok(Some(Session {
        root,
        model: saved.model,
        system_prompt,
        interactive,
        policy,
        mode,
        transcript,
        todos: saved.todos,
    }))
}

fn ensure_saved_workspace_matches(
    session_path: &Path,
    saved_root: Option<&Path>,
    current_root: &Path,
) -> Result<()> {
    let Some(saved_root) = saved_root else {
        return Ok(());
    };
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

        let err = ensure_saved_workspace_matches(&session_path, Some(&saved_root), current.path())
            .unwrap_err();

        assert!(err.to_string().contains("belongs to workspace"));
    }

    #[test]
    fn saved_workspace_allows_legacy_without_root() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            ensure_saved_workspace_matches(&dir.path().join("session.json"), None, dir.path())
                .is_ok()
        );
    }
}
