//! Workspace path resolution and safe workspace writes.

use anyhow::{Context, Result, bail};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use super::atomic_write;

pub struct WorkspaceWrite<'a> {
    pub path: &'a Path,
    pub bytes: &'a [u8],
}

impl<'a> WorkspaceWrite<'a> {
    pub fn new(path: &'a Path, bytes: &'a [u8]) -> Self {
        Self { path, bytes }
    }
}

pub fn oy_root() -> Result<PathBuf> {
    let raw_root = env::var("OY_ROOT").unwrap_or_else(|_| ".".to_string());
    let path = expand_home(PathBuf::from(&raw_root))
        .unwrap_or_else(|_| PathBuf::from(raw_root))
        .canonicalize()
        .context("failed to resolve workspace root")?;
    if !path.is_dir() {
        bail!("Workspace root is not a directory: {}", path.display());
    }
    Ok(path)
}

pub fn write_workspace_file(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write::write_workspace_batch(&[WorkspaceWrite::new(path, bytes)])
}

pub fn resolve_workspace_output_path(root: &Path, requested: &Path) -> Result<PathBuf> {
    if requested.is_absolute()
        || requested
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
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

fn expand_home(path: PathBuf) -> Result<PathBuf> {
    let text = path.to_string_lossy();
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
    Ok(path)
}
