use anyhow::{Context, Result, bail};
use std::path::{Component, Path, PathBuf};

use super::super::ToolContext;

fn candidate_path(root: &Path, path: &str) -> Result<PathBuf> {
    let raw = Path::new(path);
    if raw.components().any(|c| matches!(c, Component::ParentDir)) {
        bail!("path outside workspace is not allowed: {path}");
    }
    if !raw.is_absolute() && raw.components().any(|c| matches!(c, Component::Prefix(_))) {
        bail!("path outside workspace is not allowed: {path}");
    }

    Ok(if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        root.join(raw)
    })
}

pub(crate) fn resolve_existing_path(ctx: &ToolContext, path: &str) -> Result<PathBuf> {
    let root = ctx
        .root()
        .canonicalize()
        .context("failed to resolve workspace root")?;
    let resolved = candidate_path(&root, path)?
        .canonicalize()
        .with_context(|| format!("path does not exist: {path}"))?;
    if !resolved.starts_with(&root) {
        bail!(
            "path outside workspace is not allowed: {path} -> {}",
            resolved.display()
        );
    }
    Ok(resolved)
}

pub(super) fn resolve_read_path(ctx: &ToolContext, path: &str) -> Result<PathBuf> {
    resolve_existing_path(ctx, path)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn absolute_path_inside_workspace_is_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("src.rs");
        fs::write(&file, "fn main() {}\n").unwrap();
        let ctx = ToolContext::new(dir.path().canonicalize().unwrap());

        let resolved = resolve_existing_path(&ctx, file.to_str().unwrap()).unwrap();

        assert_eq!(resolved, file.canonicalize().unwrap());
    }

    #[test]
    fn absolute_path_outside_workspace_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let file = outside.path().join("src.rs");
        fs::write(&file, "fn main() {}\n").unwrap();
        let ctx = ToolContext::new(dir.path().canonicalize().unwrap());

        let err = resolve_existing_path(&ctx, file.to_str().unwrap()).unwrap_err();

        assert!(err.to_string().contains("path outside workspace"));
    }

    #[test]
    fn parent_traversal_is_rejected_before_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path().canonicalize().unwrap());

        let err = resolve_existing_path(&ctx, "../src.rs").unwrap_err();

        assert!(err.to_string().contains("path outside workspace"));
    }
}
