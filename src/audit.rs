//! Deterministic audit input and report helpers used by file-backed workflows.

use std::path::PathBuf;

pub(crate) mod findings;
pub(crate) mod input;
pub(crate) mod report;
mod sarif;
pub(crate) mod transparency;

pub const DEFAULT_MAX_REVIEW_CHUNKS: usize = 80;
pub(crate) const MAX_FILE_BYTES: u64 = 512 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOutputFormat {
    Markdown,
    Sarif,
}

impl AuditOutputFormat {
    pub fn name(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Sarif => "sarif",
        }
    }
}

pub fn default_output_path(format: AuditOutputFormat) -> PathBuf {
    report::default_output_path(format)
}
