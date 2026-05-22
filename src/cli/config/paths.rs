//! Config and workspace paths: `~/.config/oy-rust/` layout,
//! private file helpers, workspace output resolution, and
//! symlink-destination rejection.

use anyhow::{Context, Result, bail};
use dirs::config_dir;
use std::env;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

pub struct WorkspaceWrite<'a> {
    pub path: &'a Path,
    pub bytes: &'a [u8],
}

impl<'a> WorkspaceWrite<'a> {
    pub fn new(path: &'a Path, bytes: &'a [u8]) -> Self {
        Self { path, bytes }
    }
}

pub(super) const DEFAULT_CONFIG_DIR_NAME: &str = "oy-rust";

pub fn config_root() -> PathBuf {
    if let Ok(raw) = env::var("OY_CONFIG") {
        return PathBuf::from(&raw)
            .expand_home()
            .unwrap_or_else(|_| PathBuf::from(raw));
    }
    config_dir()
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join(DEFAULT_CONFIG_DIR_NAME)
        .join("config.json")
}

pub fn oy_root() -> Result<PathBuf> {
    let raw_root = env::var("OY_ROOT").unwrap_or_else(|_| ".".to_string());
    let path = PathBuf::from(&raw_root)
        .expand_home()
        .unwrap_or_else(|_| PathBuf::from(raw_root))
        .canonicalize()
        .context("failed to resolve workspace root")?;
    if !path.is_dir() {
        bail!("Workspace root is not a directory: {}", path.display());
    }
    Ok(path)
}

pub fn config_dir_path() -> PathBuf {
    config_root()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(format!(".config/{DEFAULT_CONFIG_DIR_NAME}")))
}

pub fn sessions_dir() -> Result<PathBuf> {
    let dir = config_dir_path().join("sessions");
    create_private_dir_all(&dir)?;
    Ok(dir)
}

pub fn write_workspace_file(path: &Path, bytes: &[u8]) -> Result<()> {
    write_workspace_batch(&[WorkspaceWrite::new(path, bytes)])
}

pub fn write_workspace_batch(writes: &[WorkspaceWrite<'_>]) -> Result<()> {
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

pub fn resolve_workspace_output_path(root: &Path, requested: &Path) -> Result<PathBuf> {
    if requested.is_absolute()
        || requested.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::Prefix(_)
            )
        })
    {
        bail!(
            "output path must stay inside workspace: {}",
            requested.display()
        );
    }
    let root = root
        .canonicalize()
        .context("failed to resolve workspace root")?;
    let path = root.join(requested);
    ensure_output_ancestors_safe(&root, &path, requested)?;
    reject_symlink_destination(&path)?;
    Ok(path)
}

fn ensure_output_ancestors_safe(root: &Path, path: &Path, requested: &Path) -> Result<()> {
    let mut current = root.to_path_buf();
    let relative_parent = path
        .parent()
        .unwrap_or(root)
        .strip_prefix(root)
        .context("output path must stay inside workspace")?;
    for component in relative_parent.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                bail!(
                    "output path escapes workspace through symlink ancestor: {}",
                    requested.display()
                )
            }
            Ok(_) => {
                let resolved = current
                    .canonicalize()
                    .with_context(|| format!("failed resolving {}", current.display()))?;
                if !resolved.starts_with(root) {
                    bail!("output path escapes workspace: {}", requested.display());
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => break,
            Err(err) => {
                return Err(err).with_context(|| format!("failed checking {}", current.display()));
            }
        }
    }
    Ok(())
}

pub fn reject_symlink_destination(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            bail!("refusing to write symlink: {}", path.display())
        }
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("failed checking {}", path.display())),
    }
}

pub fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
        if let Some(parent) = path.parent() {
            create_private_dir_all(parent)?;
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("failed writing {}", path.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("failed writing {}", path.display()))?;
        let mut perms = file.metadata()?.permissions();
        perms.set_mode(0o600);
        file.set_permissions(perms)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::write(path, bytes).with_context(|| format!("failed writing {}", path.display()))
    }
}

pub fn create_private_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("failed to create {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o700);
        fs::set_permissions(path, perms)?;
    }
    Ok(())
}

pub(super) trait ExpandHome {
    fn expand_home(self) -> Result<PathBuf>;
}

impl ExpandHome for PathBuf {
    fn expand_home(self) -> Result<PathBuf> {
        let text = self.to_string_lossy();
        if text == "~" || text.starts_with("~/") {
            let home = dirs::home_dir().context("home directory not found")?;
            let suffix = text
                .strip_prefix('~')
                .unwrap_or_default()
                .trim_start_matches('/');
            return Ok(if suffix.is_empty() {
                home
            } else {
                home.join(suffix)
            });
        }
        Ok(self)
    }
}
