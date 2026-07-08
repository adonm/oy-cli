//! Audit report post-processing facade.
//!
//! Historically this module held every report concern (transparency lines,
//! finding types, structured-findings I/O, enhance parsing, and markdown
//! helpers) in a single 950+ line file. It now re-exports the focused
//! [`transparency`], [`findings`], and [`enhance`] submodules so external
//! callers can keep using `crate::audit::report::...` while each concern
//! lives in its own file.

pub(crate) use super::findings::{
    Finding, findings_from_report, normalized_findings_payload, with_structured_findings_block,
    with_structured_findings_payload,
};
pub(crate) use super::sarif::render_sarif;
pub(crate) use super::transparency::{
    audit_transparency_snippet, default_output_path, review_transparency_snippet,
    with_audit_transparency_line, with_review_transparency_line, with_succinct_findings_summary,
};
