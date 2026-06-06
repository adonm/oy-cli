use anyhow::{Context, Result, bail};
use std::path::{Component, PathBuf};

use super::super::ToolContext;

fn reject_out_of_workspace_path(ctx: &ToolContext, path: &str) -> Result<()> {
    let raw = std::path::Path::new(path);
    if raw.is_absolute() {
        bail!("path outside workspace is not allowed: {path} (absolute path)");
    }
    if raw
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
    {
        bail!("path outside workspace is not allowed: {path}");
    }
    let resolved = ctx.root().join(path).canonicalize().ok();
    if let Some(resolved) = resolved.filter(|resolved| !resolved.starts_with(ctx.root())) {
        bail!(
            "path outside workspace is not allowed: {path} -> {}",
            resolved.display()
        );
    }
    Ok(())
}

pub(crate) fn resolve_existing_path(ctx: &ToolContext, path: &str) -> Result<PathBuf> {
    reject_out_of_workspace_path(ctx, path)?;
    ctx.root()
        .join(path)
        .canonicalize()
        .with_context(|| format!("path does not exist: {path}"))
}

pub(super) fn resolve_read_path(ctx: &ToolContext, path: &str) -> Result<PathBuf> {
    let resolved = resolve_existing_path(ctx, path)?;
    if !resolved.starts_with(ctx.root()) {
        bail!("path outside workspace is not allowed: {path}");
    }
    Ok(resolved)
}

pub(super) fn resolve_existing_paths(ctx: &ToolContext, path: &str) -> Result<Vec<PathBuf>> {
    match resolve_existing_path(ctx, path) {
        Ok(path) => Ok(vec![path]),
        Err(full_path_error) => {
            let parts = path.split_whitespace().collect::<Vec<_>>();
            if parts.len() <= 1 {
                return Err(full_path_error);
            }
            let mut out = Vec::new();
            for part in parts {
                out.push(resolve_existing_path(ctx, part)?);
            }
            out.sort();
            out.dedup();
            Ok(out)
        }
    }
}
