use anyhow::{Context, Result, bail};
use dirs::config_dir;
use std::env;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

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
    reject_symlink_destination(path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed creating {}", parent.display()))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
        let mode = fs::metadata(path)
            .ok()
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0o600);
        let mut file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(mode)
            .open(path)
            .with_context(|| format!("failed writing {}", path.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("failed writing {}", path.display()))?;
        let mut perms = file.metadata()?.permissions();
        perms.set_mode(mode);
        file.set_permissions(perms)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::write(path, bytes).with_context(|| format!("failed writing {}", path.display()))
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
