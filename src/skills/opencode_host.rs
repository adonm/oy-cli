//! OpenCode executable selection and compatibility detection.
//!
//! Only setup's location eviction uses the host: after `oy setup` changes
//! skills on disk, a running, supported OpenCode 2 service is asked to drop
//! its cached location.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;
use wait_timeout::ChildExt as _;

pub(crate) const OPENCODE_ENV: &str = "OY_OPENCODE";
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const VERSION_OUTPUT_LIMIT: u64 = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenCodeHost {
    executable: PathBuf,
    version: Option<String>,
    available: bool,
}

impl OpenCodeHost {
    pub(crate) fn selected_in(directory: &Path) -> Self {
        Self::probe(selected_executable(), Some(directory))
    }

    fn probe(executable: PathBuf, directory: Option<&Path>) -> Self {
        let (available, version) = probe_version(&executable, directory).unwrap_or((false, None));
        Self {
            executable,
            version,
            available,
        }
    }

    pub(crate) fn executable(&self) -> &Path {
        &self.executable
    }

    /// Supported hosts run a tagged OpenCode 2 stable release (`2.x.y`).
    /// Prerelease, unparseable, and unavailable hosts are unsupported: setup
    /// then skips the optional location refresh instead of guessing.
    pub(crate) fn supported(&self) -> bool {
        self.available && self.version.as_deref().and_then(version_major) == Some(2)
    }
}

fn selected_executable() -> PathBuf {
    std::env::var_os(OPENCODE_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("opencode2"))
}

fn probe_version(executable: &Path, directory: Option<&Path>) -> Option<(bool, Option<String>)> {
    let mut command = Command::new(executable);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let stderr = child.stderr.take()?;
    let (sender, receiver) = mpsc::channel();
    for (is_stdout, stream) in [
        (true, Box::new(stdout) as Box<dyn Read + Send>),
        (false, Box::new(stderr) as Box<dyn Read + Send>),
    ] {
        let sender = sender.clone();
        std::thread::spawn(move || {
            let _ = sender.send((is_stdout, read_first_line(stream)));
        });
    }
    drop(sender);

    let status = match child.wait_timeout(VERSION_PROBE_TIMEOUT) {
        Ok(Some(status)) => status,
        Ok(None) | Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Some((false, None));
        }
    };
    if !status.success() {
        return Some((false, None));
    }

    let mut stdout_version = None;
    let mut stderr_version = None;
    for _ in 0..2 {
        let Ok((is_stdout, version)) = receiver.recv_timeout(Duration::from_millis(500)) else {
            break;
        };
        if is_stdout {
            stdout_version = version;
        } else {
            stderr_version = version;
        }
    }
    let version = stdout_version.or(stderr_version);
    Some((true, version))
}

fn read_first_line(mut reader: impl Read) -> Option<String> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(VERSION_OUTPUT_LIMIT)
        .read_to_end(&mut bytes)
        .ok()?;
    String::from_utf8_lossy(&bytes)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToOwned::to_owned)
}

fn version_major(version: &str) -> Option<u64> {
    let token = version_token(version)?;
    // Fail closed on prereleases (for example `2.0.0-rc.1` or the retired
    // `0.0.0-beta-*`/`0.0.0-next-*` channels): only tagged `2.x.y` builds are
    // supported.
    if token.contains('-') {
        return None;
    }
    token.split('.').next()?.parse().ok()
}

fn version_token(version: &str) -> Option<&str> {
    version
        .split_whitespace()
        .map(|part| part.trim_start_matches('v'))
        .find(|part| {
            part.chars().next().is_some_and(|ch| ch.is_ascii_digit()) && part.contains('.')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(version: Option<&str>) -> OpenCodeHost {
        OpenCodeHost {
            executable: PathBuf::from("opencode2"),
            version: version.map(ToOwned::to_owned),
            available: true,
        }
    }

    #[test]
    fn supports_tagged_v2_builds() {
        assert!(host(Some("opencode v2.0.12")).supported());
        assert!(host(Some("opencode2 v2.0.0")).supported());
        assert!(host(Some("2.0.14")).supported());
    }

    #[test]
    fn rejects_prerelease_other_major_and_unavailable_hosts() {
        assert!(!host(Some("opencode v0.0.0-beta-19271")).supported());
        assert!(!host(Some("0.0.0-next-15363")).supported());
        assert!(!host(Some("opencode2 v2.0.0-rc.1")).supported());
        assert!(!host(Some("opencode version 1.17.18")).supported());
        assert!(!host(Some("3.0.0")).supported());
        assert!(!host(Some("custom version unknown")).supported());
        assert!(!host(None).supported());
        assert!(
            !OpenCodeHost {
                executable: PathBuf::from("opencode2"),
                version: Some("2.0.12".to_string()),
                available: false,
            }
            .supported()
        );
    }

    #[test]
    fn version_major_reads_the_version_token() {
        assert_eq!(version_major("opencode v2.0.12"), Some(2));
        assert_eq!(version_major("opencode version 1.17.18"), Some(1));
        assert_eq!(version_major("2.0.0"), Some(2));
        assert_eq!(version_major("2.0.0-rc.1"), None);
        assert_eq!(version_major("0.0.0-beta-19271"), None);
        assert_eq!(version_major("custom version unknown"), None);
    }
}
