//! `oy doctor` checks the OpenCode integration.

use anyhow::{Result, bail};
use clap::Args;
use std::io::{IsTerminal as _, Write as _};
use std::path::Path;
use std::time::Duration;

use crate::config;

const TOKEI_MISE_TOOL: &str = "aqua:XAMPPRocky/tokei@12.1.2";
const CTAGS_MISE_TOOL: &str =
    "github:universal-ctags/ctags-nightly-build[matching=.release.tar.gz]";
const MISE_MINIMUM_RELEASE_AGE: &str = "0";
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
            && runtime.cursor_provider
            && runtime.cursor_bridge
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
                            "service={} openapi={} location={} agent={} commands={} skills={} models={} providers={} cursor_provider={} cursor_bridge={} plugins={}",
                            runtime.service_version,
                            runtime.openapi,
                            runtime.location,
                            runtime.agents,
                            runtime.commands,
                            runtime.skills,
                            runtime.models,
                            runtime.providers,
                            runtime.cursor_provider,
                            runtime.cursor_bridge,
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
            "Run `oy doctor --install-missing` to install the current OpenCode 2 beta, then `oy setup`."
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
        tools.push(crate::mise::OPENCODE_MISE_SPEC);
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
    [
        "use",
        "--global",
        "--yes",
        "--minimum-release-age",
        MISE_MINIMUM_RELEASE_AGE,
    ]
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
        if requested && mise_ok && crate::mise::ensure_opencode_allow_builds()? {
            // Nothing is missing, but a workspace mise config can still pin
            // the OpenCode tool without `allow_builds`. Patch it now so a
            // later `mise install` here has the same postinstall permission
            // the global config has and cannot leave a stub `opencode2`.
            crate::ui::success("patched mise configs to allow the OpenCode 2 postinstall");
        }
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
    run_mise_use(&mise, &tools)?;
    // Patch every mise config declaring the OpenCode tool, including
    // workspace configs that override the global entry without
    // `allow_builds`, before any install runs its postinstall.
    crate::mise::ensure_opencode_allow_builds()?;
    if !opencode_ok {
        run_opencode_install(&mise)?;
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

fn run_opencode_install(mise: &Path) -> Result<()> {
    let status = std::process::Command::new(mise)
        .args(["install", "-f", crate::mise::OPENCODE_MISE_SPEC])
        .status()?;
    if status.success() {
        return Ok(());
    }
    bail!(
        "OpenCode install failed with exit code {}",
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
            vec![crate::mise::OPENCODE_MISE_SPEC, TOKEI_MISE_TOOL]
        );
        assert_eq!(missing_mise_tools(true, true, false), vec![CTAGS_MISE_TOOL]);
        assert!(missing_mise_tools(true, true, true).is_empty());
    }

    #[test]
    fn mise_install_uses_global_use_to_activate_shims() {
        assert_eq!(
            mise_use_global_args(&[TOKEI_MISE_TOOL, CTAGS_MISE_TOOL]),
            vec![
                "use",
                "--global",
                "--yes",
                "--minimum-release-age",
                "0",
                TOKEI_MISE_TOOL,
                CTAGS_MISE_TOOL
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
            "Run `oy doctor --install-missing` to install the current OpenCode 2 beta, then `oy setup`."
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

    #[cfg(unix)]
    mod fake_mise {
        use super::*;
        use std::ffi::OsString;
        use std::os::unix::fs::PermissionsExt as _;
        use std::path::{Path, PathBuf};
        use std::sync::Mutex;

        static ENV_LOCK: Mutex<()> = Mutex::new(());

        struct PathGuard {
            previous: Option<OsString>,
        }

        impl PathGuard {
            fn prepend(bin: &Path) -> Self {
                let previous = std::env::var_os("PATH");
                let mut paths = vec![bin.as_os_str().to_owned()];
                if let Some(path) = &previous {
                    paths.extend(std::env::split_paths(path).map(OsString::from));
                }
                unsafe {
                    std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
                }
                Self { previous }
            }
        }

        impl Drop for PathGuard {
            fn drop(&mut self) {
                unsafe {
                    if let Some(previous) = &self.previous {
                        std::env::set_var("PATH", previous);
                    } else {
                        std::env::remove_var("PATH");
                    }
                }
            }
        }

        /// A `mise` shim that answers `config ls --json` with a workspace
        /// config pinning the OpenCode tool without `allow_builds`, and logs
        /// every invocation so tests can assert whether mise was called.
        struct FakeMise {
            _dir: tempfile::TempDir,
            config: PathBuf,
            log: PathBuf,
            _path_guard: PathGuard,
        }

        impl FakeMise {
            fn new() -> Self {
                let dir = tempfile::tempdir().unwrap();
                let bin = dir.path().join("bin");
                std::fs::create_dir(&bin).unwrap();
                let config = dir.path().join("workspace.mise.toml");
                std::fs::write(&config, "[tools]\n\"npm:@opencode-ai/cli\" = \"beta\"\n").unwrap();
                let log = dir.path().join("mise.log");
                let shim = bin.join("mise");
                std::fs::write(
                    &shim,
                    format!(
                        "#!/bin/sh\nprintf '%s' \"$*\" >> '{}'\nprintf '%s' '[{{\"path\":\"{}\",\"tools\":[\"npm:@opencode-ai/cli\"]}}]'\n",
                        log.display(),
                        config.display()
                    ),
                )
                .unwrap();
                std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
                let _path_guard = PathGuard::prepend(&bin);
                Self {
                    _dir: dir,
                    config,
                    log,
                    _path_guard,
                }
            }
        }

        #[test]
        fn explicit_install_patches_workspace_configs_even_when_nothing_is_missing() {
            let _lock = ENV_LOCK.lock().unwrap();
            let fake = FakeMise::new();
            maybe_install_missing_with_mise(true, true, true, true, true).unwrap();
            let patched = std::fs::read_to_string(&fake.config).unwrap();
            assert!(
                patched.contains("allow_builds = [\"@opencode-ai/cli\"]"),
                "workspace config should get the same allow_builds patch as the global config: {patched}"
            );
            assert!(fake.log.exists(), "patch path must consult mise config ls");
        }

        #[test]
        fn diagnostic_doctor_leaves_workspace_configs_untouched() {
            let _lock = ENV_LOCK.lock().unwrap();
            let fake = FakeMise::new();
            maybe_install_missing_with_mise(false, true, true, true, true).unwrap();
            assert!(
                !fake.log.exists(),
                "plain doctor must stay read-only and never invoke mise"
            );
            let config = std::fs::read_to_string(&fake.config).unwrap();
            assert_eq!(config, "[tools]\n\"npm:@opencode-ai/cli\" = \"beta\"\n");
        }
    }
}
