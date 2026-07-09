use anyhow::{Context, Result, anyhow};
use serde_json::Value;
use std::ffi::OsString;
use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use super::super::ToolContext;
use super::super::args::{ExcludeArg, SlocArgs};
use super::super::external::{ExternalCommand, discover};
use super::output::SlocOutput;
use super::paths::resolve_existing_paths;

const SLOC_TIMEOUT: Duration = Duration::from_secs(120);
const SLOC_OUTPUT_LIMIT: usize = 8 * 1024 * 1024;

static TOKEI: LazyLock<std::result::Result<ExternalCommand, String>> =
    LazyLock::new(|| discover_tokei().map_err(|err| format!("{err:#}")));

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
    tokei_command().is_ok()
}

fn run_tokei(root: &Path, targets: &[std::path::PathBuf], exclude: &[String]) -> Result<Value> {
    let command = tokei_command()?;
    let mut args = vec![
        OsString::from("--output"),
        OsString::from("json"),
        OsString::from("--sort"),
        OsString::from("code"),
    ];
    for pattern in exclude {
        args.push(OsString::from("--exclude"));
        args.push(OsString::from(pattern));
    }
    for target in targets {
        args.push(target.as_os_str().to_os_string());
    }

    let output = command.run(root, args, SLOC_TIMEOUT, SLOC_OUTPUT_LIMIT)?;
    output.require_success(command)?;
    serde_json::from_slice(&output.stdout).context("tokei output was not valid JSON")
}

fn tokei_command() -> Result<&'static ExternalCommand> {
    TOKEI.as_ref().map_err(|err| anyhow!(err.clone()))
}

fn discover_tokei() -> Result<ExternalCommand> {
    discover("tokei", "OY_TOKEI", &["tokei"], |command| {
        let output = command.probe(&["--version"])?;
        output.require_success(command)?;
        if !String::from_utf8_lossy(&output.stdout)
            .to_ascii_lowercase()
            .contains("tokei")
        {
            anyhow::bail!("version output does not identify tokei");
        }
        Ok(())
    })
}
