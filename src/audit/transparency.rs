//! Markdown transparency line, command snippet, and post-processing helpers
//! shared by audit/review report rendering.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::{AuditOutputFormat, DEFAULT_MAX_REVIEW_CHUNKS};

pub(crate) const AUDIT_REPORT_TITLE: &str = "# Audit Issues";
pub(crate) const REVIEW_REPORT_TITLE: &str = "# Code Quality Review";
pub(crate) const TRANSPARENCY_PREFIX: &str =
    "Generated with [oy-cli](https://crates.io/crates/oy-cli):";

pub(crate) fn default_output_path(format: AuditOutputFormat) -> PathBuf {
    match format {
        AuditOutputFormat::Markdown => PathBuf::from("ISSUES.md"),
        AuditOutputFormat::Sarif => PathBuf::from("oy.sarif"),
    }
}

pub(crate) fn audit_transparency_snippet(
    model: Option<&str>,
    focus: Option<&str>,
    out: &std::path::Path,
    max_chunks: Option<usize>,
    format: AuditOutputFormat,
) -> String {
    let mut command = base_command(model, "audit");
    if format != AuditOutputFormat::Markdown {
        command.push("--format".to_string());
        command.push(format.name().to_string());
    }
    if out != default_output_path(format) {
        command.push("--out".to_string());
        command.push(shell_quote(&out.to_string_lossy()));
    }
    push_max_chunks(&mut command, max_chunks);
    if let Some(focus) = non_empty(focus) {
        command.push(shell_quote(focus));
    }
    transparency_snippet(command)
}

pub(crate) fn review_transparency_snippet(
    model: Option<&str>,
    target: Option<&str>,
    focus: Option<&str>,
    out: &std::path::Path,
    max_chunks: Option<usize>,
) -> String {
    let mut command = base_command(model, "review");
    if out != Path::new("REVIEW.md") {
        command.push("--out".to_string());
        command.push(shell_quote(&out.to_string_lossy()));
    }
    push_max_chunks(&mut command, max_chunks);
    if let Some(target) = non_empty(target) {
        command.push(shell_quote(target));
    }
    if let Some(focus) = non_empty(focus) {
        command.push("--focus".to_string());
        command.push(shell_quote(focus));
    }
    transparency_snippet(command)
}

fn base_command(model: Option<&str>, workflow: &str) -> Vec<String> {
    let mut command = Vec::new();
    if let Some(model) = non_empty(model) {
        command.push(format!("OY_OPENCODE_MODEL={}", shell_quote(model)));
    }
    command.push("oy".to_string());
    command.push(workflow.to_string());
    command
}

fn push_max_chunks(command: &mut Vec<String>, max_chunks: Option<usize>) {
    if let Some(max_chunks) = max_chunks
        && max_chunks != DEFAULT_MAX_REVIEW_CHUNKS
    {
        command.push("--max-chunks".to_string());
        command.push(max_chunks.to_string());
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
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

fn transparency_snippet(command: Vec<String>) -> String {
    format!(
        "> {} `{}` · {}",
        TRANSPARENCY_PREFIX,
        command.join(" "),
        utc_date_string()
    )
}

pub(crate) fn with_audit_transparency_line(report: &str, snippet: &str) -> String {
    with_report_transparency_line(report, snippet, AUDIT_REPORT_TITLE, TRANSPARENCY_PREFIX)
}

pub(crate) fn with_review_transparency_line(report: &str, snippet: &str) -> String {
    with_report_transparency_line(report, snippet, REVIEW_REPORT_TITLE, TRANSPARENCY_PREFIX)
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

    let insert_at = transparency_insert_index(&lines, AUDIT_REPORT_TITLE, TRANSPARENCY_PREFIX);
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

fn has_heading(lines: &[&str], heading: &str) -> bool {
    lines.iter().any(|line| {
        line.trim_start_matches('#')
            .trim()
            .eq_ignore_ascii_case(heading)
    })
}

fn transparency_insert_index(lines: &[&str], title: &str, transparency_prefix: &str) -> usize {
    lines
        .iter()
        .position(|line| line.starts_with(&format!("> {transparency_prefix}")))
        .map(|idx| idx + 1)
        .unwrap_or_else(|| {
            lines
                .iter()
                .position(|line| line.trim() == title)
                .map(|idx| idx + 1)
                .unwrap_or(0)
        })
}

fn utc_date_string() -> String {
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}-{month:02}-{day:02}")
}

fn civil_from_days(days_since_unix_epoch: i64) -> (i64, i64, i64) {
    let days = days_since_unix_epoch + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    let year = year + if month <= 2 { 1 } else { 0 };
    (year, month, day)
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
    fn transparency_line_quotes_audit_context() {
        let snippet = audit_transparency_snippet(
            Some("my model"),
            Some("auth paths"),
            &PathBuf::from("audit output.md"),
            Some(120),
            AuditOutputFormat::Markdown,
        );
        assert!(snippet.contains(
            "OY_OPENCODE_MODEL='my model' oy audit --out 'audit output.md' --max-chunks 120 'auth paths'"
        ));
    }

    #[test]
    fn transparency_line_quotes_review_context() {
        let snippet = review_transparency_snippet(
            Some("my model"),
            Some("feature branch"),
            Some("types and boundaries"),
            &PathBuf::from("review output.md"),
            Some(120),
        );
        assert!(snippet.contains(
            "OY_OPENCODE_MODEL='my model' oy review --out 'review output.md' --max-chunks 120 'feature branch' --focus 'types and boundaries'"
        ));
    }

    #[test]
    fn with_transparency_line_inserts_title_and_replaces_existing_line() {
        let out = with_audit_transparency_line(
            "> Generated with [oy-cli](https://crates.io/crates/oy-cli): `old` · 2026-01-01\n\n## Details\n",
            "> Generated with [oy-cli](https://crates.io/crates/oy-cli): `oy audit` · 2026-06-06",
        );
        assert!(out.starts_with("# Audit Issues\n\n> Generated with [oy-cli]"));
        assert!(out.contains("`oy audit`"));
        assert!(!out.contains("`old`"));
    }

    #[test]
    fn summary_is_inserted_after_transparency_line() {
        let out = with_succinct_findings_summary(
            "# Audit Issues\n\n> Generated with [oy-cli](https://crates.io/crates/oy-cli): `oy audit` · 2026-06-06\n\n## Detailed findings\n\n### High: path traversal reaches file writes\n\n- Evidence: `src/files.rs:42` passes user input into write.\n",
        );
        assert!(out.find("Generated with").unwrap() < out.find("## Findings summary").unwrap());
        assert!(
            out.find("## Findings summary").unwrap() < out.find("## Detailed findings").unwrap()
        );
    }

    #[test]
    fn civil_date_conversion_matches_unix_epoch() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_610), (2026, 6, 6));
    }
}
