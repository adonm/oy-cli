use anyhow::{Result, bail};
use std::fs;
use std::path::Path;

use super::MAX_WORKSPACE_FILE_BYTES;

pub(crate) fn read_file_content(_root: &Path, path: &Path) -> Result<String> {
    if fs::metadata(path)?.len() > MAX_WORKSPACE_FILE_BYTES {
        bail!(
            "file exceeds workspace read cap of {} bytes: {}",
            MAX_WORKSPACE_FILE_BYTES,
            path.display()
        );
    }
    let raw = fs::read(path)?;
    crate::decode_utf8(raw)
        .map_err(|_| anyhow::anyhow!("file is not utf-8 text: {}", path.display()))
}
