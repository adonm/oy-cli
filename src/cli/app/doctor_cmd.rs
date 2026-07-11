//! `oy doctor` checks the integration and local deterministic helpers.

use anyhow::{Result, bail};
use clap::Args;
use std::io::{IsTerminal as _, Write as _};
use std::path::Path;

use crate::config;

const OPENCODE_MISE_TOOL: &str = "npm:@opencode-ai/cli@0.0.0-next-15323";
const OPENCODE_NODE_TOOL: &str = "node@24";
const TOKEI_MISE_TOOL: &str = "cargo:tokei";
const CTAGS_MISE_TOOL: &str = "github:universal-ctags/ctags";
const SIGHTHOUND_MISE_TOOL: &str = "cargo:https://github.com/Corgea/Sighthound[bin=sighthound,locked=true]@rev:c4608eb2b6ca256daf4dbd1e74aadc3570343685";
const SIGHTHOUND_RUST_TOOL: &str = "rust@1.96";
const TOKEI_HINT: &str = "mise use --global cargo:tokei || brew install tokei";
const CTAGS_HINT: &str =
    "mise use --global github:universal-ctags/ctags || brew install universal-ctags";
const SIGHTHOUND_HINT: &str = "oy doctor --install-sighthound";

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
        help = "Install missing OpenCode 2, tokei, and ctags with global mise config"
    )]
    install_missing: bool,
    #[arg(
        long,
        default_value_t = false,
        help = "Source-build pinned Sighthound with Rust 1.96 and global mise config"
    )]
    install_sighthound: bool,
    #[arg(
        long,
        conflicts_with_all = ["install_missing", "install_sighthound"],
        default_value_t = false,
        help = "Validate effective OpenCode agents, commands, MCP, and models; exit nonzero on failure"
    )]
    check: bool,
}

pub(super) async fn doctor_command(args: DoctorArgs) -> Result<i32> {
    if crate::ui::is_json() && (args.install_missing || args.install_sighthound) {
        bail!("--json cannot be combined with doctor install flags");
    }
    let root = config::oy_root()?;
    let mode = args.mode;
    let policy = config::tool_policy(mode);
    let opencode_host = crate::opencode::OpenCodeHost::selected_in(&root);
    let opencode_ok = opencode_host.available();
    let opencode_supported = opencode_host.supported();
    let mise_ok = command_ok("mise", &["--version"]);
    let oy_mcp_ok = command_ok("oy", &["mcp"]);
    let tokei_ok = crate::tools::has_external_sloc_counter();
    let ctags_ok = crate::tools::has_external_outline_tool();
    let sighthound_ok = crate::tools::has_external_security_scanner();
    let global_config = crate::opencode::global_config_path()?;
    let workspace_config = crate::opencode::workspace_config_path()?;
    let configured = global_config.exists() || workspace_config.exists();
    let custom_host = !opencode_host.is_default_executable();
    let opencode_mise_satisfied = opencode_supported || custom_host;
    let no_missing_mise_tools =
        missing_mise_tools(opencode_mise_satisfied, tokei_ok, ctags_ok, sighthound_ok).is_empty();
    let runtime = if args.check && opencode_supported && configured {
        crate::opencode::runtime_health(&opencode_host, &root).ok()
    } else {
        None
    };
    let runtime_ok = runtime.as_ref().is_some_and(|runtime| {
        runtime.healthy
            && runtime.service_version
            && runtime.openapi
            && runtime.location
            && runtime.agents
            && runtime.commands
            && runtime.skills
            && runtime.permissions
            && runtime.mcp_connected
            && runtime.models
            && runtime.providers
            && runtime.plugins
    });
    let check_ok = opencode_supported && configured && runtime_ok;

    if crate::ui::is_json() {
        let payload = serde_json::json!({
            "workspace": root,
            "mode": mode.name(),
            "policy": policy,
            "opencode": opencode_ok,
            "opencode_host": {
                "executable": opencode_host.executable_display(),
                "version": opencode_host.version(),
                "contract": opencode_host.contract().label(),
                "supported": opencode_supported,
                "run_workflows": opencode_supported,
                "model_api": opencode_supported,
            },
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
            "runtime": runtime,
            "check_ok": check_ok,
            "next_step": recommended_next_step(opencode_supported, configured, mise_ok, no_missing_mise_tools, custom_host),
        });
        crate::ui::line(serde_json::to_string_pretty(&payload)?);
        return Ok(if args.check && !check_ok { 1 } else { 0 });
    }

    crate::ui::section("Doctor");
    crate::ui::kv("workspace", root.display());
    crate::ui::kv("mode", mode.name());
    crate::ui::kv("files-write", format_args!("{:?}", policy.files_write()));
    crate::ui::kv("shell", format_args!("{:?}", policy.shell));
    crate::ui::kv(
        "opencode",
        crate::ui::status_text(
            opencode_supported,
            if opencode_supported {
                format!(
                    "ok; {} ({}, {})",
                    opencode_host.executable_display(),
                    opencode_host.version().unwrap_or("version unknown"),
                    opencode_host.contract().label()
                )
            } else if opencode_ok {
                format!(
                    "unsupported; {} ({})",
                    opencode_host.executable_display(),
                    opencode_host.version().unwrap_or("version unknown")
                )
            } else {
                format!("missing; {}", opencode_host.executable_display())
            },
        ),
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
    if args.check {
        crate::ui::kv(
            "runtime",
            crate::ui::status_text(
                runtime_ok,
                runtime
                    .as_ref()
                    .map(|runtime| {
                        format!(
                            "service={} openapi={} location={} agents={} commands={} skills={} permissions={} mcp={} models={} providers={} plugins={}",
                            runtime.service_version,
                            runtime.openapi,
                            runtime.location,
                            runtime.agents,
                            runtime.commands,
                            runtime.skills,
                            runtime.permissions,
                            runtime.mcp_connected,
                            runtime.models,
                            runtime.providers,
                            runtime.plugins
                        )
                    })
                    .unwrap_or_else(|| "unavailable".to_string()),
            ),
        );
    }
    crate::ui::line("");
    crate::ui::section("Recommended next step");
    crate::ui::line(format_args!(
        "  {}",
        recommended_next_step(
            opencode_supported,
            configured,
            mise_ok,
            no_missing_mise_tools,
            custom_host,
        )
    ));
    crate::ui::line("");
    crate::ui::section("Container hint");
    crate::ui::line(format_args!("  {}", safe_container_command(&root, true)));

    maybe_install_missing_with_mise(
        args.install_missing,
        mise_ok,
        opencode_mise_satisfied,
        tokei_ok,
        ctags_ok,
        sighthound_ok,
    )?;
    maybe_install_sighthound_with_mise(args.install_sighthound, mise_ok, sighthound_ok)?;
    Ok(if args.check && !check_ok { 1 } else { 0 })
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
    opencode_supported: bool,
    configured: bool,
    mise_ok: bool,
    no_missing_mise_tools: bool,
    custom_host: bool,
) -> &'static str {
    if !opencode_supported && custom_host {
        return "Fix or unset `OY_OPENCODE`, then rerun `oy doctor`.";
    }
    match (
        opencode_supported,
        configured,
        mise_ok,
        no_missing_mise_tools,
    ) {
        (false, _, true, _) => {
            "Run `oy doctor --install-missing` to install the pinned OpenCode 2 beta, then `oy setup`."
        }
        (false, _, false, _) => "Install OpenCode 2, then run `oy setup`.",
        (true, false, _, _) => "Run `oy setup`, then restart OpenCode 2.",
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
    _sighthound_ok: bool,
) -> Vec<&'static str> {
    let mut tools = Vec::new();
    if !opencode_ok {
        tools.push(OPENCODE_NODE_TOOL);
        tools.push(OPENCODE_MISE_TOOL);
    }
    if !tokei_ok {
        tools.push(TOKEI_MISE_TOOL);
    }
    if !ctags_ok {
        tools.push(CTAGS_MISE_TOOL);
    }
    tools
}

fn mise_use_global_args(tools: &[&str]) -> Vec<String> {
    ["use", "--global", "--yes"]
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
    for batch in mise_install_batches(&tools) {
        run_mise_use(&batch)?;
    }
    let status = std::process::Command::new("mise").arg("reshim").status()?;
    if !status.success() {
        bail!("tools installed, but `mise reshim` failed");
    }
    if !opencode_ok && !command_ok("mise", &["exec", "--", "opencode2", "--version"]) {
        bail!("OpenCode 2 installed, but `mise exec -- opencode2 --version` failed");
    }
    crate::ui::success("mise use --global completed");
    Ok(())
}

fn mise_install_batches<'a>(tools: &[&'a str]) -> Vec<Vec<&'a str>> {
    if !tools.contains(&OPENCODE_NODE_TOOL) {
        return vec![tools.to_vec()];
    }
    let remaining = tools
        .iter()
        .copied()
        .filter(|tool| *tool != OPENCODE_NODE_TOOL)
        .collect::<Vec<_>>();
    vec![vec![OPENCODE_NODE_TOOL], remaining]
}

fn run_mise_use(tools: &[&str]) -> Result<()> {
    if tools.is_empty() {
        return Ok(());
    }
    let status = std::process::Command::new("mise")
        .args(mise_use_global_args(tools))
        .status()?;
    if status.success() {
        return Ok(());
    }
    bail!(
        "mise use --global failed with exit code {}",
        status.code().unwrap_or(1)
    )
}

fn maybe_install_sighthound_with_mise(
    requested: bool,
    mise_ok: bool,
    _sighthound_ok: bool,
) -> Result<()> {
    if !requested {
        return Ok(());
    }
    if !mise_ok {
        bail!("--install-sighthound requires mise");
    }
    crate::ui::line(
        "Building pinned Sighthound from source with Rust 1.96; this may take several minutes.",
    );
    let status = std::process::Command::new("mise")
        .args(mise_use_global_args(&[SIGHTHOUND_RUST_TOOL]))
        .status()?;
    if !status.success() {
        bail!(
            "mise failed to install Rust for Sighthound with exit code {}",
            status.code().unwrap_or(1)
        );
    }
    let status = std::process::Command::new("mise")
        .args(mise_use_global_args(&[SIGHTHOUND_MISE_TOOL]))
        .status()?;
    if !status.success() {
        bail!(
            "mise failed to install Sighthound with exit code {}",
            status.code().unwrap_or(1)
        );
    }
    let status = std::process::Command::new("mise").arg("reshim").status()?;
    if !status.success() {
        bail!("Sighthound installed, but `mise reshim` failed");
    }
    if !command_ok("mise", &["exec", "--", "sighthound", "--version"]) {
        bail!("Sighthound installed, but `mise exec -- sighthound --version` failed");
    }
    crate::ui::success("pinned Sighthound source build installed");
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
            vec![OPENCODE_NODE_TOOL, OPENCODE_MISE_TOOL, "cargo:tokei"]
        );
        assert_eq!(
            missing_mise_tools(true, true, false, true),
            vec!["github:universal-ctags/ctags"]
        );
        assert_eq!(
            missing_mise_tools(true, true, true, false),
            Vec::<&str>::new()
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
                "--yes",
                "cargo:tokei",
                "github:universal-ctags/ctags"
            ]
        );
        assert_eq!(
            mise_install_batches(&[OPENCODE_NODE_TOOL, OPENCODE_MISE_TOOL, TOKEI_MISE_TOOL]),
            vec![
                vec![OPENCODE_NODE_TOOL],
                vec![OPENCODE_MISE_TOOL, TOKEI_MISE_TOOL]
            ]
        );
    }

    #[test]
    fn supported_v2_guidance_launches_oy_integration() {
        assert_eq!(
            recommended_next_step(false, false, true, true, false),
            "Run `oy doctor --install-missing` to install the pinned OpenCode 2 beta, then `oy setup`."
        );
        assert_eq!(
            recommended_next_step(true, true, true, true, false),
            "Run `oy` to launch with the oy integration."
        );
    }

    #[test]
    fn custom_host_guidance_does_not_offer_an_ineffective_mise_install() {
        assert_eq!(
            recommended_next_step(false, false, true, true, true),
            "Fix or unset `OY_OPENCODE`, then rerun `oy doctor`."
        );
    }

    #[test]
    fn sighthound_install_is_pinned_to_one_binary_and_commit() {
        assert!(SIGHTHOUND_MISE_TOOL.contains("bin=sighthound"));
        assert!(SIGHTHOUND_MISE_TOOL.contains("locked=true"));
        assert!(SIGHTHOUND_MISE_TOOL.contains("@rev:c4608eb2"));
    }
}
