//! `oy review` compatibility argument types.

use clap::Args;
use std::path::PathBuf;

#[derive(Debug, Args, Clone)]
pub(super) struct ReviewArgs {
    #[arg(
        long,
        value_name = "PATH",
        help = "Write review findings to a workspace file (default: REVIEW.md)"
    )]
    pub(super) out: Option<PathBuf>,
    #[arg(
        long,
        value_name = "N",
        default_value_t = crate::review::DEFAULT_MAX_REVIEW_CHUNKS,
        help = "Maximum review chunks before failing closed"
    )]
    pub(super) max_chunks: usize,
    #[arg(
        long,
        value_name = "TEXT",
        help = "Optional review focus text; can be repeated"
    )]
    pub(super) focus: Vec<String>,
    #[arg(
        value_name = "TARGET",
        help = "Optional branch/commit/ref to diff current workspace against; omitted reviews the whole workspace"
    )]
    pub(super) target: Option<String>,
}

pub(super) fn default_output_path() -> PathBuf {
    crate::review::default_output_path()
}
