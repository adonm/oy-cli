//! mise toolchain management shared by the install, upgrade, and doctor paths.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value};

pub(crate) const OPENCODE_MISE_TOOL: &str = "npm:@opencode-ai/cli";
pub(crate) const OPENCODE_MISE_SPEC: &str = "npm:@opencode-ai/cli@beta";
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
    let Some(patched) = patch_opencode_entry(&config)
        .with_context(|| format!("failed to patch mise config {}", path.display()))?
    else {
        return Ok(false);
    };
    std::fs::write(path, patched).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(true)
}

/// Ensure the `npm:@opencode-ai/cli` entry in a mise `[tools]` table allows its
/// postinstall. Handles the shorthand `= "<version>"` form written by
/// `mise use`, inline tables, and subtables, preserving the declared version
/// and any other tool options. Returns the rewritten config when it changed,
/// or `None` when the entry is already correct or the tool is not declared.
fn patch_opencode_entry(config: &str) -> Result<Option<String>> {
    let mut doc: DocumentMut = config
        .parse()
        .context("failed to parse mise config as TOML")?;
    let Some(tools) = doc.get_mut("tools").and_then(Item::as_table_mut) else {
        return Ok(None);
    };
    let Some(entry) = tools.get_mut(OPENCODE_MISE_TOOL) else {
        return Ok(None);
    };
    if !ensure_allow_builds(entry) {
        return Ok(None);
    }
    Ok(Some(doc.to_string()))
}

fn ensure_allow_builds(entry: &mut Item) -> bool {
    match entry {
        Item::Value(Value::String(version)) => {
            let pinned = version.value().clone();
            let decor = version.decor().clone();
            let mut table = InlineTable::new();
            table.insert("version", Value::from(pinned));
            table.insert("allow_builds", allow_builds_value());
            let mut value = Value::InlineTable(table);
            *value.decor_mut() = decor;
            *entry = Item::Value(value);
            true
        }
        Item::Value(Value::InlineTable(table)) => add_allow_builds_inline(table),
        Item::Table(table) => add_allow_builds_table(table),
        _ => false,
    }
}

fn allow_builds_value() -> Value {
    let mut array = Array::new();
    array.push(OPENCODE_ALLOW_BUILDS.to_string());
    Value::Array(array)
}

fn add_allow_builds_inline(table: &mut InlineTable) -> bool {
    match table.get_mut("allow_builds") {
        Some(existing) => add_package_to_value(existing),
        None => {
            table.insert("allow_builds", allow_builds_value());
            true
        }
    }
}

fn add_allow_builds_table(table: &mut Table) -> bool {
    match table.get_mut("allow_builds") {
        Some(existing) => add_package_to_item(existing),
        None => {
            table.insert("allow_builds", Item::Value(allow_builds_value()));
            true
        }
    }
}

fn add_package_to_item(item: &mut Item) -> bool {
    item.as_value_mut()
        .map(add_package_to_value)
        .unwrap_or(false)
}

fn add_package_to_value(value: &mut Value) -> bool {
    value
        .as_array_mut()
        .map(add_package_to_array)
        .unwrap_or(false)
}

fn add_package_to_array(array: &mut Array) -> bool {
    if array.iter().any(|value| {
        value
            .as_str()
            .is_some_and(|name| name == OPENCODE_ALLOW_BUILDS || name == "all")
    }) {
        return false;
    }
    array.push(OPENCODE_ALLOW_BUILDS.to_string());
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patched(config: &str) -> Option<String> {
        patch_opencode_entry(config).unwrap()
    }

    #[test]
    fn patches_the_plain_entry_written_by_mise_use() {
        let config = "[tools]\n\"npm:@opencode-ai/cli\" = \"beta\"\nnode = \"latest\"\n";
        assert_eq!(
            patched(config),
            Some(
                "[tools]\n\"npm:@opencode-ai/cli\" = { version = \"beta\", allow_builds = [\"@opencode-ai/cli\"] }\nnode = \"latest\"\n"
                    .to_string()
            )
        );
    }

    #[test]
    fn patches_a_pinned_version_entry() {
        let config = "[tools]\n\"npm:@opencode-ai/cli\" = \"0.14.8\"\n";
        assert_eq!(
            patched(config),
            Some(
                "[tools]\n\"npm:@opencode-ai/cli\" = { version = \"0.14.8\", allow_builds = [\"@opencode-ai/cli\"] }\n"
                    .to_string()
            )
        );
    }

    #[test]
    fn leaves_entries_that_already_allow_builds_untouched() {
        let config = "[tools]\n\"npm:@opencode-ai/cli\" = { version = \"beta\", allow_builds = [\"@opencode-ai/cli\"] }\n";
        assert_eq!(patched(config), None);
    }

    #[test]
    fn adds_allow_builds_to_a_table_entry_without_it() {
        let config = "[tools]\n\"npm:@opencode-ai/cli\" = { version = \"beta\" }\n";
        let patched = patched(config).unwrap();
        assert!(patched.contains("version = \"beta\""));
        assert!(patched.contains("allow_builds = [\"@opencode-ai/cli\"]"));
    }

    #[test]
    fn adds_allow_builds_to_a_subtable_entry() {
        let config = "[tools.\"npm:@opencode-ai/cli\"]\nversion = \"beta\"\n";
        let patched = patched(config).unwrap();
        assert!(patched.contains("version = \"beta\""));
        assert!(patched.contains("allow_builds = [\"@opencode-ai/cli\"]"));
    }

    #[test]
    fn adds_the_package_to_an_existing_allow_builds_list() {
        let config = "[tools]\n\"npm:@opencode-ai/cli\" = { version = \"beta\", allow_builds = [\"other\"] }\n";
        let patched = patched(config).unwrap();
        assert!(patched.contains("allow_builds = [\"other\", \"@opencode-ai/cli\"]"));
    }

    #[test]
    fn honors_a_wildcard_allow_builds_entry() {
        let config = "[tools]\n\"npm:@opencode-ai/cli\" = { version = \"beta\", allow_builds = [\"all\"] }\n";
        assert_eq!(patched(config), None);
    }

    #[test]
    fn leaves_configs_without_the_tool_untouched() {
        let config = "[tools]\nnode = \"latest\"\n";
        assert_eq!(patched(config), None);
    }

    #[test]
    fn ignores_the_tool_name_outside_the_tools_table() {
        let config = "\"npm:@opencode-ai/cli\" = \"beta\"\n[alias]\nnode = \"latest\"\n";
        assert_eq!(patched(config), None);
    }

    #[test]
    fn preserves_comments_and_unrelated_content() {
        let config = "# pinned by hand\n[tools]\n\"npm:@opencode-ai/cli\" = \"beta\" # keep me\nnode = \"latest\"\n";
        let patched = patched(config).unwrap();
        assert!(patched.starts_with("# pinned by hand\n"));
        assert!(patched.contains("node = \"latest\"\n"));
        assert!(patched.contains("# keep me"));
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
            "[tools]\n\"npm:@opencode-ai/cli\" = \"beta\"\nnode = \"latest\"\n",
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
