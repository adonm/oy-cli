//! Report post-processing: transparency lines, succinct findings
//! summaries, finding extraction, and shell quoting.

use chrono::Utc;
use regex::Regex;
use std::path::PathBuf;

use super::{AuditOptions, AuditOutputFormat, DEFAULT_MAX_REVIEW_CHUNKS, prompts};

pub(super) fn transparency_snippet(options: &AuditOptions) -> String {
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

pub(super) fn with_transparency_line(report: &str, snippet: &str) -> String {
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

pub(super) fn with_succinct_findings_summary(report: &str) -> String {
    let lines = report.lines().collect::<Vec<_>>();
    if has_heading(&lines, "Findings summary") {
        return finish_markdown(lines);
    }
    let findings = extract_findings(&lines);
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
    rebuilt.extend(findings.into_iter().map(|finding| finding.to_markdown()));
    rebuilt.push(String::new());
    rebuilt.extend(lines[insert_at..].iter().map(|line| (*line).to_string()));
    finish_markdown_owned(rebuilt)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FindingSummary {
    pub(super) severity: String,
    pub(super) title: String,
    pub(super) code_ref: String,
}

impl FindingSummary {
    fn to_markdown(&self) -> String {
        format!(
            "- **{}** `{}` — {}",
            self.severity, self.code_ref, self.title
        )
    }
}

pub(super) fn extract_findings(lines: &[&str]) -> Vec<FindingSummary> {
    static HEADING_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"^(#{3,4})\s+(.+?)\s*$").expect("valid heading regex")
    });
    let mut findings = Vec::new();
    let mut current: Option<(FindingHeading, Vec<&str>)> = None;

    for line in lines {
        if let Some(captures) = HEADING_RE.captures(line) {
            if let Some((heading, body)) = current.take()
                && let Some(finding) = finding_from_section(heading, &body)
            {
                findings.push(finding);
            }
            let heading = captures
                .get(2)
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default();
            if let Some(heading) = parse_finding_heading(&heading) {
                current = Some((heading, Vec::new()));
            } else {
                current = None;
            }
        } else if let Some((_, body)) = current.as_mut() {
            body.push(line);
        }
    }
    if let Some((heading, body)) = current.take()
        && let Some(finding) = finding_from_section(heading, &body)
    {
        findings.push(finding);
    }
    findings
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FindingHeading {
    severity: String,
    title: String,
}

fn parse_finding_heading(heading: &str) -> Option<FindingHeading> {
    let heading = heading.trim();
    if is_ignored_report_heading(heading) {
        return None;
    }
    static HEADING_FINDING_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(
            r"(?i)^\s*(?:\[(critical|high|medium|low|info|informational)\]\s*(?:[:—–-]\s*)?|(critical|high|medium|low|info|informational)(?:\s*[:—–]\s*|\s+-\s+))(\S.*)$",
        )
        .expect("valid finding heading regex")
    });
    let captures = HEADING_FINDING_RE.captures(heading)?;
    let severity = captures
        .get(1)
        .or_else(|| captures.get(2))
        .and_then(|value| severity_from_text(value.as_str()))?;
    let title = captures
        .get(3)
        .map(|value| value.as_str().trim().to_string())
        .filter(|title| !title.is_empty())?;
    Some(FindingHeading { severity, title })
}

fn finding_from_section(heading: FindingHeading, body: &[&str]) -> Option<FindingSummary> {
    let code_ref = body
        .iter()
        .find_map(|line| code_ref_from_line(line))
        .or_else(|| code_ref_from_line(&heading.title))?;
    Some(FindingSummary {
        severity: heading.severity,
        title: heading.title,
        code_ref,
    })
}

fn is_ignored_report_heading(heading: &str) -> bool {
    let lower = heading.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "findings summary"
            | "summary"
            | "detailed findings"
            | "details"
            | "no concrete findings"
            | "audit issues"
    )
}

fn severity_from_text(text: &str) -> Option<String> {
    static SEVERITY_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?i)\b(critical|high|medium|low|info|informational)\b")
            .expect("valid severity regex")
    });
    SEVERITY_RE
        .captures(text)
        .and_then(|captures| captures.get(1))
        .map(
            |match_| match match_.as_str().to_ascii_lowercase().as_str() {
                "critical" => "Critical".to_string(),
                "high" => "High".to_string(),
                "medium" => "Medium".to_string(),
                "low" => "Low".to_string(),
                _ => "Info".to_string(),
            },
        )
}

fn code_ref_from_line(line: &str) -> Option<String> {
    static CODE_REF_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"[A-Za-z0-9_.@+\-/]+\.[A-Za-z0-9]+(?::\d+)?(?:::[A-Za-z_][A-Za-z0-9_]*)?")
            .expect("valid code reference regex")
    });
    CODE_REF_RE.find(line).map(|match_| {
        match_
            .as_str()
            .trim_matches(|ch: char| ch == '`' || ch == ',' || ch == ')' || ch == ']')
            .to_string()
    })
}

fn has_heading(lines: &[&str], heading: &str) -> bool {
    lines.iter().any(|line| {
        line.trim_start_matches('#')
            .trim()
            .eq_ignore_ascii_case(heading)
    })
}

fn transparency_insert_index(lines: &[&str]) -> usize {
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

fn finish_markdown(lines: Vec<&str>) -> String {
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn finish_markdown_owned(lines: Vec<String>) -> String {
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_findings_accepts_explicit_severity_headings() {
        let report = [
            "# Audit Issues",
            "",
            "## Detailed findings",
            "",
            "### [High] Shell env leaks src/tools/shell.rs::tool_bash",
            "",
            "Evidence: `src/tools/shell.rs:48` passes env through.",
            "",
            "### Medium: Workspace write can partially apply",
            "Evidence: src/cli/config/paths.rs::write_workspace_batch",
        ];

        assert_eq!(
            extract_findings(&report),
            vec![
                FindingSummary {
                    severity: "High".to_string(),
                    title: "Shell env leaks src/tools/shell.rs::tool_bash".to_string(),
                    code_ref: "src/tools/shell.rs:48".to_string(),
                },
                FindingSummary {
                    severity: "Medium".to_string(),
                    title: "Workspace write can partially apply".to_string(),
                    code_ref: "src/cli/config/paths.rs::write_workspace_batch".to_string(),
                },
            ]
        );
    }

    #[test]
    fn extract_findings_rejects_non_finding_subheadings() {
        let report = [
            "# Audit Issues",
            "",
            "## Detailed findings",
            "",
            "### High-level overview",
            "src/overview.rs should not become a finding just because it has a path.",
            "",
            "### Evidence",
            "src/evidence.rs should not become a finding either.",
            "",
            "### Low: Concrete issue",
            "Evidence: `src/lib.rs:10`",
        ];

        assert_eq!(
            extract_findings(&report),
            vec![FindingSummary {
                severity: "Low".to_string(),
                title: "Concrete issue".to_string(),
                code_ref: "src/lib.rs:10".to_string(),
            }]
        );
    }
}
