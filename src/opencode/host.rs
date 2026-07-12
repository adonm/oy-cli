//! OpenCode executable selection and compatibility detection.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;
use wait_timeout::ChildExt as _;

pub(crate) const OPENCODE_ENV: &str = "OY_OPENCODE";
pub(crate) const PINNED_BETA_BUILD: u64 = 15_353;
pub(crate) const PINNED_BETA_VERSION: &str = "0.0.0-next-15353";
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const VERSION_OUTPUT_LIMIT: u64 = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenCodeContract {
    V1,
    V2Beta,
    V2,
    Unknown,
}

impl OpenCodeContract {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::V1 => "v1",
            Self::V2Beta => "v2-beta",
            Self::V2 => "v2",
            Self::Unknown => "unknown (unprobed preview)",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenCodeHost {
    executable: PathBuf,
    version: Option<String>,
    available: bool,
    contract: OpenCodeContract,
}

impl OpenCodeHost {
    pub(crate) fn selected_in(directory: &Path) -> Self {
        Self::probe(selected_executable(), Some(directory))
    }

    fn probe(executable: PathBuf, directory: Option<&Path>) -> Self {
        let (available, version) = probe_version(&executable, directory).unwrap_or((false, None));
        let contract = detect_contract(&executable, version.as_deref());
        Self {
            executable,
            version,
            available,
            contract,
        }
    }

    pub(crate) fn executable(&self) -> &Path {
        &self.executable
    }

    pub(crate) fn executable_display(&self) -> String {
        self.executable.display().to_string()
    }

    pub(crate) fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    pub(crate) fn installation_version(&self) -> Option<&str> {
        self.version.as_deref().and_then(version_token)
    }

    pub(crate) fn available(&self) -> bool {
        self.available
    }

    pub(crate) fn contract(&self) -> OpenCodeContract {
        self.contract
    }

    pub(crate) fn supported(&self) -> bool {
        if !self.available {
            return false;
        }
        match self.contract {
            OpenCodeContract::V2 => true,
            OpenCodeContract::V2Beta => self.version.as_deref().is_some_and(is_pinned_beta),
            OpenCodeContract::V1 | OpenCodeContract::Unknown => false,
        }
    }

    pub(crate) fn require_supported(&self) -> Result<(), String> {
        if self.supported() {
            return Ok(());
        }
        if !self.available {
            return Err(format!(
                "OpenCode 2 host `{}` is unavailable; install @opencode-ai/cli@{PINNED_BETA_VERSION} or set {OPENCODE_ENV}",
                self.executable.display()
            ));
        }
        match self.contract {
            OpenCodeContract::V1 => Err(format!(
                "OpenCode 1 is no longer supported; install @opencode-ai/cli@{PINNED_BETA_VERSION}"
            )),
            OpenCodeContract::V2Beta => Err(format!(
                "OpenCode beta {} is unsupported; this oy release requires build {PINNED_BETA_BUILD}",
                self.version.as_deref().unwrap_or("unknown")
            )),
            OpenCodeContract::V2 | OpenCodeContract::Unknown => {
                Err("failed to identify a supported OpenCode 2 version".to_string())
            }
        }
    }

    pub(crate) fn is_default_executable(&self) -> bool {
        self.executable == Path::new("opencode2")
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

fn detect_contract(executable: &Path, version: Option<&str>) -> OpenCodeContract {
    if version.is_some_and(|value| value.contains("0.0.0-next-")) {
        return OpenCodeContract::V2Beta;
    }
    if let Some(major) = version.and_then(version_major) {
        return if major == 2 {
            OpenCodeContract::V2
        } else if major == 1 {
            OpenCodeContract::V1
        } else {
            OpenCodeContract::Unknown
        };
    }
    if version.is_none()
        && executable
            .file_stem()
            .is_some_and(|name| name == "opencode2")
    {
        return OpenCodeContract::V2Beta;
    }
    OpenCodeContract::Unknown
}

fn version_major(version: &str) -> Option<u64> {
    if version.contains('-') {
        return None;
    }
    version
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .find(|part| part.contains('.'))?
        .split('.')
        .next()?
        .parse()
        .ok()
}

fn beta_build(version: &str) -> Option<u64> {
    version
        .split("-next-")
        .nth(1)?
        .split(|ch: char| !ch.is_ascii_digit())
        .find(|part| !part.is_empty())?
        .parse()
        .ok()
}

fn is_pinned_beta(version: &str) -> bool {
    let token = version_token(version).unwrap_or(version);
    token == PINNED_BETA_VERSION && beta_build(token) == Some(PINNED_BETA_BUILD)
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

    #[test]
    fn detects_v2_beta_from_executable_or_next_version() {
        assert_eq!(
            detect_contract(Path::new("opencode2"), Some("opencode2 v0.0.0-next-15321")),
            OpenCodeContract::V2Beta
        );
        assert_eq!(
            detect_contract(Path::new("custom-host"), Some("opencode 0.0.0-next-42")),
            OpenCodeContract::V2Beta
        );
    }

    #[test]
    fn extracts_beta_build() {
        assert_eq!(beta_build("0.0.0-next-15353"), Some(15_353));
        assert_eq!(beta_build("opencode 0.0.0-next-15324"), Some(15_324));
        assert_eq!(beta_build("2.0.0"), None);
    }

    #[test]
    fn support_requires_current_beta_or_tagged_v2() {
        let host = |version: &str, contract| OpenCodeHost {
            executable: PathBuf::from("opencode2"),
            version: Some(version.to_string()),
            available: true,
            contract,
        };
        assert!(
            host(PINNED_BETA_VERSION, OpenCodeContract::V2Beta).supported(),
            "pinned beta must be supported"
        );
        assert!(!host("0.0.0-next-15322", OpenCodeContract::V2Beta).supported());
        assert!(!host("0.0.0-next-15324", OpenCodeContract::V2Beta).supported());
        assert!(!host("1.0.0-next-15353", OpenCodeContract::V2Beta).supported());
        assert!(host("2.0.0", OpenCodeContract::V2).supported());
        assert!(!host("3.0.0", OpenCodeContract::Unknown).supported());
        assert!(!host("1.17.18", OpenCodeContract::V1).supported());
    }

    #[test]
    fn detects_tagged_major_versions() {
        assert_eq!(
            detect_contract(Path::new("opencode"), Some("opencode version 1.17.18")),
            OpenCodeContract::V1
        );
        assert_eq!(
            detect_contract(Path::new("opencode2"), Some("opencode2 v2.0.0")),
            OpenCodeContract::V2
        );
        assert_eq!(
            detect_contract(Path::new("opencode2"), Some("opencode2 v1.17.18")),
            OpenCodeContract::V1
        );
        assert_eq!(
            detect_contract(Path::new("custom"), Some("custom version unknown")),
            OpenCodeContract::Unknown
        );
        assert_eq!(
            detect_contract(Path::new("opencode2"), Some("opencode2 v2.0.0-rc.1")),
            OpenCodeContract::Unknown
        );
    }
}
