//! Configuration facade for workspace paths and atomic writes.

mod atomic_write;
mod paths;

pub(crate) use atomic_write::{FileMutation, apply_file_batch_in};
pub use paths::{oy_root, resolve_workspace_output_path, write_workspace_file};

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::Path};

    #[test]
    fn output_paths_stay_in_workspace() {
        let dir = tempfile::tempdir().unwrap();
        assert!(resolve_workspace_output_path(dir.path(), Path::new("notes/out.md")).is_ok());
        assert!(resolve_workspace_output_path(dir.path(), Path::new("../out.md")).is_err());
        assert!(resolve_workspace_output_path(dir.path(), Path::new("/tmp/out.md")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn output_paths_reject_symlink_ancestor_escapes() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), dir.path().join("reports")).unwrap();
        let err =
            resolve_workspace_output_path(dir.path(), Path::new("reports/new/out.md")).unwrap_err();
        assert!(err.to_string().contains("symlink ancestor"));
    }

    #[cfg(unix)]
    #[test]
    fn output_paths_reject_symlink_destinations() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.md");
        fs::write(&target, "safe").unwrap();
        symlink(&target, dir.path().join("link.md")).unwrap();
        let err = resolve_workspace_output_path(dir.path(), Path::new("link.md")).unwrap_err();
        assert!(err.to_string().contains("refusing to write symlink"));
    }
}
