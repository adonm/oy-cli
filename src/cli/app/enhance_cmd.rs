//! `oy enhance` convenience argument types.

use clap::Args;

#[derive(Debug, Args, Clone)]
pub(super) struct EnhanceArgs {
    #[arg(
        long,
        default_value_t = false,
        help = "Open the native OpenCode mini UI for permissions, questions, and forms"
    )]
    pub(super) interactive: bool,
    #[arg(
        long,
        value_name = "TARGET",
        help = "Optional branch/commit/ref for the review step; omitted reviews the whole workspace"
    )]
    pub(super) review_target: Option<String>,
    #[arg(
        long,
        value_name = "N",
        default_value_t = crate::audit::DEFAULT_MAX_REVIEW_CHUNKS,
        help = "Maximum audit chunks before failing closed"
    )]
    pub(super) audit_max_chunks: usize,
    #[arg(
        long,
        value_name = "N",
        default_value_t = crate::review::DEFAULT_MAX_REVIEW_CHUNKS,
        help = "Maximum review chunks before failing closed"
    )]
    pub(super) review_max_chunks: usize,
    #[arg(
        value_name = "FOCUS",
        help = "Optional audit/review/remediation focus text"
    )]
    pub(super) focus: Vec<String>,
}
