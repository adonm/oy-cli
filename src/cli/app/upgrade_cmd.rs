//! `oy upgrade` refreshes the mise-installed oy/OpenCode toolchain.

use anyhow::{Context, Result, bail};
use clap::Args;
use semver::Version;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

const OY_MISE_TOOL: &str = "github:adonm/oy-cli";
const OY_MISE_SPEC: &str = "github:adonm/oy-cli@latest";
const LEGACY_OY_MISE_TOOL: &str = "cargo:oy-cli";
const OPENCODE_DIST_TAGS_URL: &str =
    "https://registry.npmjs.org/-/package/@opencode-ai/cli/dist-tags";
const INSTALL_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const SETUP_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const VERSION_CHECK_TIMEOUT: Duration = Duration::from_secs(30);
const OUTPUT_LIMIT: usize = 1024 * 1024;

#[derive(Debug, Args, Clone)]
pub(super) struct UpgradeArgs {
    #[arg(
        long,
        conflicts_with = "check",
        default_value_t = false,
        help = "Print the mise upgrade commands without running them"
    )]
    dry_run: bool,
    #[arg(
        long,
        conflicts_with = "dry_run",
        default_value_t = false,
        help = "Check whether mise-managed oy/OpenCode are outdated"
    )]
    check: bool,
}

pub(super) fn upgrade_command(args: UpgradeArgs) -> Result<i32> {
    let Some(install) = mise_managed_install()? else {
        bail!(
            "oy upgrade only manages the mise install path. Install oy with `mise use --global github:adonm/oy-cli`, or use your package manager's upgrade command."
        );
    };

    if args.dry_run {
        print_dry_run(install.has_legacy_tools);
        return Ok(0);
    }

    if args.check {
        return check_for_upgrades();
    }

    run_checked(
        "mise use",
        &mise_use_args(false),
        INSTALL_TIMEOUT,
        "failed to install the latest oy with mise",
    )?;
    crate::mise::ensure_opencode_allow_builds()?;
    run_checked(
        "OpenCode 2 install",
        &opencode_install_args(),
        INSTALL_TIMEOUT,
        "failed to install the current OpenCode 2 beta with mise",
    )?;

    if install.has_legacy_tools {
        run_checked(
            "legacy tool migration",
            &legacy_unuse_args(),
            SETUP_TIMEOUT,
            "installed the new toolchain, but failed to remove superseded mise entries",
        )?;
    }

    run_checked(
        "mise reshim",
        &["reshim".to_string()],
        SETUP_TIMEOUT,
        "installed the new toolchain, but `mise reshim` failed",
    )?;

    let setup = run_command(
        "post-upgrade setup",
        &post_upgrade_setup_args(),
        SETUP_TIMEOUT,
    )
    .context(
        "upgraded with mise, but failed to refresh oy integration with the newly installed oy",
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

    run_checked(
        "OpenCode service restart",
        &service_restart_args(),
        SETUP_TIMEOUT,
        "upgraded oy/OpenCode, but failed to restart the OpenCode service",
    )?;

    crate::ui::success("upgraded oy and OpenCode");
    if let Some(backup) = summary.backup {
        crate::ui::line(format_args!(
            "Previous oy integration files were moved to {}.",
            backup.display()
        ));
    }
    Ok(0)
}

fn print_dry_run(has_legacy_tools: bool) {
    crate::ui::line(format_args!(
        "{}",
        shell_command("mise", &mise_use_args(false))
    ));
    crate::ui::line(format_args!(
        "{}",
        shell_command("mise", &opencode_install_args())
    ));
    if has_legacy_tools {
        crate::ui::line(format_args!(
            "{}",
            shell_command("mise", &legacy_unuse_args())
        ));
    }
    crate::ui::line("mise reshim");
    crate::ui::line(format_args!(
        "{}",
        shell_command("mise", &post_upgrade_setup_args())
    ));
    crate::ui::line(format_args!(
        "{}",
        shell_command("mise", &service_restart_args())
    ));
}

fn check_for_upgrades() -> Result<i32> {
    let mise_status = Command::new("mise")
        .args(mise_use_args(true))
        .status()
        .context("failed to launch mise; install mise or upgrade oy manually")?;
    let mise_outdated = match mise_status.code() {
        Some(0) => false,
        Some(1) => true,
        code => {
            crate::ui::err_line(format_args!(
                "`mise use --dry-run-code` failed with exit code {}",
                code.unwrap_or(1)
            ));
            return Ok(code.unwrap_or(1));
        }
    };

    let opencode_outdated = opencode_update_available()?;
    crate::ui::line(if opencode_outdated {
        "OpenCode 2 update available"
    } else {
        "OpenCode 2 is up to date"
    });
    Ok(i32::from(mise_outdated || opencode_outdated))
}

fn opencode_update_available() -> Result<bool> {
    let remote_version = opencode_dist_tag_version()?;
    let installed = run_command(
        "installed OpenCode version check",
        &[
            "exec".to_string(),
            "--".to_string(),
            "opencode2".to_string(),
            "--version".to_string(),
        ],
        SETUP_TIMEOUT,
    )
    .context("failed to inspect the installed OpenCode 2 version")?;
    if !installed.status.success() || installed.truncated {
        return Ok(true);
    }
    let Some(installed_version) = installed_opencode_version(&installed.stdout) else {
        return Ok(true);
    };
    Ok(installed_version < remote_version)
}

fn opencode_dist_tag_version() -> Result<Version> {
    parse_dist_tag_version(&fetch_opencode_dist_tags()?)
}

fn parse_dist_tag_version(body: &str) -> Result<Version> {
    let tags: serde_json::Value = serde_json::from_str(body)
        .context("failed to parse the OpenCode 2 npm dist-tags response")?;
    let Some(raw) = tags.get("beta").and_then(|tag| tag.as_str()) else {
        bail!("npm dist-tags response has no `beta` channel");
    };
    Version::parse(raw)
        .with_context(|| format!("npm returned an unparseable OpenCode 2 version: {raw:?}"))
}

fn fetch_opencode_dist_tags() -> Result<String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(VERSION_CHECK_TIMEOUT))
        .build()
        .into();
    let mut response = agent
        .get(OPENCODE_DIST_TAGS_URL)
        .call()
        .context("failed to query the current OpenCode 2 npm version")?;
    response
        .body_mut()
        .read_to_string()
        .context("failed to read the OpenCode 2 npm dist-tags response")
}

fn installed_opencode_version(stdout: &[u8]) -> Option<Version> {
    let text = String::from_utf8_lossy(stdout);
    text.split_whitespace()
        .find_map(|token| Version::parse(token.trim_start_matches('v')).ok())
}

fn run_checked(label: &str, args: &[String], timeout: Duration, context: &str) -> Result<()> {
    let output = run_command(label, args, timeout).with_context(|| context.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    report_command_failure(label, &output);
    bail!("{context}")
}

fn run_command(
    label: &str,
    args: &[String],
    timeout: Duration,
) -> Result<crate::tools::external::ExternalOutput> {
    let mut command = Command::new("mise");
    command.args(args);
    crate::tools::external::run_bounded_process(&mut command, label, timeout, OUTPUT_LIMIT)
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

fn mise_use_args(check: bool) -> Vec<String> {
    let mut args = vec![
        "use".to_string(),
        "--global".to_string(),
        "--yes".to_string(),
        "--minimum-release-age".to_string(),
        "0".to_string(),
    ];
    if check {
        args.push("--dry-run-code".to_string());
    }
    args.extend([
        OY_MISE_SPEC.to_string(),
        crate::mise::OPENCODE_MISE_SPEC.to_string(),
    ]);
    args
}

fn opencode_install_args() -> Vec<String> {
    ["install", "-f", crate::mise::OPENCODE_MISE_SPEC]
        .into_iter()
        .map(ToOwned::to_owned)
        .collect()
}

fn legacy_unuse_args() -> Vec<String> {
    ["unuse", "--global", "--yes", LEGACY_OY_MISE_TOOL]
        .into_iter()
        .map(ToOwned::to_owned)
        .collect()
}

fn post_upgrade_setup_args() -> Vec<String> {
    ["exec", OY_MISE_SPEC, "--", "oy", "--json", "setup"]
        .into_iter()
        .map(ToOwned::to_owned)
        .collect()
}

fn service_restart_args() -> Vec<String> {
    ["exec", "--", "opencode2", "service", "restart"]
        .into_iter()
        .map(ToOwned::to_owned)
        .collect()
}

fn mise_managed_install() -> Result<Option<ManagedInstall>> {
    let mut command = Command::new("mise");
    command.args(["list", "--global", "--json"]);
    let output = crate::tools::external::run_bounded_process(
        &mut command,
        "mise list --global --json",
        Duration::from_secs(30),
        OUTPUT_LIMIT,
    )
    .context("failed to inspect mise-managed tools")?;
    if !output.status.success() {
        bail!("failed to inspect mise-managed tools with `mise list --global --json`");
    }
    if output.truncated {
        bail!("`mise list --global --json` output exceeded the safety limit");
    }
    let listing: MiseListing = serde_json::from_slice(&output.stdout)
        .context("failed to parse `mise list --global --json` output")?;
    Ok(mise_managed_install_from_listing(&listing.0))
}

#[derive(Debug, PartialEq, Eq)]
struct ManagedInstall {
    has_legacy_tools: bool,
}

#[derive(Debug, Deserialize)]
struct MiseListing(BTreeMap<String, Vec<MiseToolVersion>>);

#[derive(Debug, Deserialize)]
struct MiseToolVersion {
    version: String,
    active: bool,
    installed: bool,
    source: Option<MiseSource>,
}

#[derive(Debug, Deserialize)]
struct MiseSource {
    #[serde(rename = "type")]
    source_type: String,
}

fn mise_managed_install_from_listing(
    listing: &BTreeMap<String, Vec<MiseToolVersion>>,
) -> Option<ManagedInstall> {
    let current = has_active_mise_toml_tool(listing, OY_MISE_TOOL);
    let legacy = has_active_mise_toml_tool(listing, LEGACY_OY_MISE_TOOL);
    if !current && !legacy {
        return None;
    }
    Some(ManagedInstall {
        has_legacy_tools: has_mise_toml_tool(listing, LEGACY_OY_MISE_TOOL),
    })
}

fn has_active_mise_toml_tool(listing: &BTreeMap<String, Vec<MiseToolVersion>>, tool: &str) -> bool {
    active_mise_toml_version(listing, tool).is_some()
}

fn has_mise_toml_tool(listing: &BTreeMap<String, Vec<MiseToolVersion>>, tool: &str) -> bool {
    listing.get(tool).is_some_and(|versions| {
        versions.iter().any(|version| {
            version
                .source
                .as_ref()
                .is_some_and(|source| source.source_type == "mise.toml")
        })
    })
}

fn active_mise_toml_version(
    listing: &BTreeMap<String, Vec<MiseToolVersion>>,
    tool: &str,
) -> Option<String> {
    listing.get(tool)?.iter().find_map(|version| {
        (version.active
            && version.installed
            && version
                .source
                .as_ref()
                .is_some_and(|source| source.source_type == "mise.toml"))
        .then(|| version.version.clone())
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
            version: "1.2.3".to_string(),
            active,
            installed,
            source: source_type.map(|source_type| MiseSource {
                source_type: source_type.to_string(),
            }),
        }
    }

    #[test]
    fn recognizes_current_binary_install() {
        let mut listing = BTreeMap::new();
        listing.insert(
            OY_MISE_TOOL.to_string(),
            vec![tool(true, true, Some("mise.toml"))],
        );

        assert_eq!(
            mise_managed_install_from_listing(&listing),
            Some(ManagedInstall {
                has_legacy_tools: false,
            })
        );
    }

    #[test]
    fn recognizes_legacy_install_for_migration() {
        let mut listing = BTreeMap::new();
        listing.insert(
            LEGACY_OY_MISE_TOOL.to_string(),
            vec![tool(true, true, Some("mise.toml"))],
        );

        assert_eq!(
            mise_managed_install_from_listing(&listing),
            Some(ManagedInstall {
                has_legacy_tools: true,
            })
        );
    }

    #[test]
    fn refuses_non_mise_installations() {
        let mut listing = BTreeMap::new();
        listing.insert(OY_MISE_TOOL.to_string(), vec![tool(true, true, None)]);
        assert_eq!(mise_managed_install_from_listing(&listing), None);
    }

    #[test]
    fn dry_run_commands_use_binary_oy_and_mise_opencode_install() {
        assert_eq!(
            shell_command("mise", &mise_use_args(false)),
            "mise use --global --yes --minimum-release-age 0 'github:adonm/oy-cli@latest' 'npm:@opencode-ai/cli@beta'"
        );
        assert_eq!(
            shell_command("mise", &opencode_install_args()),
            "mise install -f 'npm:@opencode-ai/cli@beta'"
        );
        assert_eq!(
            shell_command("mise", &post_upgrade_setup_args()),
            "mise exec 'github:adonm/oy-cli@latest' -- oy --json setup"
        );
        assert_eq!(
            shell_command("mise", &service_restart_args()),
            "mise exec -- opencode2 service restart"
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
    fn check_uses_non_mutating_mise_use() {
        assert_eq!(
            mise_use_args(true),
            vec![
                "use",
                "--global",
                "--yes",
                "--minimum-release-age",
                "0",
                "--dry-run-code",
                OY_MISE_SPEC,
                crate::mise::OPENCODE_MISE_SPEC,
            ]
        );
    }

    #[test]
    fn parses_the_opencode_version_from_version_output() {
        assert_eq!(
            installed_opencode_version(b"opencode2 v0.0.0-beta-17639\n"),
            Some(Version::parse("0.0.0-beta-17639").unwrap())
        );
        assert_eq!(
            installed_opencode_version(b"opencode2 v0.0.0-next-17114\n"),
            Some(Version::parse("0.0.0-next-17114").unwrap())
        );
        assert_eq!(
            installed_opencode_version(b"0.14.9\n"),
            Some(Version::parse("0.14.9").unwrap())
        );
        assert_eq!(installed_opencode_version(b"garbage\n"), None);
        assert_eq!(installed_opencode_version(b""), None);
    }

    #[test]
    fn orders_beta_channel_builds_semantically() {
        let older = Version::parse("0.0.0-beta-17639").unwrap();
        let newer = Version::parse("0.0.0-beta-18001").unwrap();
        assert!(older < newer);
    }

    #[test]
    fn parses_the_beta_dist_tag() {
        let version =
            parse_dist_tag_version(r#"{"latest":"1.18.18","beta":"0.0.0-beta-17639"}"#).unwrap();
        assert_eq!(version, Version::parse("0.0.0-beta-17639").unwrap());
    }

    #[test]
    fn rejects_a_missing_or_unparseable_beta_dist_tag() {
        let missing = parse_dist_tag_version(r#"{"latest":"1.18.18"}"#).unwrap_err();
        assert!(missing.to_string().contains("no `beta` channel"));

        let invalid = parse_dist_tag_version(r#"{"beta":"not-semver"}"#).unwrap_err();
        assert!(invalid.to_string().contains("unparseable"));
    }
}
