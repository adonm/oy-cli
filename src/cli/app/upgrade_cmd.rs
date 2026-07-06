//! `oy upgrade` updates mise-managed oy/opencode installations together.

use anyhow::{Context, Result, bail};
use clap::Args;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::process::Command;

const OY_MISE_TOOL: &str = "cargo:oy-cli";
const OPENCODE_MISE_TOOL: &str = "opencode";

#[derive(Debug, Args, Clone)]
pub(super) struct UpgradeArgs {
    #[arg(
        long,
        conflicts_with = "check",
        default_value_t = false,
        help = "Print the mise upgrade command without running it"
    )]
    dry_run: bool,
    #[arg(
        long,
        conflicts_with = "dry_run",
        default_value_t = false,
        help = "Check whether mise-managed oy/opencode are outdated"
    )]
    check: bool,
}

pub(super) fn upgrade_command(args: UpgradeArgs) -> Result<i32> {
    let tools = mise_managed_upgrade_tools()?;
    if tools.is_empty() {
        bail!(
            "oy upgrade only manages the mise install path. Install both with mise (`mise use cargo-binstall cargo:oy-cli opencode`) or use your package manager's upgrade command."
        );
    }

    let command_args = mise_upgrade_args(args.check, args.dry_run, tools);

    if args.dry_run {
        crate::ui::line(format_args!("{}", shell_command("mise", &command_args)));
        return Ok(0);
    }

    let status = Command::new("mise")
        .args(&command_args)
        .status()
        .context("failed to launch mise; install mise or upgrade oy/opencode manually")?;
    if !status.success() {
        return Ok(status.code().unwrap_or(1));
    }
    if args.check {
        return Ok(0);
    }

    // Refresh generated integration files through the active mise shim, not this
    // still-running old process, so newly upgraded embedded agents are installed.
    let setup_status = Command::new("mise")
        .args(["exec", "--", "oy", "setup"])
        .status()
        .context(
            "upgraded with mise, but failed to refresh oy integration with `mise exec -- oy setup`",
        )?;
    Ok(setup_status.code().unwrap_or(1))
}

fn mise_upgrade_args(check: bool, dry_run: bool, tools: Vec<String>) -> Vec<String> {
    let mut args = vec!["upgrade".to_string()];
    if check {
        args.push("--dry-run-code".to_string());
    } else if dry_run {
        args.push("--dry-run".to_string());
    }
    args.extend(tools);
    args
}

fn mise_managed_upgrade_tools() -> Result<Vec<String>> {
    let output = Command::new("mise")
        .args(["list", "--json"])
        .output()
        .context("failed to inspect mise-managed tools")?;
    if !output.status.success() {
        bail!("failed to inspect mise-managed tools with `mise list --json`");
    }
    let listing: MiseListing = serde_json::from_slice(&output.stdout)
        .context("failed to parse `mise list --json` output")?;
    Ok(mise_managed_upgrade_tools_from_listing(&listing.0))
}

#[derive(Debug, Deserialize)]
struct MiseListing(BTreeMap<String, Vec<MiseToolVersion>>);

#[derive(Debug, Deserialize)]
struct MiseToolVersion {
    active: bool,
    installed: bool,
    source: Option<MiseSource>,
}

#[derive(Debug, Deserialize)]
struct MiseSource {
    #[serde(rename = "type")]
    source_type: String,
}

fn mise_managed_upgrade_tools_from_listing(
    listing: &BTreeMap<String, Vec<MiseToolVersion>>,
) -> Vec<String> {
    let oy = has_active_mise_toml_tool(listing, OY_MISE_TOOL);
    let opencode = has_active_mise_toml_tool(listing, OPENCODE_MISE_TOOL);
    if oy && opencode {
        vec![OY_MISE_TOOL.to_string(), OPENCODE_MISE_TOOL.to_string()]
    } else {
        Vec::new()
    }
}

fn has_active_mise_toml_tool(listing: &BTreeMap<String, Vec<MiseToolVersion>>, tool: &str) -> bool {
    listing.get(tool).is_some_and(|versions| {
        versions.iter().any(|version| {
            version.active
                && version.installed
                && version
                    .source
                    .as_ref()
                    .is_some_and(|source| source.source_type == "mise.toml")
        })
    })
}

fn shell_command(program: &str, args: &[String]) -> String {
    std::iter::once(program.to_string())
        .chain(args.iter().cloned())
        .map(|part| shell_quote(&part))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | ':' | '='))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(active: bool, installed: bool, source_type: Option<&str>) -> MiseToolVersion {
        MiseToolVersion {
            active,
            installed,
            source: source_type.map(|source_type| MiseSource {
                source_type: source_type.to_string(),
            }),
        }
    }

    #[test]
    fn upgrades_both_tools_only_when_active_from_mise_toml() {
        let mut listing = BTreeMap::new();
        listing.insert(
            OY_MISE_TOOL.to_string(),
            vec![tool(true, true, Some("mise.toml"))],
        );
        listing.insert(
            OPENCODE_MISE_TOOL.to_string(),
            vec![tool(true, true, Some("mise.toml"))],
        );

        assert_eq!(
            mise_managed_upgrade_tools_from_listing(&listing),
            vec!["cargo:oy-cli", "opencode"]
        );
    }

    #[test]
    fn refuses_partial_or_non_mise_installations() {
        let mut listing = BTreeMap::new();
        listing.insert(
            OY_MISE_TOOL.to_string(),
            vec![tool(true, true, Some("mise.toml"))],
        );
        assert!(mise_managed_upgrade_tools_from_listing(&listing).is_empty());

        listing.insert(OPENCODE_MISE_TOOL.to_string(), vec![tool(true, true, None)]);
        assert!(mise_managed_upgrade_tools_from_listing(&listing).is_empty());
    }

    #[test]
    fn dry_run_command_is_shell_quoted() {
        assert_eq!(
            shell_command(
                "mise",
                &mise_upgrade_args(
                    false,
                    true,
                    vec!["cargo:oy-cli".to_string(), "opencode".to_string()]
                )
            ),
            "mise upgrade --dry-run cargo:oy-cli opencode"
        );
    }

    #[test]
    fn check_uses_dry_run_code_without_setup_refresh() {
        assert_eq!(
            mise_upgrade_args(
                true,
                false,
                vec!["cargo:oy-cli".to_string(), "opencode".to_string()]
            ),
            vec!["upgrade", "--dry-run-code", "cargo:oy-cli", "opencode"]
        );
    }
}
