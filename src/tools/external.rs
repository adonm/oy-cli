//! Shared resolution and bounded execution for optional external helpers.

use anyhow::{Context, Result, anyhow, bail};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::thread;
use std::time::Duration;
use wait_timeout::ChildExt as _;

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const PROBE_OUTPUT_LIMIT: usize = 256 * 1024;

#[derive(Debug, Clone)]
pub(super) struct ExternalCommand {
    name: String,
    path: PathBuf,
}

impl ExternalCommand {
    pub(super) fn name(&self) -> &str {
        &self.name
    }

    pub(super) fn run<I, S>(
        &self,
        root: &Path,
        args: I,
        timeout: Duration,
        output_limit: usize,
    ) -> Result<ExternalOutput>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut process = std::process::Command::new(&self.path);
        process
            .current_dir(root)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            process.process_group(0);
        }
        let mut child = process
            .spawn()
            .with_context(|| format!("failed to run {}", self.path.display()))?;

        let stdout = child
            .stdout
            .take()
            .context("failed to capture helper stdout")?;
        let stderr = child
            .stderr
            .take()
            .context("failed to capture helper stderr")?;
        let stdout_reader = thread::spawn(move || read_bounded(stdout, output_limit));
        let stderr_reader = thread::spawn(move || read_bounded(stderr, output_limit));

        let status = match child
            .wait_timeout(timeout)
            .with_context(|| format!("failed waiting for {}", self.path.display()))?
        {
            Some(status) => {
                // A successful direct child may still leave descendants holding inherited
                // stdout/stderr pipes. Close that escape hatch before joining reader threads.
                terminate_process_group(child.id());
                status
            }
            None => {
                terminate(&mut child);
                let _ = child.wait();
                let _ = join_reader(stdout_reader, "stdout");
                let _ = join_reader(stderr_reader, "stderr");
                bail!(
                    "{} timed out after {} seconds",
                    self.name,
                    timeout.as_secs()
                );
            }
        };

        let (stdout, stdout_truncated) = join_reader(stdout_reader, "stdout")?;
        let (stderr, stderr_truncated) = join_reader(stderr_reader, "stderr")?;
        if stdout_truncated || stderr_truncated {
            bail!(
                "{} output exceeded the {} byte per-stream limit",
                self.name,
                output_limit
            );
        }

        Ok(ExternalOutput {
            status,
            stdout,
            stderr,
        })
    }

    pub(super) fn probe(&self, args: &[&str]) -> Result<ExternalOutput> {
        self.run(
            &std::env::current_dir().context("failed to resolve current directory")?,
            args,
            PROBE_TIMEOUT,
            PROBE_OUTPUT_LIMIT,
        )
    }
}

fn terminate(child: &mut std::process::Child) {
    terminate_process_group(child.id());
    let _ = child.kill();
}

fn terminate_process_group(process_id: u32) {
    #[cfg(unix)]
    {
        // The child was launched as its own process group. Kill the group so descendants cannot
        // keep captured pipes open after the direct child is gone.
        let process_group = -(process_id as i32);
        // SAFETY: `kill` receives a process-group id created for this child and a fixed signal.
        let _ = unsafe { libc::kill(process_group, libc::SIGKILL) };
    }
    #[cfg(not(unix))]
    let _ = process_id;
}

#[derive(Debug)]
pub(super) struct ExternalOutput {
    pub(super) status: ExitStatus,
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: Vec<u8>,
}

impl ExternalOutput {
    pub(super) fn require_success(&self, command: &ExternalCommand) -> Result<()> {
        if self.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&self.stderr).trim().to_string();
        if stderr.is_empty() {
            bail!("{} failed with status {}", command.name(), self.status);
        }
        bail!(
            "{} failed with status {}: {stderr}",
            command.name(),
            self.status
        )
    }
}

pub(super) fn discover<F>(
    display_name: &str,
    override_env: &str,
    candidates: &[&str],
    probe: F,
) -> Result<ExternalCommand>
where
    F: Fn(&ExternalCommand) -> Result<()>,
{
    discover_in(
        display_name,
        override_env,
        std::env::var_os(override_env),
        std::env::var_os("PATH"),
        candidates,
        probe,
    )
}

fn discover_in<F>(
    display_name: &str,
    override_env: &str,
    override_path: Option<OsString>,
    search_path: Option<OsString>,
    candidates: &[&str],
    probe: F,
) -> Result<ExternalCommand>
where
    F: Fn(&ExternalCommand) -> Result<()>,
{
    if let Some(path) = override_path.filter(|value| !value.is_empty()) {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            bail!("{override_env} must be an absolute executable path");
        }
        let command = command_from_path(&path)?;
        probe(&command).with_context(|| {
            format!(
                "{display_name} configured by {override_env} is not usable: {}",
                path.display()
            )
        })?;
        return Ok(command);
    }

    let mut probe_errors = Vec::new();
    for candidate in candidates {
        for path in candidate_paths(candidate, search_path.as_deref()) {
            let Ok(command) = command_from_path(&path) else {
                continue;
            };
            match probe(&command) {
                Ok(()) => return Ok(command),
                Err(err) => probe_errors.push(format!("{}: {err}", path.display())),
            }
        }
    }

    if probe_errors.is_empty() {
        bail!("{display_name} was not found on PATH; set {override_env} to an absolute path");
    }
    bail!(
        "no usable {display_name} executable was found; set {override_env} to an absolute path ({})",
        probe_errors.join("; ")
    )
}

fn candidate_paths(candidate: &str, search_path: Option<&OsStr>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let Some(search_path) = search_path else {
        return paths;
    };
    for directory in std::env::split_paths(search_path) {
        // Do not let empty or relative PATH entries select an executable from an untrusted
        // workspace. Explicit overrides remain available for intentional local helpers.
        if !directory.is_absolute() {
            continue;
        }
        let base = directory.join(candidate);
        paths.push(base.clone());
        #[cfg(windows)]
        if base.extension().is_none() {
            let extensions = std::env::var_os("PATHEXT")
                .unwrap_or_else(|| OsString::from(".COM;.EXE;.BAT;.CMD"));
            for extension in extensions
                .to_string_lossy()
                .split(';')
                .filter(|item| !item.is_empty())
            {
                paths.push(directory.join(format!("{candidate}{extension}")));
            }
        }
    }
    paths
}

fn command_from_path(path: &Path) -> Result<ExternalCommand> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("external helper does not exist: {}", path.display()))?;
    if !metadata.is_file() {
        bail!("external helper is not a regular file: {}", path.display());
    }
    if !is_executable(&metadata) {
        bail!("external helper is not executable: {}", path.display());
    }
    let path = path
        .canonicalize()
        .with_context(|| format!("failed to resolve external helper: {}", path.display()))?;
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("external helper")
        .to_string();
    Ok(ExternalCommand { name, path })
}

#[cfg(test)]
pub(super) fn test_command(path: &Path) -> ExternalCommand {
    command_from_path(path).expect("test external command should be executable")
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &fs::Metadata) -> bool {
    true
}

fn read_bounded(mut reader: impl io::Read, limit: usize) -> io::Result<(Vec<u8>, bool)> {
    let mut output = Vec::with_capacity(limit.min(64 * 1024));
    let mut truncated = false;
    let mut chunk = [0_u8; 8192];
    loop {
        let count = reader.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        let remaining = limit.saturating_sub(output.len());
        let keep = count.min(remaining);
        output.extend_from_slice(&chunk[..keep]);
        truncated |= keep < count;
    }
    Ok((output, truncated))
}

fn join_reader(
    reader: thread::JoinHandle<io::Result<(Vec<u8>, bool)>>,
    stream: &str,
) -> Result<(Vec<u8>, bool)> {
    reader
        .join()
        .map_err(|_| anyhow!("external helper {stream} reader panicked"))?
        .with_context(|| format!("failed reading external helper {stream}"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    fn executable(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }

    #[test]
    fn discovery_returns_a_probed_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = executable(dir.path(), "helper", "printf 'helper 1.0\\n'");
        let search_path = std::env::join_paths([dir.path()]).unwrap();

        let command = discover_in(
            "Helper",
            "OY_HELPER",
            None,
            Some(search_path),
            &["helper"],
            |command| {
                let output = command.probe(&[])?;
                output.require_success(command)
            },
        )
        .unwrap();

        assert_eq!(command.path, path.canonicalize().unwrap());
    }

    #[test]
    fn override_must_be_absolute() {
        let err = discover_in(
            "Helper",
            "OY_HELPER",
            Some(OsString::from("helper")),
            None,
            &["helper"],
            |_| Ok(()),
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("must be an absolute executable path")
        );
    }

    #[test]
    fn discovery_ignores_relative_path_entries() {
        let relative_path = std::env::join_paths([Path::new("."), Path::new("bin")]).unwrap();

        let err = discover_in(
            "Helper",
            "OY_HELPER",
            None,
            Some(relative_path),
            &["helper"],
            |_| Ok(()),
        )
        .unwrap_err();

        assert!(err.to_string().contains("was not found on PATH"));
    }

    #[test]
    fn runner_enforces_output_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = executable(dir.path(), "helper", "printf '123456789'");
        let command = command_from_path(&path).unwrap();

        let err = command
            .run(
                dir.path(),
                std::iter::empty::<&str>(),
                Duration::from_secs(1),
                4,
            )
            .unwrap_err();

        assert!(err.to_string().contains("output exceeded"));
    }

    #[test]
    fn runner_enforces_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let path = executable(dir.path(), "helper", "sleep 10 &\nwait");
        let command = command_from_path(&path).unwrap();

        let err = command
            .run(
                dir.path(),
                std::iter::empty::<&str>(),
                Duration::from_millis(100),
                1024,
            )
            .unwrap_err();

        assert!(err.to_string().contains("timed out"));
    }

    #[test]
    fn runner_closes_pipes_held_by_descendants_after_parent_exit() {
        let dir = tempfile::tempdir().unwrap();
        let path = executable(dir.path(), "helper", "sleep 10 &\nexit 0");
        let command = command_from_path(&path).unwrap();
        let started = std::time::Instant::now();

        let output = command
            .run(
                dir.path(),
                std::iter::empty::<&str>(),
                Duration::from_secs(1),
                1024,
            )
            .unwrap();

        assert!(output.status.success());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
