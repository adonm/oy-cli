use anyhow::Result;
use std::path::PathBuf;

use crate::config;

pub(super) fn history_path(name: &str) -> Result<PathBuf> {
    history_path_in(config::config_dir_path(), name)
}

fn history_path_in(config_dir: PathBuf, name: &str) -> Result<PathBuf> {
    let history = config_dir.join("history");
    config::create_private_dir_all(&history)?;
    let path = history.join(format!("{name}.txt"));
    if !path.exists() {
        config::write_private_file(&path, b"")?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_path_uses_named_private_history_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = history_path_in(dir.path().to_path_buf(), "chat").unwrap();
        assert!(path.ends_with("history/chat.txt"));
        assert!(path.exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let history_dir_mode = std::fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            let file_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(history_dir_mode, 0o700);
            assert_eq!(file_mode, 0o600);
        }
    }
}
