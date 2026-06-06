//! Review compatibility constants used by OpenCode wrappers.

use std::path::PathBuf;

pub const DEFAULT_MAX_REVIEW_CHUNKS: usize = 80;

pub fn default_output_path() -> PathBuf {
    PathBuf::from("REVIEW.md")
}
