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

fn shell_quote(value: &str) -> String {
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
    let mut lines = report
        .lines()
        .filter(|line| !line.starts_with(&format!("> {}", prompts::AUDIT_TRANSPARENCY_PREFIX)))
        .collect::<Vec<_>>();
    while lines.first().is_some_and(|line| line.trim().is_empty()) {
        lines.remove(0);
    }
    if lines
        .first()
        .is_none_or(|line| line.trim() != prompts::AUDIT_REPORT_TITLE)
    {
        lines.insert(0, prompts::AUDIT_REPORT_TITLE);
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
        Regex::new(r"^(#{2,4})\s+(.+?)\s*$").expect("valid heading regex")
    });
    let mut findings = Vec::new();
    let mut current: Option<(String, Vec<&str>)> = None;

    for line in lines {
        if let Some(captures) = HEADING_RE.captures(line) {
            if let Some((heading, body)) = current.take()
                && let Some(finding) = finding_from_section(&heading, &body)
            {
                findings.push(finding);
            }
            let level = captures.get(1).map(|m| m.as_str().len()).unwrap_or(0);
            let heading = captures
                .get(2)
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default();
            if level >= 2 && is_finding_heading(&heading) {
                current = Some((heading, Vec::new()));
            } else {
                current = None;
            }
        } else if let Some((_, body)) = current.as_mut() {
            body.push(line);
        }
    }
    if let Some((heading, body)) = current.take()
        && let Some(finding) = finding_from_section(&heading, &body)
    {
        findings.push(finding);
    }
    findings
}

fn finding_from_section(heading: &str, body: &[&str]) -> Option<FindingSummary> {
    let severity = severity_from_text(heading)
        .or_else(|| body.iter().find_map(|line| severity_from_text(line)))
        .unwrap_or_else(|| "Unrated".to_string());
    let title = clean_finding_title(heading);
    let code_ref = body
        .iter()
        .find_map(|line| code_ref_from_line(line))
        .or_else(|| code_ref_from_line(heading))?;
    Some(FindingSummary {
        severity,
        title,
        code_ref,
    })
}

fn is_finding_heading(heading: &str) -> bool {
    let lower = heading.to_ascii_lowercase();
    !matches!(
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

fn clean_finding_title(heading: &str) -> String {
    static TITLE_SEVERITY_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(
            r"(?i)^\s*[\[(]?\s*(informational|critical|high|medium|low|info)\s*[\])]?\s*[:—–-]+\s*",
        )
        .expect("valid title severity regex")
    });
    let title = heading.trim().trim_matches('#').trim();
    let title = TITLE_SEVERITY_RE.replace(title, "").trim().to_string();
    if title.is_empty() {
        "Untitled finding".to_string()
    } else {
        title
    }
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
