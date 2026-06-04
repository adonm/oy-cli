//! Audit report post-processing facade.
//!
//! Historically this module held every report concern (transparency lines,
//! finding types, structured-findings I/O, enhance parsing, and markdown
//! helpers) in a single 950+ line file. It now re-exports the focused
//! [`transparency`], [`findings`], and [`enhance`] submodules so external
//! callers can keep using `crate::audit::report::...` while each concern
//! lives in its own file.

pub(crate) use super::enhance::{EnhanceFinding, FindingSource, parse_findings};
pub(crate) use super::findings::{Finding, findings_from_report, with_structured_findings_block};
pub(crate) use super::transparency::{
    default_output_path, shell_quote, transparency_snippet, with_report_transparency_line,
    with_succinct_findings_summary, with_transparency_line,
};
