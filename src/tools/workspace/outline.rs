use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;
use std::ffi::OsString;
use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use super::super::ToolContext;
use super::super::args::OutlineArgs;
use super::super::external::{ExternalCommand, discover};
use super::output::OutlineOutput;
use super::paths::resolve_read_path;

const OUTLINE_TIMEOUT: Duration = Duration::from_secs(30);
const OUTLINE_OUTPUT_LIMIT: usize = 8 * 1024 * 1024;

static CTAGS: LazyLock<std::result::Result<ExternalCommand, String>> =
    LazyLock::new(|| discover_ctags().map_err(|err| format!("{err:#}")));

pub(crate) fn tool_outline(ctx: &ToolContext, args: OutlineArgs) -> Result<Value> {
    let path = resolve_read_path(ctx, &args.path)?;
    if path.is_dir() {
        bail!("outline path is a directory: {}", args.path);
    }

    let command = ctags_command()?;
    let output = run_ctags(ctx.root(), command, &path)?;

    Ok(serde_json::to_value(OutlineOutput {
        path: args.path,
        format: "universal-ctags-json",
        command: command.name().to_string(),
        output,
    })?)
}

pub(crate) fn has_universal_ctags() -> bool {
    ctags_command().is_ok()
}

fn ctags_command() -> Result<&'static ExternalCommand> {
    CTAGS.as_ref().map_err(|err| anyhow!(err.clone()))
}

fn discover_ctags() -> Result<ExternalCommand> {
    discover(
        "Universal Ctags with JSON support",
        "OY_CTAGS",
        &["u-ctags", "ctags"],
        |command| {
            let version = command.probe(&["--options=NONE", "--version"])?;
            version.require_success(command)?;
            if !String::from_utf8_lossy(&version.stdout).contains("Universal Ctags") {
                bail!("version output does not identify Universal Ctags");
            }

            let features = command.probe(&["--options=NONE", "--list-features"])?;
            features.require_success(command)?;
            if !supports_json_feature(&features.stdout) {
                bail!("Universal Ctags was built without JSON output support");
            }
            Ok(())
        },
    )
}

fn supports_json_feature(output: &[u8]) -> bool {
    String::from_utf8_lossy(output).lines().any(|line| {
        line.split_whitespace()
            .next()
            .is_some_and(|feature| feature.eq_ignore_ascii_case("json"))
    })
}

fn run_ctags(root: &Path, command: &ExternalCommand, path: &Path) -> Result<Value> {
    let args = [
        OsString::from("--options=NONE"),
        OsString::from("--output-format=json"),
        OsString::from("--fields=+nK"),
        OsString::from("--extras=-F"),
        OsString::from("-f"),
        OsString::from("-"),
        path.as_os_str().to_os_string(),
    ];
    let output = command.run(root, args, OUTLINE_TIMEOUT, OUTLINE_OUTPUT_LIMIT)?;
    output.require_success(command)?;
    let stdout = String::from_utf8(output.stdout).context("ctags output was not UTF-8")?;
    let mut records = Vec::new();
    for (index, line) in stdout.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        records.push(
            serde_json::from_str(line)
                .with_context(|| format!("ctags output line {} was not valid JSON", index + 1))?,
        );
    }
    Ok(Value::Array(records))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctags_json_feature_accepts_tabular_feature_output() {
        assert!(supports_json_feature(
            b"#NAME DESCRIPTION\njson supports json format output\n"
        ));
        assert!(!supports_json_feature(
            b"wildcards supports glob matching\n"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn ctags_call_disables_options_and_parses_json_lines() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("ctags");
        fs::write(
            &executable,
            "#!/bin/sh\n[ \"$1\" = \"--options=NONE\" ] || exit 19\nprintf '%s\\n' '{\"name\":\"main\",\"line\":1}'\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).unwrap();
        let source = dir.path().join("main.rs");
        fs::write(&source, "fn main() {}\n").unwrap();
        let command = super::super::super::external::test_command(&executable);

        let output = run_ctags(dir.path(), &command, &source).unwrap();

        assert_eq!(output[0]["name"], "main");
    }
}
