//! `oy doctor` checks the integration and local deterministic helpers.

use anyhow::{Result, bail};
use clap::Args;
use std::io::{IsTerminal as _, Write as _};
use std::path::Path;

use crate::config;

const OPENCODE_MISE_TOOL: &str = "opencode";
const TOKEI_MISE_TOOL: &str = "cargo:tokei";
const CTAGS_MISE_TOOL: &str = "github:universal-ctags/ctags";
const SIGHTHOUND_MISE_TOOL: &str = "cargo:https://github.com/Corgea/Sighthound@tag:1.0";
const TOKEI_HINT: &str = "mise use --global cargo:tokei || brew install tokei";
const CTAGS_HINT: &str =
    "mise use --global github:universal-ctags/ctags || brew install universal-ctags";
const SIGHTHOUND_HINT: &str =
    "mise use --global cargo:https://github.com/Corgea/Sighthound@tag:1.0";

#[derive(Debug, Args, Clone)]
pub(super) struct DoctorArgs {
    #[arg(
        long,
        alias = "agent",
        default_value = "default",
        help = "Safety mode to inspect (default: balanced): plan, ask, edit, or auto"
    )]
    mode: config::SafetyMode,
    #[arg(
        long,
        default_value_t = false,
        help = "Install missing opencode/tokei/ctags/source-built Sighthound with global mise config"
    )]
    install_missing: bool,
}

pub(super) async fn doctor_command(args: DoctorArgs) -> Result<i32> {
    let root = config::oy_root()?;
    let mode = args.mode;
    let policy = config::tool_policy(mode);
    let opencode_ok = command_ok("opencode", &["--version"]);
    let mise_ok = command_ok("mise", &["--version"]);
    let oy_mcp_ok = command_ok("oy", &["mcp"]);
    let tokei_ok = crate::tools::has_external_sloc_counter();
    let ctags_ok = crate::tools::has_external_outline_tool();
    let sighthound_ok = crate::tools::has_external_security_scanner();
    let global_config = crate::opencode::global_config_path()?;
    let workspace_config = crate::opencode::workspace_config_path()?;
    let configured = global_config.exists() || workspace_config.exists();

    if crate::ui::is_json() {
        let payload = serde_json::json!({
            "workspace": root,
            "mode": mode.name(),
            "policy": policy,
            "opencode": opencode_ok,
            "mise": mise_ok,
            "oy_mcp_command": oy_mcp_ok,
            "optional_tools": {
                "tokei": {
                    "available": tokei_ok,
                    "enables": "sloc MCP tool",
                    "install": TOKEI_HINT,
                },
                "universal_ctags": {
                    "available": ctags_ok,
                    "enables": "outline MCP tool",
                    "install": CTAGS_HINT,
                },
                "sighthound": {
                    "available": sighthound_ok,
                    "enables": "sighthound MCP security scan tool",
                    "install": SIGHTHOUND_HINT,
                }
            },
            "global_opencode_config": global_config,
            "workspace_opencode_config": workspace_config,
            "configured": configured,
            "next_step": recommended_next_step(opencode_ok, configured, mise_ok, missing_mise_tools(opencode_ok, tokei_ok, ctags_ok, sighthound_ok).is_empty()),
        });
        crate::ui::line(serde_json::to_string_pretty(&payload)?);
        return Ok(0);
    }

    crate::ui::section("Doctor");
    crate::ui::kv("workspace", root.display());
    crate::ui::kv("mode", mode.name());
    crate::ui::kv("files-write", format_args!("{:?}", policy.files_write()));
    crate::ui::kv("shell", format_args!("{:?}", policy.shell));
    crate::ui::kv(
        "opencode",
        crate::ui::status_text(opencode_ok, if opencode_ok { "ok" } else { "missing" }),
    );
    crate::ui::kv(
        "mise",
        crate::ui::status_text(mise_ok, if mise_ok { "ok" } else { "missing" }),
    );
    crate::ui::kv(
        "optional tokei",
        crate::ui::status_text(
            tokei_ok,
            if tokei_ok {
                "ok; enables sloc MCP tool".to_string()
            } else {
                format!("missing; install with `{TOKEI_HINT}`")
            },
        ),
    );
    crate::ui::kv(
        "optional ctags",
        crate::ui::status_text(
            ctags_ok,
            if ctags_ok {
                "ok; enables outline MCP tool".to_string()
            } else {
                format!("missing; install Universal Ctags with `{CTAGS_HINT}`")
            },
        ),
    );
    crate::ui::kv(
        "optional Sighthound",
        crate::ui::status_text(
            sighthound_ok,
            if sighthound_ok {
                "ok; enables sighthound MCP security scan tool".to_string()
            } else {
                format!("missing; install from pinned source with `{SIGHTHOUND_HINT}`")
            },
        ),
    );
    crate::ui::kv(
        "global config",
        crate::ui::status_text(
            global_config.exists(),
            format_args!("{}", global_config.display()),
        ),
    );
    crate::ui::kv(
        "workspace config",
        crate::ui::status_text(
            workspace_config.exists(),
            format_args!("{}", workspace_config.display()),
        ),
    );
    crate::ui::line("");
    crate::ui::section("Recommended next step");
    crate::ui::line(format_args!(
        "  {}",
        recommended_next_step(
            opencode_ok,
            configured,
            mise_ok,
            missing_mise_tools(opencode_ok, tokei_ok, ctags_ok, sighthound_ok).is_empty(),
        )
    ));
    crate::ui::line("");
    crate::ui::section("Container hint");
    crate::ui::line(format_args!("  {}", safe_container_command(&root, true)));

    maybe_install_missing_with_mise(
        args.install_missing,
        mise_ok,
        opencode_ok,
        tokei_ok,
        ctags_ok,
        sighthound_ok,
    )?;
    Ok(0)
}

fn command_ok(command: &str, args: &[&str]) -> bool {
    std::process::Command::new(command)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn recommended_next_step(
    opencode_ok: bool,
    configured: bool,
    mise_ok: bool,
    no_missing_mise_tools: bool,
) -> &'static str {
    match (opencode_ok, configured, mise_ok, no_missing_mise_tools) {
        (false, _, true, _) => {
            "Run `oy doctor --install-missing` to install opencode with mise, then `oy setup`."
        }
        (false, _, false, _) => "Install opencode, then run `oy setup`.",
        (true, false, _, _) => "Run `oy setup`, then restart opencode.",
        (true, true, true, false) => {
            "Run `oy doctor --install-missing` to install optional mise helpers, or `oy` to launch now."
        }
        (true, true, _, _) => "Run `oy` to launch with the oy integration.",
    }
}

fn missing_mise_tools(
    opencode_ok: bool,
    tokei_ok: bool,
    ctags_ok: bool,
    sighthound_ok: bool,
) -> Vec<&'static str> {
    let mut tools = Vec::new();
    if !opencode_ok {
        tools.push(OPENCODE_MISE_TOOL);
    }
    if !tokei_ok {
        tools.push(TOKEI_MISE_TOOL);
    }
    if !ctags_ok {
        tools.push(CTAGS_MISE_TOOL);
    }
    if !sighthound_ok {
        tools.push(SIGHTHOUND_MISE_TOOL);
    }
    tools
}

fn mise_use_global_args(tools: &[&str]) -> Vec<String> {
    ["use", "--global"]
        .into_iter()
        .chain(tools.iter().copied())
        .map(ToOwned::to_owned)
        .collect()
}

fn maybe_install_missing_with_mise(
    requested: bool,
    mise_ok: bool,
    opencode_ok: bool,
    tokei_ok: bool,
    ctags_ok: bool,
    sighthound_ok: bool,
) -> Result<()> {
    let tools = missing_mise_tools(opencode_ok, tokei_ok, ctags_ok, sighthound_ok);
    if tools.is_empty() {
        return Ok(());
    }
    if !mise_ok {
        return Ok(());
    }
    if !requested && !should_prompt_install(&tools)? {
        return Ok(());
    }
    crate::ui::line(format_args!(
        "Installing and activating missing tools with mise: {}",
        tools.join(" ")
    ));
    let status = std::process::Command::new("mise")
        .args(mise_use_global_args(&tools))
        .status()?;
    if !status.success() {
        bail!(
            "mise use --global failed with exit code {}",
            status.code().unwrap_or(1)
        );
    }
    crate::ui::success("mise use --global completed");
    Ok(())
}

fn should_prompt_install(tools: &[&str]) -> Result<bool> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() || crate::ui::is_json() {
        return Ok(false);
    }
    crate::ui::line("");
    crate::ui::out(&format!(
        "Install and activate missing tools with mise now? [{}] [y/N] ",
        tools.join(" ")
    ));
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn safe_container_command(root: &Path, read_only: bool) -> String {
    let mode = if read_only { "ro" } else { "rw" };
    let mount = format!("{}:/workspace:{mode}", root.display());
    format!(
        "docker run --rm -it -v {} -w /workspace oy-image oy",
        shell_quote(&mount)
    )
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

    #[test]
    fn command_probe_closes_stdin() {
        assert!(!command_ok(
            "sh",
            &["-c", "if read _; then exit 0; else exit 17; fi"]
        ));
    }

    #[test]
    fn mise_tool_list_tracks_missing_tools() {
        assert_eq!(
            missing_mise_tools(false, false, true, true),
            vec!["opencode", "cargo:tokei"]
        );
        assert_eq!(
            missing_mise_tools(true, true, false, true),
            vec!["github:universal-ctags/ctags"]
        );
        assert_eq!(
            missing_mise_tools(true, true, true, false),
            vec!["cargo:https://github.com/Corgea/Sighthound@tag:1.0"]
        );
        assert!(missing_mise_tools(true, true, true, true).is_empty());
    }

    #[test]
    fn mise_install_uses_global_use_to_activate_shims() {
        assert_eq!(
            mise_use_global_args(&["cargo:tokei", "github:universal-ctags/ctags"]),
            vec![
                "use",
                "--global",
                "cargo:tokei",
                "github:universal-ctags/ctags"
            ]
        );
    }
}
