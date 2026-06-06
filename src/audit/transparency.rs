//! Markdown post-processing helpers shared by audit/review report rendering.

use std::path::PathBuf;

use super::AuditOutputFormat;

pub(crate) fn default_output_path(format: AuditOutputFormat) -> PathBuf {
    match format {
        AuditOutputFormat::Markdown => PathBuf::from("ISSUES.md"),
        AuditOutputFormat::Sarif => PathBuf::from("oy.sarif"),
    }
}

pub(crate) fn with_succinct_findings_summary(report: &str) -> String {
    use super::findings::{Finding, extract_findings};

    let lines = report.lines().collect::<Vec<_>>();
    if has_heading(&lines, "Findings summary") {
        return finish_markdown(lines);
    }
    let findings: Vec<Finding> = extract_findings(&lines)
        .into_iter()
        .map(|summary| Finding::from_summary("", summary))
        .collect();
    if findings.is_empty() {
        return finish_markdown(lines);
    }

    let mut rebuilt = Vec::with_capacity(lines.len() + findings.len() + 4);
    rebuilt.push(
        lines
            .first()
            .copied()
            .unwrap_or("# Audit Issues")
            .to_string(),
    );
    rebuilt.push(String::new());
    rebuilt.push("## Findings summary".to_string());
    rebuilt.push(String::new());
    rebuilt.extend(
        findings
            .into_iter()
            .map(|finding| finding.to_summary_markdown()),
    );
    rebuilt.push(String::new());
    rebuilt.extend(lines.into_iter().skip(1).map(str::to_string));
    finish_markdown_owned(rebuilt)
}

fn has_heading(lines: &[&str], heading: &str) -> bool {
    lines.iter().any(|line| {
        line.trim_start_matches('#')
            .trim()
            .eq_ignore_ascii_case(heading)
    })
}

pub(crate) fn finish_markdown(lines: Vec<&str>) -> String {
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

pub(crate) fn finish_markdown_owned(lines: Vec<String>) -> String {
    let mut out = lines.join("\n");
    out.push('\n');
    out
}
