//! `oy doctor` checks the OpenCode integration and local deterministic helpers.

use anyhow::Result;
use clap::Args;
use std::path::Path;

use crate::config;

#[derive(Debug, Args, Clone)]
pub(super) struct DoctorArgs {
    #[arg(
        long,
        alias = "agent",
        default_value = "default",
        help = "Safety mode to inspect (default: balanced): plan, ask, edit, or auto"
    )]
    mode: config::SafetyMode,
}

pub(super) async fn doctor_command(args: DoctorArgs) -> Result<i32> {
    let root = config::oy_root()?;
    let mode = args.mode;
    let policy = config::tool_policy(mode);
    let opencode_ok = command_ok("opencode", &["--version"]);
    let oy_mcp_ok = command_ok("oy", &["mcp"]);
    let global_config = crate::opencode::global_config_path()?;
    let workspace_config = crate::opencode::workspace_config_path()?;
    let configured = global_config.exists() || workspace_config.exists();

    if crate::ui::is_json() {
        let payload = serde_json::json!({
            "workspace": root,
            "mode": mode.name(),
            "policy": policy,
            "opencode": opencode_ok,
            "oy_mcp_command": oy_mcp_ok,
            "global_opencode_config": global_config,
            "workspace_opencode_config": workspace_config,
            "configured": configured,
            "next_step": recommended_next_step(opencode_ok, configured),
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
        recommended_next_step(opencode_ok, configured)
    ));
    crate::ui::line("");
    crate::ui::section("Container hint");
    crate::ui::line(format_args!("  {}", safe_container_command(&root, true)));
    Ok(0)
}

fn command_ok(command: &str, args: &[&str]) -> bool {
    std::process::Command::new(command)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn recommended_next_step(opencode_ok: bool, configured: bool) -> &'static str {
    match (opencode_ok, configured) {
        (false, _) => "Install OpenCode, then run `oy setup`.",
        (true, false) => "Run `oy setup`, then restart OpenCode.",
        (true, true) => "Run `oy` to launch OpenCode with oy MCP tools.",
    }
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
