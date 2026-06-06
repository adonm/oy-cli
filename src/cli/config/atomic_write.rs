//! Atomic batch writes with backup-and-rollback for workspace files.
//!
//! Writes are staged through temporary files, validated upfront, then
//! committed one-by-one with automatic rollback on any failure. This
//! keeps the workspace consistent even when a batch is partially applied.

use anyhow::{Context, Result};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use super::paths::{WorkspaceWrite, reject_symlink_destination};

/// Write a batch of workspace files atomically with backup-and-rollback.
pub(super) fn write_workspace_batch(writes: &[WorkspaceWrite<'_>]) -> Result<()> {
    if writes.is_empty() {
        return Ok(());
    }
    for write in writes {
        prevalidate_workspace_write(write.path)?;
    }

    let mut prepared = Vec::with_capacity(writes.len());
    for write in writes {
        match prepare_workspace_write(write.path, write.bytes) {
            Ok(temp) => prepared.push(temp),
            Err(err) => {
                drop(prepared);
                return Err(err);
            }
        }
    }

    commit_workspace_writes(prepared)
}

fn commit_workspace_writes(prepared: Vec<PreparedWorkspaceWrite>) -> Result<()> {
    let mut committed = Vec::with_capacity(prepared.len());
    for prepared_write in prepared {
        let backup = match backup_existing_workspace_file(&prepared_write.path) {
            Ok(backup) => backup,
            Err(err) => {
                if let Err(rollback_err) = restore_workspace_backups(committed) {
                    return Err(err).context(format!("rollback failed: {rollback_err:#}"));
                }
                return Err(err);
            }
        };
        let path = prepared_write.path.clone();
        match prepared_write.commit() {
            Ok(()) => committed.push(CommittedWorkspaceWrite { path, backup }),
            Err(err) => {
                if let Err(rollback_err) = restore_workspace_backups(committed) {
                    return Err(err).context(format!("rollback failed: {rollback_err:#}"));
                }
                return Err(err);
            }
        }
    }
    for committed_write in committed {
        committed_write.cleanup_backup();
    }
    Ok(())
}

fn backup_existing_workspace_file(path: &Path) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let backup = tempfile::Builder::new()
        .prefix(".oy-backup-")
        .tempfile_in(parent)
        .with_context(|| format!("failed preparing backup for {}", path.display()))?
        .into_temp_path()
        .keep()
        .with_context(|| format!("failed preparing backup for {}", path.display()))?;
    fs::copy(path, &backup).with_context(|| format!("failed backing up {}", path.display()))?;
    Ok(Some(backup))
}

fn restore_workspace_backups(committed: Vec<CommittedWorkspaceWrite>) -> Result<()> {
    let mut rollback_error = None;
    for committed_write in committed.into_iter().rev() {
        if let Err(err) = committed_write.rollback() {
            rollback_error.get_or_insert(err);
        }
    }
    if let Some(err) = rollback_error {
        return Err(err);
    }
    Ok(())
}

fn prevalidate_workspace_write(path: &Path) -> Result<()> {
    reject_symlink_destination(path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed creating {}", parent.display()))?;
    }
    Ok(())
}

fn prepare_workspace_write(path: &Path, bytes: &[u8]) -> Result<PreparedWorkspaceWrite> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = fs::metadata(path)
            .ok()
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0o600);
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let mut file = tempfile::Builder::new()
            .prefix(".oy-write-")
            .tempfile_in(parent)
            .with_context(|| format!("failed preparing temporary file for {}", path.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("failed writing {}", path.display()))?;
        file.flush()
            .with_context(|| format!("failed flushing {}", path.display()))?;
        let mut perms = file.as_file().metadata()?.permissions();
        perms.set_mode(mode);
        file.as_file().set_permissions(perms)?;
        Ok(PreparedWorkspaceWrite {
            path: path.to_path_buf(),
            temp: file,
        })
    }
    #[cfg(not(unix))]
    {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let mut file = tempfile::Builder::new()
            .prefix(".oy-write-")
            .tempfile_in(parent)
            .with_context(|| format!("failed preparing temporary file for {}", path.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("failed writing {}", path.display()))?;
        file.flush()
            .with_context(|| format!("failed flushing {}", path.display()))?;
        Ok(PreparedWorkspaceWrite {
            path: path.to_path_buf(),
            temp: file,
        })
    }
}

struct PreparedWorkspaceWrite {
    path: PathBuf,
    temp: tempfile::NamedTempFile,
}

struct CommittedWorkspaceWrite {
    path: PathBuf,
    backup: Option<PathBuf>,
}

impl PreparedWorkspaceWrite {
    fn commit(self) -> Result<()> {
        self.temp
            .persist(&self.path)
            .map(|_| ())
            .map_err(|err| err.error)
            .with_context(|| format!("failed replacing {}", self.path.display()))
    }
}

impl CommittedWorkspaceWrite {
    fn cleanup_backup(self) {
        if let Some(backup) = self.backup {
            let _ = fs::remove_file(backup);
        }
    }

    fn rollback(self) -> Result<()> {
        if let Some(backup) = self.backup {
            replace_file(&backup, &self.path)
                .with_context(|| format!("failed restoring backup for {}", self.path.display()))
        } else {
            remove_file_if_exists(&self.path)
                .with_context(|| format!("failed removing {}", self.path.display()))
        }
    }
}

fn replace_file(source: &Path, destination: &Path) -> Result<()> {
    if destination.exists() {
        fs::remove_file(destination)
            .with_context(|| format!("failed removing {}", destination.display()))?;
    }
    fs::rename(source, destination).with_context(|| {
        format!(
            "failed moving {} to {}",
            source.display(),
            destination.display()
        )
    })
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("failed removing {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_file_overwrites_existing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let destination = dir.path().join("destination");
        fs::write(&source, "backup").unwrap();
        fs::write(&destination, "current").unwrap();

        replace_file(&source, &destination).unwrap();

        assert_eq!(fs::read_to_string(&destination).unwrap(), "backup");
        assert!(!source.exists());
    }

    #[test]
    fn batch_rolls_back_committed_writes_when_later_write_fails() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.md");
        let invalid_destination = dir.path().join("directory.md");
        fs::write(&first, "old").unwrap();
        fs::create_dir(&invalid_destination).unwrap();
        let writes = [
            WorkspaceWrite::new(&first, b"new"),
            WorkspaceWrite::new(&invalid_destination, b"bad"),
        ];

        let err = write_workspace_batch(&writes).unwrap_err();

        assert!(err.to_string().contains("failed backing up"));
        assert_eq!(fs::read_to_string(&first).unwrap(), "old");
    }

    #[test]
    fn restore_workspace_backups_reports_rollback_errors() {
        let dir = tempfile::tempdir().unwrap();
        let backup = dir.path().join("backup");
        let destination = dir.path().join("destination");
        fs::write(&backup, "backup").unwrap();
        fs::create_dir(&destination).unwrap();

        let err = restore_workspace_backups(vec![CommittedWorkspaceWrite {
            path: destination,
            backup: Some(backup.clone()),
        }])
        .unwrap_err();

        assert!(err.to_string().contains("failed restoring backup"));
        assert!(backup.exists());
    }
}
