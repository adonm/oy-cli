//! Audit transparency line, command snippet, and markdown
//! post-processing helpers shared by every audit output format.

use std::path::PathBuf;

use chrono::Utc;

use super::prompts;
use super::{AuditOptions, AuditOutputFormat, DEFAULT_MAX_REVIEW_CHUNKS};

pub(crate) fn transparency_snippet(options: &AuditOptions) -> String {
    let mut command = Vec::new();
    if !options.model.trim().is_empty() {
        command.push(format!("OY_MODEL={}", shell_quote(options.model.trim())));
    }
    command.push("oy".to_string());
    command.push("audit".to_string());
    if options.format != AuditOutputFormat::Markdown {
        command.push("--format".to_string());
        command.push(options.format.name().to_string());
    }
    if options.out != default_output_path(options.format) {
        command.push("--out".to_string());
        command.push(shell_quote(&options.out.to_string_lossy()));
    }
    if options.max_chunks != DEFAULT_MAX_REVIEW_CHUNKS {
        command.push("--max-chunks".to_string());
        command.push(options.max_chunks.to_string());
    }
    if !options.focus.trim().is_empty() {
        command.push(shell_quote(options.focus.trim()));
    }
    format!(
        "> {} `{}` · {}",
        prompts::AUDIT_TRANSPARENCY_PREFIX,
        command.join(" "),
        Utc::now().format("%Y-%m-%d")
    )
}

pub(crate) fn default_output_path(format: AuditOutputFormat) -> PathBuf {
    match format {
        AuditOutputFormat::Markdown => PathBuf::from("ISSUES.md"),
        AuditOutputFormat::Sarif => PathBuf::from("oy.sarif"),
    }
}

pub(crate) fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | ':' | '='))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(crate) fn with_transparency_line(report: &str, snippet: &str) -> String {
    with_report_transparency_line(
        report,
        snippet,
        prompts::AUDIT_REPORT_TITLE,
        prompts::AUDIT_TRANSPARENCY_PREFIX,
    )
}

pub(crate) fn with_report_transparency_line(
    report: &str,
    snippet: &str,
    title: &str,
    transparency_prefix: &str,
) -> String {
    let mut lines = report
        .lines()
        .filter(|line| !line.starts_with(&format!("> {transparency_prefix}")))
        .collect::<Vec<_>>();
    while lines.first().is_some_and(|line| line.trim().is_empty()) {
        lines.remove(0);
    }
    if lines.first().is_none_or(|line| line.trim() != title) {
        lines.insert(0, title);
    }
    let insert_at = 1;
    let mut rebuilt = Vec::new();
    rebuilt.extend_from_slice(&lines[..insert_at]);
    rebuilt.push("");
    rebuilt.push(snippet);
    if lines.len() > insert_at {
        rebuilt.push("");
        for line in &lines[insert_at..] {
            if !line.trim().is_empty() || rebuilt.last().is_some_and(|last| !last.trim().is_empty())
            {
                rebuilt.push(line);
            }
        }
    }
    finish_markdown(rebuilt)
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

    let insert_at = transparency_insert_index(&lines);
    let mut rebuilt = Vec::with_capacity(lines.len() + findings.len() + 4);
    rebuilt.extend(lines[..insert_at].iter().map(|line| (*line).to_string()));
    if rebuilt.last().is_some_and(|line| !line.trim().is_empty()) {
        rebuilt.push(String::new());
    }
    rebuilt.push("## Findings summary".to_string());
    rebuilt.push(String::new());
    rebuilt.extend(
        findings
            .into_iter()
            .map(|finding| finding.to_summary_markdown()),
    );
    rebuilt.push(String::new());
    rebuilt.extend(lines[insert_at..].iter().map(|line| (*line).to_string()));
    finish_markdown_owned(rebuilt)
}

pub(crate) fn has_heading(lines: &[&str], heading: &str) -> bool {
    lines.iter().any(|line| {
        line.trim_start_matches('#')
            .trim()
            .eq_ignore_ascii_case(heading)
    })
}

pub(crate) fn transparency_insert_index(lines: &[&str]) -> usize {
    lines
        .iter()
        .position(|line| line.starts_with(&format!("> {}", prompts::AUDIT_TRANSPARENCY_PREFIX)))
        .map(|idx| idx + 1)
        .unwrap_or_else(|| {
            lines
                .iter()
                .position(|line| line.trim() == prompts::AUDIT_REPORT_TITLE)
                .map(|idx| idx + 1)
                .unwrap_or(0)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_leaves_simple_values_alone() {
        assert_eq!(shell_quote("openai/gpt-5"), "openai/gpt-5");
        assert_eq!(shell_quote("/tmp/repo"), "/tmp/repo");
    }

    #[test]
    fn shell_quote_wraps_values_with_spaces_or_quotes() {
        assert_eq!(shell_quote("hello world"), "'hello world'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn has_heading_matches_with_or_without_leading_hashes() {
        let lines = ["# Title", "", "## Findings summary"];
        assert!(has_heading(&lines, "Findings summary"));
        assert!(has_heading(&lines, "Title"));
        assert!(!has_heading(&lines, "Missing"));
    }
}
