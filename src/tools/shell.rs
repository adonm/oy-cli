//! `bash` tool implementation and process trust boundary.
//!
//! Shell execution is policy-gated, timeout-bound, output-capped, and launched
//! with credential-like environment variables removed by default.

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;
use std::ffi::OsStr;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt as _;
use tokio::process::Command;
use tokio::time::timeout;

use crate::config;

use super::ToolContext;
use super::args::BashArgs;
use super::policy::require_mutation_approval;

const MAX_BASH_TIMEOUT_SECONDS: u64 = 600;
const MAX_BASH_OUTPUT_BYTES: usize = 200_000;

#[derive(Debug, Serialize)]
pub(super) struct BashOutput {
    pub command: String,
    pub returncode: i32,
    pub stdout: String,
    pub stderr: String,
    pub stdout_preview: String,
    pub stderr_preview: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub stdout_capped: bool,
    pub stderr_capped: bool,
}

pub(crate) async fn tool_bash(ctx: &ToolContext, args: BashArgs) -> Result<Value> {
    if args.command.len() > config::max_bash_cmd_bytes() {
        bail!("command too large ({} bytes)", args.command.len());
    }
    let timeout_seconds = args.timeout_seconds.clamp(1, MAX_BASH_TIMEOUT_SECONDS);
    let approval_preview = format!(
        "workspace: {}\ntimeout: {timeout_seconds}s\ncommand:\n{}",
        ctx.root.display(),
        args.command.trim()
    );
    require_mutation_approval(ctx, "bash", Some(&approval_preview))?;
    let mut command = Command::new("bash");
    remove_sensitive_child_env(&mut command);
    let mut child = command
        .arg("-c")
        .arg(&args.command)
        .current_dir(&ctx.root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child.stdout.take().context("failed to capture stdout")?;
    let stderr = child.stderr.take().context("failed to capture stderr")?;
    let stdout_task = tokio::spawn(read_child_output(stdout, MAX_BASH_OUTPUT_BYTES));
    let stderr_task = tokio::spawn(read_child_output(stderr, MAX_BASH_OUTPUT_BYTES));
    let status = match timeout(Duration::from_secs(timeout_seconds), child.wait()).await {
        Ok(status) => status?,
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            bail!("bash timed out after {timeout_seconds}s");
        }
    };
    let (stdout, stdout_truncated) = stdout_task.await??;
    let (stderr, stderr_truncated) = stderr_task.await??;
    let (stdout_preview, stdout_preview_truncated) = crate::ui::head_tail(&stdout, 12_000);
    let (stderr_preview, stderr_preview_truncated) = crate::ui::head_tail(&stderr, 8_000);
    Ok(serde_json::to_value(BashOutput {
        command: args.command,
        returncode: status.code().unwrap_or(-1),
        stdout,
        stderr,
        stdout_preview,
        stderr_preview,
        stdout_truncated: stdout_truncated || stdout_preview_truncated,
        stderr_truncated: stderr_truncated || stderr_preview_truncated,
        stdout_capped: stdout_truncated,
        stderr_capped: stderr_truncated,
    })?)
}

fn remove_sensitive_child_env(command: &mut Command) {
    for (name, _) in std::env::vars_os() {
        if is_sensitive_env_name(&name) {
            command.env_remove(name);
        }
    }
}

fn is_sensitive_env_name(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return true;
    };
    let name = name.to_ascii_uppercase();
    name.contains("API_KEY")
        || name.contains("ACCESS_KEY")
        || name.contains("PRIVATE_KEY")
        || name.contains("SECRET")
        || name.contains("TOKEN")
        || name.contains("PASSWORD")
        || name.contains("PASSWD")
        || name.contains("CREDENTIAL")
        || name.contains("AUTH")
}

async fn read_child_output<R>(mut reader: R, max_bytes: usize) -> Result<(String, bool)>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut out = Vec::new();
    let mut truncated = false;
    let mut buf = [0u8; 1024];
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        let remaining = max_bytes.saturating_sub(out.len());
        if n > remaining {
            out.extend_from_slice(&buf[..remaining]);
            truncated = true;
        } else if remaining > 0 {
            out.extend_from_slice(&buf[..n]);
        } else {
            truncated = true;
        }
    }
    Ok((String::from_utf8_lossy(&out).to_string(), truncated))
}
