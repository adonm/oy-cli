//! `oy doctor` checks the agent-skills installation and optional tooling.

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
        help = "Install missing tokei and Universal Ctags with global mise config"
    )]
    install_missing: bool,
    #[arg(
        long,
        conflicts_with = "install_missing",
        default_value_t = false,
        help = "Validate the oy skills installation; exit nonzero on failure"
    )]
    check: bool,
}

pub(super) fn doctor_command(args: DoctorArgs) -> Result<i32> {
    if crate::ui::is_json() && args.install_missing {
        bail!("--json cannot be combined with doctor install flags");
    }
    let root = config::oy_root()?;
    let global_skills = crate::skills::global_skills_dir()?;
    let workspace_skills = crate::skills::workspace_skills_dir()?;
    let global_complete = crate::skills::skills_complete(&global_skills);
    let workspace_complete = crate::skills::skills_complete(&workspace_skills);
    let skills_ok = global_complete || workspace_complete;
    let plugin_cache = crate::skills::plugin_cache_paths();
    let cache_clean = plugin_cache.is_empty();
    let mise_ok = command_ok("mise", &["--version"]);
    let tokei_ok = command_ok("tokei", &["--version"]);
    let ctags_ok = universal_ctags_ok();
    let no_missing_mise_tools = missing_mise_tools(tokei_ok, ctags_ok).is_empty();
    let check_ok = skills_ok && cache_clean;

    if crate::ui::is_json() {
        let payload = serde_json::json!({
            "workspace": root,
            "skills": {
                "global": {
                    "path": global_skills,
                    "complete": global_complete,
                },
                "workspace": {
                    "path": workspace_skills,
                    "complete": workspace_complete,
                },
            },
            "complete": skills_ok,
            "legacy_plugin_cache": plugin_cache,
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
            "check_ok": check_ok,
            "next_step": recommended_next_step(check_ok, skills_ok, cache_clean, mise_ok, no_missing_mise_tools),
        });
        crate::ui::line(serde_json::to_string_pretty(&payload)?);
        return Ok(if args.check && !check_ok { 1 } else { 0 });
    }

    crate::ui::section("Doctor");
    crate::ui::kv("workspace", root.display());
    crate::ui::kv(
        "global skills",
        crate::ui::status_text(
            global_complete,
            format_args!(
                "{}",
                if global_complete {
                    format!("ok; {}", global_skills.display())
                } else {
                    format!("incomplete; {}", global_skills.display())
                }
            ),
        ),
    );
    crate::ui::kv(
        "workspace skills",
        crate::ui::status_text(
            workspace_complete,
            format_args!(
                "{}",
                if workspace_complete {
                    format!("ok; {}", workspace_skills.display())
                } else {
                    format!("incomplete; {}", workspace_skills.display())
                }
            ),
        ),
    );
    crate::ui::kv(
        "legacy plugin cache",
        crate::ui::status_text(
            cache_clean,
            if cache_clean {
                "absent".to_string()
            } else {
                plugin_cache
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
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
    crate::ui::line("");
    crate::ui::section("Recommended next step");
    crate::ui::line(format_args!(
        "  {}",
        recommended_next_step(
            check_ok,
            skills_ok,
            cache_clean,
            mise_ok,
            no_missing_mise_tools
        )
    ));
    maybe_install_missing_with_mise(args.install_missing, mise_ok, tokei_ok, ctags_ok)?;
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
    check_ok: bool,
    skills_ok: bool,
    cache_clean: bool,
    mise_ok: bool,
    no_missing_mise_tools: bool,
) -> &'static str {
    if check_ok {
        return if mise_ok && !no_missing_mise_tools {
            "Run `oy doctor --install-missing` for optional context helpers."
        } else {
            "Ask your agent to audit or review with the oy skills."
        };
    }
    if !cache_clean {
        return "Run `oy setup` to remove the obsolete OpenCode plugin cache.";
    }
    if !skills_ok {
        return "Run `oy setup` (or `oy setup --workspace`), then ask your agent to run the oy-setup skill.";
    }
    "Run `oy doctor --check` for details."
}

fn missing_mise_tools(tokei_ok: bool, ctags_ok: bool) -> Vec<&'static str> {
    let mut tools = Vec::new();
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
    tokei_ok: bool,
    ctags_ok: bool,
) -> Result<()> {
    let tools = missing_mise_tools(tokei_ok, ctags_ok);
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
    run_mise_use(&mise, &tools)?;
    let status = std::process::Command::new(&mise).arg("reshim").status()?;
    if !status.success() {
        bail!("tools installed, but `mise reshim` failed");
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
        assert_eq!(missing_mise_tools(false, true), vec![TOKEI_MISE_TOOL]);
        assert_eq!(missing_mise_tools(true, false), vec![CTAGS_MISE_TOOL]);
        assert!(missing_mise_tools(true, true).is_empty());
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
        let error = maybe_install_missing_with_mise(true, false, false, false).unwrap_err();
        assert!(error.to_string().contains("requires mise"));
    }

    #[test]
    fn skills_guidance_depends_on_installation_state() {
        assert_eq!(
            recommended_next_step(false, false, true, true, true),
            "Run `oy setup` (or `oy setup --workspace`), then ask your agent to run the oy-setup skill."
        );
        assert_eq!(
            recommended_next_step(false, true, false, true, true),
            "Run `oy setup` to remove the obsolete OpenCode plugin cache."
        );
        assert_eq!(
            recommended_next_step(true, true, true, true, false),
            "Run `oy doctor --install-missing` for optional context helpers."
        );
        assert_eq!(
            recommended_next_step(true, true, true, true, true),
            "Ask your agent to audit or review with the oy skills."
        );
    }
}
