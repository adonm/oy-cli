//! `oy audit` convenience argument types.

use clap::ValueEnum;

use crate::audit;

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
