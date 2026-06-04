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
        let backup = backup_existing_workspace_file(&prepared_write.path)?;
        let path = prepared_write.path.clone();
        match prepared_write.commit() {
            Ok(()) => committed.push(CommittedWorkspaceWrite { path, backup }),
            Err(err) => {
                restore_workspace_backups(committed);
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

fn restore_workspace_backups(committed: Vec<CommittedWorkspaceWrite>) {
    for committed_write in committed.into_iter().rev() {
        if let Some(backup) = committed_write.backup {
            let _ = fs::rename(&backup, &committed_write.path);
        } else {
            let _ = fs::remove_file(&committed_write.path);
        }
    }
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
}
