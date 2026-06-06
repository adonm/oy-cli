use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::path::Path;
use std::process::Command;

use super::super::ToolContext;
use super::super::args::{ExcludeArg, SlocArgs};
use super::output::SlocOutput;
use super::paths::resolve_existing_paths;

pub(crate) fn tool_sloc(ctx: &ToolContext, args: SlocArgs) -> Result<Value> {
    let targets = resolve_existing_paths(ctx, &args.path)?;
    let exclude = args
        .exclude
        .as_ref()
        .map(ExcludeArg::patterns)
        .unwrap_or_default();
    let output = run_tokei(ctx.root(), &targets, &exclude)?;

    Ok(serde_json::to_value(SlocOutput {
        path: args.path,
        format: "tokei-json",
        output,
        exclude: (!exclude.is_empty()).then_some(exclude),
    })?)
}

pub(crate) fn has_tokei() -> bool {
    Command::new("tokei").arg("--version").output().is_ok()
}

fn run_tokei(root: &Path, targets: &[std::path::PathBuf], exclude: &[String]) -> Result<Value> {
    let mut command = Command::new("tokei");
    command
        .current_dir(root)
        .arg("--output")
        .arg("json")
        .arg("--sort")
        .arg("code");
    for pattern in exclude {
        command.arg("--exclude").arg(pattern);
    }
    for target in targets {
        command.arg(target);
    }

    let output = command.output().context("failed to run tokei")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            bail!("tokei failed with status {}", output.status);
        }
        bail!("tokei failed with status {}: {stderr}", output.status);
    }
    serde_json::from_slice(&output.stdout).context("tokei output was not valid JSON")
}
