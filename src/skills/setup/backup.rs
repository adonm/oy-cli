//! Persistent setup backups and cross-filesystem move/restore helpers.

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(test)]
thread_local! {
    pub(super) static TEST_BACKUP_STATE_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn create_backup_dir() -> Result<PathBuf> {
    let state = backup_state_dir()?;
    fs::create_dir_all(&state)
        .with_context(|| format!("failed creating state directory {}", state.display()))?;
    let oy_state = state.join("oy");
    create_private_backup_directory(&oy_state)?;
    let base = oy_state.join("backups");
    create_private_backup_directory(&base)?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_nanos();
    for suffix in 0..100 {
        let path = base.join(format!(
            "opencode-{}-{timestamp}-{}-{suffix}",
            env!("CARGO_PKG_VERSION"),
            std::process::id()
        ));
        match fs::create_dir(&path) {
            Ok(()) => {
                restrict_backup_directory(&path)?;
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed creating backup {}", path.display()));
            }
        }
    }
    bail!(
        "failed to allocate a unique backup directory in {}",
        base.display()
    )
}

pub(super) fn backup_state_dir() -> Result<PathBuf> {
    #[cfg(test)]
    if let Some(path) = TEST_BACKUP_STATE_DIR.with(|state| state.borrow().clone()) {
        return Ok(path);
    }
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .context("failed to find a user state directory for oy backups")
}

fn create_private_backup_directory(path: &Path) -> Result<()> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        bail!(
            "refusing to use symlinked backup directory {}",
            path.display()
        );
    }
    if path.exists() {
        if !fs::metadata(path)?.is_dir() {
            bail!("backup path is not a directory: {}", path.display());
        }
    } else {
        fs::create_dir(path)
            .with_context(|| format!("failed creating backup directory {}", path.display()))?;
    }
    restrict_backup_directory(path)
}

pub(super) fn move_path(source: &Path, destination: &Path) -> Result<()> {
    match fs::rename(source, destination) {
        Ok(()) => return Ok(()),
        Err(error) if error.kind() != std::io::ErrorKind::CrossesDevices => {
            return Err(error).with_context(|| {
                format!(
                    "failed moving {} to {}",
                    source.display(),
                    destination.display()
                )
            });
        }
        Err(_) => {}
    }
    if let Err(error) = copy_path(source, destination) {
        let _ = remove_path(destination);
        return Err(error);
    }
    if let Err(error) = remove_path(source) {
        return Err(error).with_context(|| {
            format!(
                "failed removing {}; complete copy retained at {}",
                source.display(),
                destination.display()
            )
        });
    }
    Ok(())
}

pub(super) fn copy_path(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("failed reading {}", source.display()))?;
    if metadata.file_type().is_symlink() {
        use std::os::unix::fs::symlink;
        symlink(fs::read_link(source)?, destination).with_context(|| {
            format!(
                "failed copying symlink {} to {}",
                source.display(),
                destination.display()
            )
        })?;
        return Ok(());
    }
    if metadata.is_file() {
        fs::copy(source, destination).with_context(|| {
            format!(
                "failed copying {} to {}",
                source.display(),
                destination.display()
            )
        })?;
        fs::set_permissions(destination, metadata.permissions())?;
        return Ok(());
    }
    if metadata.is_dir() {
        fs::create_dir(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_path(&entry.path(), &destination.join(entry.file_name()))?;
        }
        fs::set_permissions(destination, metadata.permissions())?;
        return Ok(());
    }
    bail!("unsupported oy backup file type: {}", source.display())
}

fn remove_path(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn restrict_backup_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed securing backup directory {}", path.display()))?;
    Ok(())
}

pub(super) fn restore_moved_paths(moved: &[(PathBuf, PathBuf)]) -> Result<()> {
    let mut first_error = None;
    for (source, backup) in moved.iter().rev() {
        let result = (|| -> Result<()> {
            if let Some(parent) = source.parent() {
                fs::create_dir_all(parent)?;
            }
            move_path(backup, source).with_context(|| {
                format!(
                    "failed restoring {} from {}",
                    source.display(),
                    backup.display()
                )
            })
        })();
        if let Err(error) = result
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}
