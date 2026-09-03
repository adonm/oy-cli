//! Small adapter for the OpenCode 2 API operations oy setup needs.
//!
//! Only location eviction remains: after `oy setup` changes the skills on
//! disk, a running OpenCode service is asked to drop its cached location so
//! the next session discovers the new files.

use super::opencode_host::OpenCodeHost;
use anyhow::{Context as _, Result, bail};
use serde_json::Value;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::Duration;
use wait_timeout::ChildExt as _;

const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const ERROR_DETAIL_BYTES: usize = 8 * 1024;
const API_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub(super) struct OpenCodeApi<'a> {
    host: &'a OpenCodeHost,
}

impl<'a> OpenCodeApi<'a> {
    pub(super) fn new(host: &'a OpenCodeHost) -> Self {
        Self { host }
    }

    pub(super) fn evict_location(&self, directory: &Path) -> Result<()> {
        let directory_text = directory.to_str().ok_or_else(|| {
            anyhow::anyhow!(
                "workspace directory is not valid UTF-8: {}",
                directory.display()
            )
        })?;
        let location = format!("location[directory]={directory_text}");
        let output = self.invoke(
            &[
                "api",
                "v2.debug.location.evict",
                "--param",
                location.as_str(),
            ],
            directory,
        )?;
        if output.stdout.iter().all(u8::is_ascii_whitespace) {
            return Ok(());
        }
        let response: Value = serde_json::from_slice(&output.stdout)
            .context("OpenCode location eviction returned invalid JSON")?;
        reject_api_error(&response)
    }

    fn invoke(&self, args: &[&str], directory: &Path) -> Result<Output> {
        self.invoke_with_timeout(args, directory, API_TIMEOUT)
    }

    fn invoke_with_timeout(
        &self,
        args: &[&str],
        directory: &Path,
        timeout: Duration,
    ) -> Result<Output> {
        let mut stdout = tempfile::tempfile().context("failed to create OpenCode stdout buffer")?;
        let mut stderr = tempfile::tempfile().context("failed to create OpenCode stderr buffer")?;
        let mut child = Command::new(self.host.executable())
            .args(args)
            .current_dir(directory)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout.try_clone()?))
            .stderr(Stdio::from(stderr.try_clone()?))
            .spawn()
            .with_context(|| {
                format!("failed to invoke {} api", self.host.executable().display())
            })?;
        let status = match child.wait_timeout(timeout)? {
            Some(status) => status,
            None => {
                let _ = child.kill();
                let _ = child.wait();
                bail!(
                    "OpenCode API request timed out after {} seconds",
                    timeout.as_secs()
                );
            }
        };
        let stdout = read_bounded(&mut stdout, "stdout")?;
        let stderr = read_bounded(&mut stderr, "stderr")?;
        let output = Output {
            status,
            stdout,
            stderr,
        };
        if output.stdout.len() > MAX_RESPONSE_BYTES || output.stderr.len() > MAX_RESPONSE_BYTES {
            bail!("OpenCode API response exceeded the 16 MiB limit");
        }
        if !output.status.success() {
            let detail = text_detail(if output.stderr.is_empty() {
                &output.stdout
            } else {
                &output.stderr
            });
            bail!("OpenCode API exited with {}: {detail}", output.status);
        }
        Ok(output)
    }
}

fn read_bounded(file: &mut std::fs::File, stream: &str) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.take((MAX_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed reading OpenCode API {stream}"))?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        bail!("OpenCode API {stream} exceeded the 16 MiB limit");
    }
    Ok(bytes)
}

fn reject_api_error(response: &Value) -> Result<()> {
    let Some(tag) = response.get("_tag").and_then(Value::as_str) else {
        return Ok(());
    };
    let message = response
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("no error detail");
    bail!("OpenCode API failed ({tag}): {message}")
}

fn text_detail(bytes: &[u8]) -> String {
    let end = bytes.len().min(ERROR_DETAIL_BYTES);
    let mut detail = String::from_utf8_lossy(&bytes[..end]).trim().to_string();
    if bytes.len() > end {
        detail.push_str(" [truncated]");
    }
    detail
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_tagged_api_error() {
        assert!(
            reject_api_error(&json!({
                "_tag": "ServiceUnavailableError",
                "message": "catalog unavailable"
            }))
            .is_err()
        );
    }
}
