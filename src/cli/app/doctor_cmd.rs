//! `oy doctor` checks the OpenCode integration.

use anyhow::{Result, bail};
use clap::Args;
use std::io::{IsTerminal as _, Write as _};
use std::path::Path;
use std::time::Duration;

use crate::config;

const OPENCODE_MISE_TOOL: &str = "npm:@opencode-ai/cli@0.0.0-next-15353";
const OPENCODE_NODE_TOOL: &str = "node@24";
const TOKEI_MISE_TOOL: &str = "cargo:tokei";
const CTAGS_MISE_TOOL: &str = "github:universal-ctags/ctags";
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const PROBE_OUTPUT_LIMIT: usize = 256 * 1024;

#[derive(Debug, Args, Clone)]
pub(super) struct DoctorArgs {
    #[arg(
        long,
        default_value_t = false,
        help = "Install missing OpenCode 2, tokei, and Universal Ctags with global mise config"
    )]
    install_missing: bool,
    #[arg(
        long,
        conflicts_with = "install_missing",
        default_value_t = false,
        help = "Validate the effective oy agent, commands, skills, and models; exit nonzero on failure"
    )]
    check: bool,
}

pub(super) fn doctor_command(args: DoctorArgs) -> Result<i32> {
    if crate::ui::is_json() && args.install_missing {
        bail!("--json cannot be combined with doctor install flags");
    }
    let root = config::oy_root()?;
    let opencode_host = crate::opencode::OpenCodeHost::selected_in(&root);
    let opencode_ok = opencode_host.available();
    let opencode_supported = opencode_host.supported();
    let mise_ok = command_ok("mise", &["--version"]);
    let tokei_ok = command_ok("tokei", &["--version"]);
    let ctags_ok = universal_ctags_ok();
    let global_config = crate::opencode::global_config_path()?;
    let workspace_config = crate::opencode::workspace_config_path()?;
    let configured = global_config.exists() || workspace_config.exists();
    let custom_host = !opencode_host.is_default_executable();
    let opencode_mise_satisfied = opencode_supported || custom_host;
    let no_missing_mise_tools =
        missing_mise_tools(opencode_mise_satisfied, tokei_ok, ctags_ok).is_empty();
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
            && runtime.models
            && runtime.providers
            && runtime.plugins
    });
    let check_ok = opencode_supported && configured && runtime_ok;

    if crate::ui::is_json() {
        let payload = serde_json::json!({
            "workspace": root,
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
            "optional_tools": {
                "tokei": {
                    "available": tokei_ok,
                    "purpose": "compact language and code-size inventory",
                },
                "universal_ctags": {
                    "available": ctags_ok,
                    "purpose": "scoped JSON symbol outlines",
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
                "ok; compact language/code-size inventory"
            } else {
                "missing"
            },
        ),
    );
    crate::ui::kv(
        "optional Universal Ctags",
        crate::ui::status_text(
            ctags_ok,
            if ctags_ok {
                "ok; scoped JSON symbol outlines"
            } else {
                "missing"
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
                            "service={} openapi={} location={} agent={} commands={} skills={} models={} providers={} plugins={}",
                            runtime.service_version,
                            runtime.openapi,
                            runtime.location,
                            runtime.agents,
                            runtime.commands,
                            runtime.skills,
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
    maybe_install_missing_with_mise(
        args.install_missing,
        mise_ok,
        opencode_mise_satisfied,
        tokei_ok,
        ctags_ok,
    )?;
    Ok(if args.check && !check_ok { 1 } else { 0 })
}

fn command_ok(command: &str, args: &[&str]) -> bool {
    command_output(command, args).is_some_and(|output| output.status.success())
}

fn command_output(command: &str, args: &[&str]) -> Option<crate::tools::external::ExternalOutput> {
    let executable = crate::tools::external::resolve_executable(&[command])?;
    let mut process = std::process::Command::new(executable);
    process.args(args);
    let output = crate::tools::external::run_bounded_process(
        &mut process,
        command,
        PROBE_TIMEOUT,
        PROBE_OUTPUT_LIMIT,
    )
    .ok()?;
    (!output.truncated).then_some(output)
}

fn universal_ctags_ok() -> bool {
    let Some(version) = command_output("ctags", &["--options=NONE", "--version"]) else {
        return false;
    };
    let Some(features) = command_output("ctags", &["--options=NONE", "--list-features"]) else {
        return false;
    };
    version.status.success()
        && String::from_utf8_lossy(&version.stdout).contains("Universal Ctags")
        && features.status.success()
        && ctags_supports_json(&features.stdout)
}

fn ctags_supports_json(output: &[u8]) -> bool {
    String::from_utf8_lossy(output).lines().any(|line| {
        line.split_whitespace()
            .next()
            .is_some_and(|feature| feature.eq_ignore_ascii_case("json"))
    })
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
            "Run `oy doctor --install-missing` for optional context helpers, or `oy` to launch now."
        }
        (true, true, _, _) => "Run `oy` to launch with the oy integration.",
    }
}

fn missing_mise_tools(opencode_ok: bool, tokei_ok: bool, ctags_ok: bool) -> Vec<&'static str> {
    let mut tools = Vec::new();
    if !opencode_ok {
        tools.extend([OPENCODE_NODE_TOOL, OPENCODE_MISE_TOOL]);
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
) -> Result<()> {
    let tools = missing_mise_tools(opencode_ok, tokei_ok, ctags_ok);
    if tools.is_empty() {
        return Ok(());
    }
    if !mise_ok {
        if requested {
            bail!("--install-missing requires mise");
        }
        return Ok(());
    }
    if !requested && !should_prompt_install(&tools)? {
        return Ok(());
    }
    let mise = crate::tools::external::resolve_executable(&["mise"])
        .ok_or_else(|| anyhow::anyhow!("mise executable disappeared from the absolute PATH"))?;
    crate::ui::line(format_args!(
        "Installing and activating missing tools with mise: {}",
        tools.join(" ")
    ));
    for batch in mise_install_batches(&tools) {
        run_mise_use(&mise, &batch)?;
    }
    let status = std::process::Command::new(&mise).arg("reshim").status()?;
    if !status.success() {
        bail!("tools installed, but `mise reshim` failed");
    }
    if !opencode_ok && !command_ok("mise", &["exec", "--", "opencode2", "--version"]) {
        bail!("OpenCode 2 installed, but `mise exec -- opencode2 --version` failed");
    }
    if !tokei_ok && !command_ok("mise", &["exec", "--", "tokei", "--version"]) {
        bail!("tokei installed, but `mise exec -- tokei --version` failed");
    }
    if !ctags_ok
        && !command_ok(
            "mise",
            &["exec", "--", "ctags", "--options=NONE", "--version"],
        )
    {
        bail!("Universal Ctags installed, but its version probe failed");
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

fn run_mise_use(mise: &Path, tools: &[&str]) -> Result<()> {
    if tools.is_empty() {
        return Ok(());
    }
    let status = std::process::Command::new(mise)
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
    fn ctags_json_feature_accepts_tabular_output() {
        assert!(ctags_supports_json(
            b"#NAME DESCRIPTION\njson supports json format output\n"
        ));
        assert!(!ctags_supports_json(b"wildcards supports glob matching\n"));
    }

    #[test]
    fn mise_tool_list_tracks_missing_tools() {
        assert_eq!(
            missing_mise_tools(false, false, true),
            vec![OPENCODE_NODE_TOOL, OPENCODE_MISE_TOOL, TOKEI_MISE_TOOL]
        );
        assert_eq!(missing_mise_tools(true, true, false), vec![CTAGS_MISE_TOOL]);
        assert!(missing_mise_tools(true, true, true).is_empty());
    }

    #[test]
    fn mise_install_uses_global_use_to_activate_shims() {
        assert_eq!(
            mise_use_global_args(&[TOKEI_MISE_TOOL, CTAGS_MISE_TOOL]),
            vec!["use", "--global", "--yes", TOKEI_MISE_TOOL, CTAGS_MISE_TOOL]
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
    fn explicit_install_requires_mise() {
        let error = maybe_install_missing_with_mise(true, false, true, false, false).unwrap_err();
        assert!(error.to_string().contains("requires mise"));
    }

    #[test]
    fn supported_v2_guidance_launches_oy_integration() {
        assert_eq!(
            recommended_next_step(false, false, true, false, false),
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
            recommended_next_step(false, false, true, false, true),
            "Fix or unset `OY_OPENCODE`, then rerun `oy doctor`."
        );
    }
}
