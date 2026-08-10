//! mise toolchain management shared by the install, upgrade, and doctor paths.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub(crate) const OPENCODE_MISE_TOOL: &str = "npm:@opencode-ai/cli";
pub(crate) const OPENCODE_MISE_SPEC: &str = "npm:@opencode-ai/cli@next";
const OPENCODE_ALLOW_BUILDS: &str = "@opencode-ai/cli";

/// Ensure every mise config that declares the `npm:@opencode-ai/cli` tool
/// allows its postinstall, which downloads the native `opencode2` binary.
/// mise's embedded package manager denies lifecycle scripts unless
/// `allow_builds` names them, so without this the installed `opencode2` is
/// only a stub. The config files come from `mise config ls`, so patching
/// follows mise's own discovery (global config, local config chain, env-
/// specific files, and filename overrides) instead of guessing names.
/// Returns whether any config changed.
pub(crate) fn ensure_opencode_allow_builds() -> Result<bool> {
    let mut changed = false;
    for path in mise_config_files()? {
        changed |= patch_config_file(&path)?;
    }
    Ok(changed)
}

/// List the config files mise reads for the current directory that declare
/// the OpenCode tool, via `mise config ls --json`.
fn mise_config_files() -> Result<Vec<PathBuf>> {
    let output = std::process::Command::new("mise")
        .args(["config", "ls", "--json"])
        .output()
        .context("failed to run `mise config ls --json`")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "`mise config ls --json` failed with exit code {}: {}",
            output.status.code().unwrap_or(1),
            stderr.trim()
        );
    }
    parse_mise_config_listing(&output.stdout)
}

fn parse_mise_config_listing(bytes: &[u8]) -> Result<Vec<PathBuf>> {
    let listing: Vec<MiseConfigFile> =
        serde_json::from_slice(bytes).context("failed to parse `mise config ls --json` output")?;
    Ok(listing
        .into_iter()
        .filter(|file| file.tools.iter().any(|tool| tool == OPENCODE_MISE_TOOL))
        .map(|file| PathBuf::from(file.path))
        .collect())
}

#[derive(Debug, Deserialize)]
struct MiseConfigFile {
    path: String,
    #[serde(default)]
    tools: Vec<String>,
}

fn patch_config_file(path: &Path) -> Result<bool> {
    let config = match std::fs::read_to_string(path) {
        Ok(config) => config,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", path.display())),
    };
    let patched = patch_opencode_entry(&config);
    if patched == config {
        return Ok(false);
    }
    std::fs::write(path, patched).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(true)
}

/// Replace the plain `"npm:@opencode-ai/cli" = "next"` line written by
/// `mise use` with the equivalent entry that carries `allow_builds`. Entries
/// that already carry tool options are left untouched.
fn patch_opencode_entry(config: &str) -> String {
    let plain = format!("{OPENCODE_MISE_TOOL:?} = \"next\"");
    let entry = format!(
        "{OPENCODE_MISE_TOOL:?} = {{ version = \"next\", allow_builds = [{OPENCODE_ALLOW_BUILDS:?}] }}"
    );
    config.replace(&plain, &entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches_the_plain_entry_written_by_mise_use() {
        let config = "[tools]\n\"npm:@opencode-ai/cli\" = \"next\"\nnode = \"latest\"\n";
        assert_eq!(
            patch_opencode_entry(config),
            "[tools]\n\"npm:@opencode-ai/cli\" = { version = \"next\", allow_builds = [\"@opencode-ai/cli\"] }\nnode = \"latest\"\n"
        );
    }

    #[test]
    fn leaves_entries_that_already_allow_builds_untouched() {
        let config = "[tools]\n\"npm:@opencode-ai/cli\" = { version = \"next\", allow_builds = [\"@opencode-ai/cli\"] }\n";
        assert_eq!(patch_opencode_entry(config), config);
    }

    #[test]
    fn leaves_configs_without_the_tool_untouched() {
        let config = "[tools]\nnode = \"latest\"\n";
        assert_eq!(patch_opencode_entry(config), config);
    }

    #[test]
    fn config_listing_keeps_only_files_declaring_the_opencode_tool() {
        let listing = br#"[
  {
    "path": "/work/project/.mise.toml",
    "tools": ["rust", "npm:@opencode-ai/cli"]
  },
  {
    "path": "/work/project/mise.dev.toml",
    "tools": ["node"]
  },
  {
    "path": "/home/user/.config/mise/config.toml",
    "tools": ["npm:@opencode-ai/cli", "just"]
  }
]"#;
        assert_eq!(
            parse_mise_config_listing(listing).unwrap(),
            vec![
                PathBuf::from("/work/project/.mise.toml"),
                PathBuf::from("/home/user/.config/mise/config.toml"),
            ]
        );
    }

    #[test]
    fn config_listing_accepts_entries_without_a_tools_list() {
        let listing = br#"[{"path": "/work/project/.mise.toml"}]"#;
        assert_eq!(
            parse_mise_config_listing(listing).unwrap(),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn patches_a_plain_local_config_file_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mise.toml");
        std::fs::write(
            &path,
            "[tools]\n\"npm:@opencode-ai/cli\" = \"next\"\nnode = \"latest\"\n",
        )
        .unwrap();
        assert!(patch_config_file(&path).unwrap());
        let patched = std::fs::read_to_string(&path).unwrap();
        assert!(patched.contains("allow_builds"));
        assert!(
            !patch_config_file(&path).unwrap(),
            "second patch must be a no-op"
        );
    }
}
