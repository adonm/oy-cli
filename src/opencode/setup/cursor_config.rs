//! Cursor CLI configuration defaults owned by setup.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

use super::config_file::format_json;

const ATTRIBUTION_KEYS: [&str; 2] = ["attributeCommitsToAgent", "attributePRsToAgent"];

pub(super) fn config_path() -> Result<PathBuf> {
    if let Some(value) = std::env::var_os("CURSOR_CONFIG_DIR") {
        if value.is_empty() {
            bail!("CURSOR_CONFIG_DIR must not be empty");
        }
        let directory = PathBuf::from(value);
        let directory = if directory.is_absolute() {
            directory
        } else {
            std::env::current_dir()
                .context("failed to resolve relative CURSOR_CONFIG_DIR")?
                .join(directory)
        };
        return Ok(directory.join("cli-config.json"));
    }
    if let Some(value) = std::env::var_os("XDG_CONFIG_HOME")
        && !value.is_empty()
    {
        return Ok(PathBuf::from(value).join("cursor/cli-config.json"));
    }
    dirs::home_dir()
        .context("failed to find the user home directory for Cursor configuration")
        .map(|home| home.join(".cursor/cli-config.json"))
}

/// Disable Cursor's commit and PR attribution when the user has not selected
/// either behavior explicitly.
pub(super) fn install_attribution_defaults(path: &Path) -> Result<Option<String>> {
    let mut root = read_config(path)?;
    let object = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("{} must contain a JSON object", path.display()))?;
    let attribution = object
        .entry("attribution")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{} field `attribution` must contain a JSON object",
                path.display()
            )
        })?;
    let mut changed = false;
    for key in ATTRIBUTION_KEYS {
        match attribution.get(key) {
            Some(Value::Bool(_)) => {}
            Some(_) => bail!(
                "{} field `attribution.{key}` must be a boolean",
                path.display()
            ),
            None => {
                attribution.insert(key.to_string(), Value::Bool(false));
                changed = true;
            }
        }
    }
    if changed {
        format_json(&root).map(Some)
    } else {
        Ok(None)
    }
}

fn read_config(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({
            "version": 1,
            "editor": { "vimMode": false },
            "permissions": { "allow": [], "deny": [] }
        }));
    }
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        bail!(
            "refusing to read symlinked Cursor config {}",
            path.display()
        );
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&text)
        .with_context(|| format!("{} must be valid Cursor CLI JSON", path.display()))
}
