//! `oy review` file-backed artifact arguments.

use clap::{Args, Subcommand};
use std::path::PathBuf;

use super::audit_cmd::FinalizeArgs;

#[derive(Debug, Args, Clone)]
pub(super) struct ReviewArgs {
    #[command(subcommand)]
    pub(super) action: ReviewAction,
}

#[derive(Debug, Clone, Subcommand)]
pub(super) enum ReviewAction {
    /// Prepare immutable review evidence under .oy/runs.
    Prepare(ReviewPrepareArgs),
    /// Validate prepared evidence and write its bound report.
    Finalize(FinalizeArgs),
}

#[derive(Debug, Args, Clone)]
pub(super) struct ReviewPrepareArgs {
    #[arg(
        value_name = "TARGET",
        help = "Optional branch, commit, or ref; omitted reviews the workspace"
    )]
    pub(super) target: Option<String>,
    #[arg(
        long,
        value_name = "PATH",
        default_value = ".",
        help = "Workspace-relative scope when TARGET is omitted"
    )]
    pub(super) path: String,
    #[arg(
        long,
        value_name = "PATH",
        help = "Final report path (default: REVIEW.md)"
    )]
    pub(super) out: Option<PathBuf>,
    #[arg(long, value_name = "TEXT", help = "Review focus text; repeatable")]
    pub(super) focus: Vec<String>,
    #[arg(long, value_name = "N", default_value_t = crate::review::DEFAULT_MAX_REVIEW_CHUNKS, help = "Maximum evidence chunks before failing closed")]
    pub(super) max_chunks: usize,
}

pub(super) fn default_output_path() -> PathBuf {
    crate::review::default_output_path()
}
