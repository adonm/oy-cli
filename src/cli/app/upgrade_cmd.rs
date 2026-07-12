//! `oy upgrade` updates mise-managed oy/opencode installations together.

use anyhow::{Context, Result, bail};
use clap::Args;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

const OY_MISE_TOOL: &str = "cargo:oy-cli";
const OPENCODE_MISE_TOOL: &str = "npm:@opencode-ai/cli";
const UPGRADE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const SETUP_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const OUTPUT_LIMIT: usize = 1024 * 1024;

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
            "oy upgrade only manages the mise install path. Install both with mise (`mise use cargo-binstall cargo:oy-cli npm:@opencode-ai/cli`) or use your package manager's upgrade command."
        );
    }

    let command_args = mise_upgrade_args(args.check, args.dry_run, tools);

    if args.dry_run {
        crate::ui::line(format_args!("{}", shell_command("mise", &command_args)));
        crate::ui::line(format_args!(
            "{}",
            shell_command("mise", &post_upgrade_doctor_args())
        ));
        crate::ui::line(format_args!(
            "{}",
            shell_command("mise", &post_upgrade_setup_args())
        ));
        return Ok(0);
    }

    if args.check {
        let status = Command::new("mise")
            .args(&command_args)
            .status()
            .context("failed to launch mise; install mise or upgrade oy/opencode manually")?;
        return Ok(status.code().unwrap_or(1));
    }

    let mut upgrade_command = Command::new("mise");
    upgrade_command.args(&command_args);
    let upgrade = crate::tools::external::run_bounded_process(
        &mut upgrade_command,
        "mise upgrade",
        UPGRADE_TIMEOUT,
        OUTPUT_LIMIT,
    )
    .context("failed to upgrade oy/OpenCode with mise")?;
    if !upgrade.status.success() {
        return Ok(report_command_failure("mise upgrade", &upgrade));
    }

    // Run the newly upgraded oy so its release-specific OpenCode pin is applied,
    // rather than the pin embedded in this still-running old process.
    let mut doctor_command = Command::new("mise");
    doctor_command.args(post_upgrade_doctor_args());
    let doctor = crate::tools::external::run_bounded_process(
        &mut doctor_command,
        "post-upgrade doctor",
        UPGRADE_TIMEOUT,
        OUTPUT_LIMIT,
    )
    .context("upgraded oy, but failed to apply the current OpenCode 2 beta")?;
    if !doctor.status.success() {
        return Ok(report_command_failure("post-upgrade doctor", &doctor));
    }

    // Apply the new version-matched plugin pin through the active mise shim, not
    // this still-running old process.
    let mut setup_command = Command::new("mise");
    setup_command
        .args(post_upgrade_setup_args())
        .env("OY_COLOR", "never");
    let setup = crate::tools::external::run_bounded_process(
        &mut setup_command,
        "post-upgrade setup",
        SETUP_TIMEOUT,
        OUTPUT_LIMIT,
    )
    .context(
        "upgraded with mise, but failed to refresh oy integration with `mise exec -- oy setup`",
    )?;
    if !setup.status.success() {
        return Ok(report_command_failure("post-upgrade setup", &setup));
    }
    if setup.truncated {
        bail!("upgraded, but post-upgrade setup output exceeded the safety limit");
    }
    let summary: SetupSummary = serde_json::from_slice(&setup.stdout).with_context(|| {
        format!(
            "upgraded and ran setup, but could not read its result: {}",
            String::from_utf8_lossy(&setup.stdout).trim()
        )
    })?;
    if summary.status != "installed" {
        bail!(
            "upgraded, but post-upgrade setup reported `{}`",
            summary.status
        );
    }
    crate::ui::success("upgraded oy and OpenCode");
    if let Some(backup) = summary.backup {
        crate::ui::line(format_args!(
            "Previous oy integration files were moved to {}.",
            backup.display()
        ));
    }
    Ok(0)
}

#[derive(Debug, Deserialize)]
struct SetupSummary {
    status: String,
    backup: Option<PathBuf>,
}

fn report_command_failure(label: &str, output: &crate::tools::external::ExternalOutput) -> i32 {
    crate::ui::err_line(format_args!(
        "{label} failed with exit code {}",
        output.status.code().unwrap_or(1)
    ));
    if !output.stdout.is_empty() {
        crate::ui::err(&String::from_utf8_lossy(&output.stdout));
    }
    if !output.stderr.is_empty() {
        crate::ui::err(&String::from_utf8_lossy(&output.stderr));
    }
    if output.truncated {
        crate::ui::err_line("command diagnostics were truncated");
    }
    output.status.code().unwrap_or(1)
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

fn post_upgrade_doctor_args() -> Vec<String> {
    vec![
        "exec".to_string(),
        "--".to_string(),
        "oy".to_string(),
        "doctor".to_string(),
        "--install-missing".to_string(),
    ]
}

fn post_upgrade_setup_args() -> Vec<String> {
    ["exec", "--", "oy", "--json", "setup"]
        .into_iter()
        .map(ToOwned::to_owned)
        .collect()
}

fn mise_managed_upgrade_tools() -> Result<Vec<String>> {
    let mut command = Command::new("mise");
    command.args(["list", "--json"]);
    let output = crate::tools::external::run_bounded_process(
        &mut command,
        "mise list --json",
        Duration::from_secs(30),
        OUTPUT_LIMIT,
    )
    .context("failed to inspect mise-managed tools")?;
    if !output.status.success() {
        bail!("failed to inspect mise-managed tools with `mise list --json`");
    }
    if output.truncated {
        bail!("`mise list --json` output exceeded the safety limit");
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
            vec!["cargo:oy-cli", "npm:@opencode-ai/cli"]
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
                    vec![
                        "cargo:oy-cli".to_string(),
                        "npm:@opencode-ai/cli".to_string()
                    ]
                )
            ),
            "mise upgrade --dry-run cargo:oy-cli 'npm:@opencode-ai/cli'"
        );
        assert_eq!(
            shell_command("mise", &post_upgrade_doctor_args()),
            "mise exec -- oy doctor --install-missing"
        );
        assert_eq!(
            shell_command("mise", &post_upgrade_setup_args()),
            "mise exec -- oy --json setup"
        );
    }

    #[test]
    fn parses_setup_backup_for_the_upgrade_summary() {
        let summary: SetupSummary =
            serde_json::from_str(r#"{"status":"installed","backup":"/tmp/oy-backup"}"#).unwrap();

        assert_eq!(summary.status, "installed");
        assert_eq!(summary.backup, Some(PathBuf::from("/tmp/oy-backup")));
    }

    #[test]
    fn check_uses_dry_run_code_without_setup_refresh() {
        assert_eq!(
            mise_upgrade_args(
                true,
                false,
                vec![
                    "cargo:oy-cli".to_string(),
                    "npm:@opencode-ai/cli".to_string()
                ]
            ),
            vec![
                "upgrade",
                "--dry-run-code",
                "cargo:oy-cli",
                "npm:@opencode-ai/cli"
            ]
        );
    }
}
