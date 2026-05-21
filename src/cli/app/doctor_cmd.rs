//! `oy doctor` subcommand: checks setup, auth, paths,
//! and safety-relevant defaults.

use anyhow::Result;
use clap::Args;
use std::path::Path;

use crate::config;
use crate::model;
use crate::tools::NetworkAccess;

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
    let listing = model::inspect_models().await?;
    let mode = args.mode;
    let policy = config::tool_policy(mode);
    let config_file = config::config_root();
    let config_dir = config::config_dir_path();
    let sessions_dir = config::sessions_dir().unwrap_or_else(|_| config_dir.join("sessions"));
    let history_dir = config_dir.join("history");
    let bash_ok = std::process::Command::new("bash")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);

    let opencode_ok = std::process::Command::new("opencode")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);

    if crate::ui::is_json() {
        let payload = serde_json::json!({
            "workspace": root,
            "model": listing.current,
            "recent_models": config::recent_models()?,
            "auth": listing.auth,
            "mode": mode.name(),
            "policy": policy,
            "interactive": config::can_prompt(),
            "non_interactive": config::non_interactive(),
            "config_file": config_file,
            "config_dir": config_dir,
            "sessions_dir": sessions_dir,
            "history_dir": history_dir,
            "bash": bash_ok,
            "opencode": opencode_ok,
            "next_step": recommended_next_step(&listing),
        });
        crate::ui::line(serde_json::to_string_pretty(&payload)?);
        return Ok(0);
    }

    crate::ui::section("Doctor");
    crate::ui::kv("workspace", root.display());
    crate::ui::kv("model", listing.current.as_deref().unwrap_or("<unset>"));
    if let Ok(recent) = config::recent_models() {
        crate::ui::kv("recent models", recent.len());
    }
    crate::ui::kv("mode", mode.name());
    crate::ui::kv("files-write", format_args!("{:?}", policy.files_write()));
    crate::ui::kv("shell", format_args!("{:?}", policy.shell));
    crate::ui::kv(
        "network",
        crate::ui::bool_text(policy.network == NetworkAccess::Enabled),
    );
    crate::ui::kv("risk", config::policy_risk_label(&policy));
    crate::ui::kv("interactive", crate::ui::bool_text(config::can_prompt()));
    crate::ui::kv(
        "bash",
        crate::ui::status_text(bash_ok, if bash_ok { "ok" } else { "missing" }),
    );
    crate::ui::kv(
        "opencode",
        crate::ui::status_text(opencode_ok, if opencode_ok { "ok" } else { "missing" }),
    );
    crate::ui::line("");
    crate::ui::section("Local state");
    crate::ui::kv("config", config_file.display());
    crate::ui::kv("sessions", sessions_dir.display());
    crate::ui::kv("history", history_dir.display());
    crate::ui::line(
        "  Treat local state as sensitive: prompts, source snippets, tool output, and command output may be saved.",
    );
    crate::ui::line("");
    crate::ui::section("Auth");
    if listing.auth.is_empty() {
        crate::ui::warn("no provider auth detected");
    } else {
        for item in &listing.auth {
            crate::ui::line(format_args!(
                "  {}  {} ({})",
                item.adapter,
                item.env_var.as_deref().unwrap_or("-"),
                item.source
            ));
            crate::ui::line(format_args!("    {}", item.detail));
        }
    }
    if listing.current.is_none() {
        crate::ui::line("");
        crate::ui::warn("no model configured");
        crate::ui::line(format_args!("  {}", recommended_next_step(&listing)));
    }
    crate::ui::line("");
    crate::ui::section("Recommended next steps");
    crate::ui::line(format_args!("  1. {}", recommended_next_step(&listing)));
    crate::ui::line("  2. For untrusted repos: `oy chat --mode plan`");
    crate::ui::line(format_args!(
        "  • Read-only container: {}",
        safe_container_command(&root, true)
    ));
    crate::ui::line("");
    crate::ui::section("Safety");
    crate::ui::line(
        "  oy is not a sandbox. Use `oy chat --mode plan` or a disposable container/VM for untrusted repos.",
    );
    crate::ui::line(
        "  Mount only needed credentials/env vars. Do not mount the host Docker socket into AI-assisted containers.",
    );
    Ok(0)
}

fn recommended_next_step(listing: &model::ModelListing) -> String {
    if listing.current.is_some() {
        return "Run `oy \"inspect this repo\"` or `oy chat`.".to_string();
    }
    if listing.all_models.is_empty() {
        return "Configure provider auth, then run `oy model` to inspect endpoint models."
            .to_string();
    }
    "Choose an introspected model with `oy model <name>`.".to_string()
}

fn safe_container_command(root: &Path, read_only: bool) -> String {
    let mode = if read_only { "ro" } else { "rw" };
    let mount = format!("{}:/workspace:{mode}", root.display());
    format!(
        "docker run --rm -it -v {} -w /workspace oy-image oy chat --mode plan",
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
