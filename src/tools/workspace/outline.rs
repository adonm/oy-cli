use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::path::Path;
use std::process::Command;

use super::super::ToolContext;
use super::super::args::OutlineArgs;
use super::output::OutlineOutput;
use super::paths::resolve_read_path;

pub(crate) fn tool_outline(ctx: &ToolContext, args: OutlineArgs) -> Result<Value> {
    let path = resolve_read_path(ctx, &args.path)?;
    if path.is_dir() {
        bail!("outline path is a directory: {}", args.path);
    }

    let command = ctags_command().context("Universal Ctags is not installed on PATH")?;
    let output = run_ctags(ctx.root(), &command, &path)?;

    Ok(serde_json::to_value(OutlineOutput {
        path: args.path,
        format: "universal-ctags-json",
        command,
        output,
    })?)
}

pub(crate) fn has_universal_ctags() -> bool {
    ctags_command().is_some()
}

fn ctags_command() -> Option<String> {
    ["u-ctags", "ctags"]
        .into_iter()
        .find(|command| is_universal_ctags(command))
        .map(ToOwned::to_owned)
}

fn is_universal_ctags(command: &str) -> bool {
    let Ok(output) = Command::new(command).arg("--version").output() else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let version = String::from_utf8_lossy(&output.stdout);
    version.contains("Universal Ctags")
}

fn run_ctags(root: &Path, command: &str, path: &Path) -> Result<String> {
    let output = Command::new(command)
        .current_dir(root)
        .arg("--output-format=json")
        .arg("--fields=+nK")
        .arg("--extras=-F")
        .arg("-f")
        .arg("-")
        .arg(path)
        .output()
        .with_context(|| format!("failed to run {command}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            bail!("{command} failed with status {}", output.status);
        }
        bail!("{command} failed with status {}: {stderr}", output.status);
    }

    String::from_utf8(output.stdout).context("ctags output was not UTF-8")
}
