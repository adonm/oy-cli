//! Shared bounded subprocess execution.

use anyhow::{Context, Result, anyhow, bail};
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::process::{ExitStatus, Stdio};
use std::thread;
use std::time::Duration;
use wait_timeout::ChildExt as _;

pub(crate) fn resolve_executable(candidates: &[&str]) -> Option<std::path::PathBuf> {
    resolve_executable_in(candidates, std::env::var_os("PATH").as_deref())
}

fn resolve_executable_in(
    candidates: &[&str],
    search_path: Option<&OsStr>,
) -> Option<std::path::PathBuf> {
    let search_path = search_path?;
    for directory in std::env::split_paths(search_path) {
        if !directory.is_absolute() {
            continue;
        }
        for candidate in candidates {
            let path = directory.join(candidate);
            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };
            if metadata.is_file()
                && is_executable(&metadata)
                && let Ok(path) = path.canonicalize()
            {
                return Some(path);
            }
        }
    }
    None
}

fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    metadata.permissions().mode() & 0o111 != 0
}

pub(crate) fn run_bounded_process(
    process: &mut std::process::Command,
    name: &str,
    timeout: Duration,
    output_limit: usize,
) -> Result<ExternalOutput> {
    process
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    use std::os::unix::process::CommandExt as _;
    process.process_group(0);
    let mut child = process
        .spawn()
        .with_context(|| format!("failed to run {name}"))?;

    let stdout = child
        .stdout
        .take()
        .context("failed to capture process stdout")?;
    let stderr = child
        .stderr
        .take()
        .context("failed to capture process stderr")?;
    let stdout_reader = thread::spawn(move || read_bounded(stdout, output_limit));
    let stderr_reader = thread::spawn(move || read_bounded(stderr, output_limit));

    let status = match child
        .wait_timeout(timeout)
        .with_context(|| format!("failed waiting for {name}"))?
    {
        Some(status) => {
            terminate_process_group(child.id());
            status
        }
        None => {
            terminate(&mut child);
            let _ = child.wait();
            let _ = join_reader(stdout_reader, "stdout");
            let _ = join_reader(stderr_reader, "stderr");
            bail!("{name} timed out after {} seconds", timeout.as_secs());
        }
    };

    let (stdout, stdout_truncated) = join_reader(stdout_reader, "stdout")?;
    let (stderr, stderr_truncated) = join_reader(stderr_reader, "stderr")?;
    Ok(ExternalOutput {
        status,
        stdout,
        stderr,
        truncated: stdout_truncated || stderr_truncated,
    })
}

fn terminate(child: &mut std::process::Child) {
    terminate_process_group(child.id());
    let _ = child.kill();
}

fn terminate_process_group(process_id: u32) {
    let process_group = -(process_id as i32);
    // SAFETY: `kill` receives a process-group id created for this child and a fixed signal.
    let _ = unsafe { libc::kill(process_group, libc::SIGKILL) };
}

#[derive(Debug)]
pub(crate) struct ExternalOutput {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) truncated: bool,
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
        .map_err(|_| anyhow!("process {stream} reader panicked"))?
        .with_context(|| format!("failed reading process {stream}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::{Path, PathBuf};

    fn executable(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }

    #[test]
    fn executable_resolution_ignores_relative_path_entries() {
        let relative = std::env::join_paths([Path::new("."), Path::new("bin")]).unwrap();
        assert!(resolve_executable_in(&["helper"], Some(&relative)).is_none());
    }

    #[test]
    fn executable_resolution_returns_a_canonical_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let executable = executable(dir.path(), "helper", "exit 0");
        let search = std::env::join_paths([dir.path()]).unwrap();

        assert_eq!(
            resolve_executable_in(&["helper"], Some(&search)),
            Some(executable.canonicalize().unwrap())
        );
    }

    #[test]
    fn runner_enforces_output_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = executable(dir.path(), "helper", "printf '123456789'");
        let mut command = std::process::Command::new(path);

        let output =
            run_bounded_process(&mut command, "helper", Duration::from_secs(1), 4).unwrap();

        assert_eq!(output.stdout, b"1234");
        assert!(output.truncated);
    }

    #[test]
    fn runner_enforces_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let path = executable(dir.path(), "helper", "sleep 10 &\nwait");
        let mut command = std::process::Command::new(path);

        let err = run_bounded_process(&mut command, "helper", Duration::from_millis(100), 1024)
            .unwrap_err();

        assert!(err.to_string().contains("timed out"));
    }

    #[test]
    fn runner_closes_pipes_held_by_descendants_after_parent_exit() {
        let dir = tempfile::tempdir().unwrap();
        let path = executable(dir.path(), "helper", "sleep 10 &\nexit 0");
        let mut command = std::process::Command::new(path);
        let started = std::time::Instant::now();

        let output =
            run_bounded_process(&mut command, "helper", Duration::from_secs(1), 1024).unwrap();

        assert!(output.status.success());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
