//! `oy audit` workflow and file-backed artifact arguments.

use clap::{Args, Subcommand, ValueEnum};
use std::path::PathBuf;

use crate::audit;

#[derive(Debug, Args, Clone)]
pub(super) struct AuditArgs {
    #[command(subcommand)]
    pub(super) action: Option<AuditAction>,
    #[arg(long, value_enum, default_value_t = AuditFormat::Markdown, help = "Output format: markdown or sarif")]
    pub(super) format: AuditFormat,
    #[arg(
        long,
        value_name = "PATH",
        help = "Write findings to a workspace file (default: ISSUES.md or oy.sarif)"
    )]
    pub(super) out: Option<PathBuf>,
    #[arg(long, value_name = "N", default_value_t = audit::DEFAULT_MAX_REVIEW_CHUNKS, help = "Maximum audit chunks before failing closed")]
    pub(super) max_chunks: usize,
    #[arg(value_name = "FOCUS", help = "Optional audit focus text")]
    pub(super) focus: Vec<String>,
}

#[derive(Debug, Clone, Subcommand)]
pub(super) enum AuditAction {
    /// Prepare immutable audit evidence under .oy/runs.
    Prepare(AuditPrepareArgs),
    /// Validate prepared evidence and write its bound report.
    Finalize(FinalizeArgs),
}

#[derive(Debug, Args, Clone)]
pub(super) struct AuditPrepareArgs {
    #[arg(
        long,
        value_name = "PATH",
        default_value = ".",
        help = "Workspace-relative file or directory scope"
    )]
    pub(super) path: String,
    #[arg(long, value_enum, default_value_t = AuditFormat::Markdown, help = "Final report format: markdown or sarif")]
    pub(super) format: AuditFormat,
    #[arg(
        long,
        value_name = "PATH",
        help = "Final report path (default: ISSUES.md or oy.sarif)"
    )]
    pub(super) out: Option<PathBuf>,
    #[arg(long, value_name = "TEXT", help = "Audit focus text; repeatable")]
    pub(super) focus: Vec<String>,
    #[arg(long, value_name = "N", default_value_t = audit::DEFAULT_MAX_REVIEW_CHUNKS, help = "Maximum evidence chunks before failing closed")]
    pub(super) max_chunks: usize,
}

#[derive(Debug, Args, Clone)]
pub(super) struct FinalizeArgs {
    #[arg(long, value_name = "RUN_ID", help = "Run ID returned by prepare")]
    pub(super) run: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(super) enum AuditFormat {
    Markdown,
    Sarif,
}

impl From<AuditFormat> for audit::AuditOutputFormat {
    fn from(format: AuditFormat) -> Self {
        match format {
            AuditFormat::Markdown => Self::Markdown,
            AuditFormat::Sarif => Self::Sarif,
        }
    }
}

impl AuditFormat {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Sarif => "sarif",
        }
    }
}
