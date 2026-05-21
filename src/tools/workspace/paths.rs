use anyhow::{Context, Result, anyhow, bail};
use std::path::{Component, Path, PathBuf};

use super::super::ToolContext;
use super::discovery::{build_exclude_set, fff_fuzzy_workspace_paths_with_limit};

pub(super) fn reject_out_of_workspace_path(
    root: &Path,
    path: &str,
    resolved: Option<&Path>,
) -> Result<()> {
    let raw = Path::new(path);
    if raw.is_absolute() {
        bail!("path outside workspace is not allowed: {path} (absolute path)");
    }
    if raw.components().any(|c| matches!(c, Component::ParentDir)) {
        bail!("path outside workspace is not allowed: {path} (parent-directory path)");
    }
    if let Some(resolved) = resolved.filter(|resolved| !within_root(root, resolved)) {
        bail!(
            "path outside workspace is not allowed: {path} -> {}",
            resolved.display()
        );
    }
    Ok(())
}

pub(super) fn resolve_existing_path(ctx: &ToolContext, path: &str) -> Result<PathBuf> {
    reject_out_of_workspace_path(&ctx.root, path, None)?;
    let joined = ctx.root.join(path);
    let resolved = joined
        .canonicalize()
        .with_context(|| format!("path does not exist: {path}"))?;
    reject_out_of_workspace_path(&ctx.root, path, Some(&resolved))?;
    Ok(resolved)
}

pub(super) fn resolve_read_path(ctx: &ToolContext, path: &str) -> Result<PathBuf> {
    reject_out_of_workspace_path(&ctx.root, path, None)?;
    match resolve_existing_path(ctx, path) {
        Ok(path) => Ok(path),
        Err(err) => Err(read_path_error_with_suggestions(ctx, path, err)),
    }
}

fn read_path_error_with_suggestions(
    ctx: &ToolContext,
    path: &str,
    err: anyhow::Error,
) -> anyhow::Error {
    let suggestions = read_path_suggestions(ctx, path).unwrap_or_default();
    if suggestions.is_empty() {
        anyhow!(
            "{err}; read requires an exact existing workspace file path; use list for fuzzy discovery"
        )
    } else {
        anyhow!(
            "{err}; did you mean {}? read requires an exact existing workspace file path; use one of the suggested paths in a follow-up read call",
            suggestions.join(", ")
        )
    }
}

fn read_path_suggestions(ctx: &ToolContext, path: &str) -> Result<Vec<String>> {
    let exclude = build_exclude_set(None)?;
    let (items, _) = fff_fuzzy_workspace_paths_with_limit(&ctx.root, path, &exclude, 3)?;
    Ok(items)
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

pub(super) fn rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"))
}

pub(super) fn display_path(root: &Path, path: &Path) -> String {
    let mut value = rel_path(root, path);
    if path.is_dir() && !value.ends_with('/') {
        value.push('/');
    }
    value
}

pub(super) fn safe_list_item(root: &Path, path: &Path) -> Option<String> {
    let resolved = path.canonicalize().ok()?;
    if !within_root(root, &resolved) {
        return None;
    }
    Some(display_path(root, path))
}

pub(super) fn within_root(root: &Path, path: &Path) -> bool {
    path == root || path.starts_with(root)
}
