//! Report post-processing: transparency lines, succinct findings
//! summaries, finding extraction, and shell quoting.

use chrono::Utc;
use regex::Regex;
use serde::{Deserialize, Serialize};
use anyhow::{Result, bail};
use serde_json::Value;
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
    let findings = findings_from_report(report);
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

pub(crate) fn with_structured_findings_block(report: &str, source: &str) -> String {
    let lines = report.lines().collect::<Vec<_>>();
    if !structured_findings_from_report(report).is_empty() {
        return finish_markdown(lines);
    }

    let findings = extract_findings(&lines)
        .into_iter()
        .map(|finding| Finding::from_summary(source, finding))
        .collect::<Vec<_>>();
    if findings.is_empty() {
        return finish_markdown(lines);
    }

    let Some(payload) = serde_json::to_string_pretty(&findings).ok() else {
        return finish_markdown(lines);
    };
    let mut rebuilt = lines
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<String>>();
    if rebuilt.last().is_some_and(|line| !line.trim().is_empty()) {
        rebuilt.push(String::new());
    }
    rebuilt.push("## Machine-readable findings".to_string());
    rebuilt.push(String::new());
    rebuilt.push("```json oy-findings".to_string());
    rebuilt.extend(payload.lines().map(str::to_string));
    rebuilt.push("```".to_string());
    finish_markdown_owned(rebuilt)
}

pub(crate) fn findings_from_report(report: &str) -> Vec<Finding> {
    let structured = structured_findings_from_report(report);
    if !structured.is_empty() {
        return structured;
    }
    extract_findings(&report.lines().collect::<Vec<_>>())
        .into_iter()
        .map(|finding| Finding::from_summary("", finding))
        .collect()
}

pub(crate) fn structured_findings_from_report(report: &str) -> Vec<Finding> {
    structured_findings_payload(report)
        .map(|payload| parse_structured_findings_payload(&payload))
        .unwrap_or_default()
}

fn structured_findings_payload(report: &str) -> Option<String> {
    let mut payload = String::new();
    let mut in_block = false;
    for line in report.lines() {
        let trimmed = line.trim();
        if !in_block {
            let Some(info) = trimmed.strip_prefix("```") else {
                continue;
            };
            if info
                .split_whitespace()
                .any(|part| part.eq_ignore_ascii_case("oy-findings"))
            {
                in_block = true;
            }
            continue;
        }
        if trimmed.starts_with("```") {
            return Some(payload);
        }
        payload.push_str(line);
        payload.push('\n');
    }
    None
}

fn parse_structured_findings_payload(payload: &str) -> Vec<Finding> {
    let Ok(value) = serde_json::from_str::<Value>(payload) else {
        return Vec::new();
    };
    let items = value
        .as_array()
        .or_else(|| value.get("findings").and_then(Value::as_array));
    let Some(items) = items else {
        return Vec::new();
    };
    items.iter().filter_map(finding_from_value).collect()
}

fn finding_from_value(value: &Value) -> Option<Finding> {
    let body = value_text(value.get("body"));
    let body = if body.trim().is_empty() {
        value_text(value.get("details"))
    } else {
        body
    };
    let mut finding = Finding {
        source: value_text(value.get("source")),
        severity: value_text(value.get("severity")),
        title: value_text(value.get("title")),
        locations: locations_from_value(value),
        evidence: value_text(value.get("evidence")),
        body,
        category: non_empty(value_text(value.get("category"))),
    };
    finding.normalize()?;
    Some(finding)
}

fn locations_from_value(value: &Value) -> Vec<FindingLocation> {
    let mut locations = Vec::new();
    if let Some(items) = value.get("locations").and_then(Value::as_array) {
        locations.extend(items.iter().filter_map(FindingLocation::from_value));
    }
    if let Some(location) = value.get("location").and_then(FindingLocation::from_value) {
        locations.push(location);
    }
    locations
}

fn value_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.trim().to_string(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(value) => non_empty(value.trim().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn non_empty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Finding {
    pub(crate) source: String,
    pub(crate) severity: String,
    pub(crate) title: String,
    pub(crate) locations: Vec<FindingLocation>,
    pub(crate) evidence: String,
    pub(crate) body: String,
    pub(crate) category: Option<String>,
}

impl Finding {
    fn from_summary(source: &str, summary: FindingSummary) -> Self {
        let mut finding = Self {
            source: source.to_string(),
            severity: summary.severity,
            title: summary.title,
            locations: FindingLocation::from_code_ref(&summary.code_ref)
                .into_iter()
                .collect(),
            evidence: String::new(),
            body: String::new(),
            category: None,
        };
        let _ = finding.normalize();
        finding
    }

    fn normalize(&mut self) -> Option<()> {
        self.source = self.source.trim().to_ascii_lowercase();
        self.title = self.title.trim().to_string();
        if self.title.is_empty() {
            return None;
        }
        self.severity = severity_from_text(&self.severity)
            .or_else(|| severity_from_text(&self.title))
            .unwrap_or_else(|| "Info".to_string());
        self.evidence = self.evidence.trim().to_string();
        self.body = self.body.trim().to_string();
        self.category = self.category.take().and_then(non_empty);
        self.locations.retain(|location| !location.path.is_empty());
        if self.locations.is_empty()
            && let Some(code_ref) = code_ref_from_line(&self.evidence)
                .or_else(|| code_ref_from_line(&self.body))
                .or_else(|| code_ref_from_line(&self.title))
            && let Some(location) = FindingLocation::from_code_ref(&code_ref)
        {
            self.locations.push(location);
        }
        Some(())
    }

    fn to_summary_markdown(&self) -> String {
        format!(
            "- **{}** `{}` — {}",
            self.severity,
            self.primary_code_ref()
                .unwrap_or_else(|| "unknown".to_string()),
            self.title
        )
    }

    pub(crate) fn primary_code_ref(&self) -> Option<String> {
        self.locations.first().map(FindingLocation::code_ref)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FindingLocation {
    pub(crate) path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) symbol: Option<String>,
}

impl FindingLocation {
    fn from_value(value: &Value) -> Option<Self> {
        if let Value::String(code_ref) = value {
            return Self::from_code_ref(code_ref);
        }
        let object = value.as_object()?;
        if let Some(code_ref) = object
            .get("code_ref")
            .or_else(|| object.get("ref"))
            .and_then(Value::as_str)
            && let Some(location) = Self::from_code_ref(code_ref)
        {
            return Some(location);
        }
        let path = object
            .get("path")
            .or_else(|| object.get("uri"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if path.is_empty() {
            return None;
        }
        let line = object
            .get("line")
            .and_then(Value::as_u64)
            .and_then(|line| u32::try_from(line).ok());
        let symbol = object
            .get("symbol")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|symbol| !symbol.is_empty())
            .map(str::to_string);
        Some(Self { path, line, symbol })
    }

    fn from_code_ref(code_ref: &str) -> Option<Self> {
        let code_ref = code_ref.trim().trim_matches('`');
        let (path_and_line, symbol) = code_ref
            .split_once("::")
            .map(|(path, symbol)| (path, Some(symbol.trim().to_string())))
            .unwrap_or((code_ref, None));
        let (path, line) = path_and_line
            .rsplit_once(':')
            .and_then(|(path, line)| line.parse::<u32>().ok().map(|line| (path, Some(line))))
            .unwrap_or((path_and_line, None));
        let path = path.trim().to_string();
        if path.is_empty() {
            return None;
        }
        Some(Self {
            path,
            line,
            symbol: symbol.and_then(non_empty),
        })
    }

    pub(crate) fn code_ref(&self) -> String {
        if let Some(line) = self.line {
            format!("{}:{line}", self.path)
        } else if let Some(symbol) = self.symbol.as_deref().filter(|symbol| !symbol.is_empty()) {
            format!("{}::{symbol}", self.path)
        } else {
            self.path.clone()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FindingSummary {
    pub(super) severity: String,
    pub(super) title: String,
    pub(super) code_ref: String,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum FindingSource {
    Audit,
    Review,
}

impl FindingSource {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Audit => "audit",
            Self::Review => "review",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "audit" => Ok(Self::Audit),
            "review" => Ok(Self::Review),
            other => bail!("invalid enhance finding source in resume state: {other}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EnhanceFinding {
    pub(crate) source: FindingSource,
    pub(crate) title: String,
    pub(crate) body: String,
}

impl EnhanceFinding {
    pub(crate) fn summary(&self) -> String {
        format!("{}: {}", self.source.label(), self.title)
    }
}

pub(crate) fn parse_findings(source: FindingSource, report: &str) -> Vec<EnhanceFinding> {
    let structured = structured_findings_from_report(report)
        .into_iter()
        .filter(|finding| !is_no_finding_title(&finding.title))
        .map(|finding| EnhanceFinding {
            source: FindingSource::parse(&finding.source).unwrap_or(source),
            title: clean_title(&format!("{}: {}", finding.severity, finding.title)),
            body: typed_finding_body(&finding),
        })
        .collect::<Vec<_>>();
    if !structured.is_empty() {
        return structured;
    }

    let mut findings = section_after_heading(report, "Detailed findings")
        .map(|detailed| {
            let mut findings = parse_heading_findings(source, detailed, 3);
            if findings.is_empty() {
                findings = parse_heading_findings(source, detailed, 2);
            }
            findings
        })
        .unwrap_or_default();
    if findings.is_empty() {
        findings = parse_summary_bullets(
            source,
            section_after_heading(report, "Findings summary").unwrap_or(report),
        );
    }
    findings
}

fn typed_finding_body(finding: &Finding) -> String {
    let mut body = String::new();
    body.push_str(&format!("### {}: {}\n", finding.severity, finding.title));
    if let Some(location) = finding.primary_code_ref() {
        body.push_str(&format!("\n- Location: `{location}`\n"));
    }
    if let Some(category) = finding.category.as_deref() {
        body.push_str(&format!("- Category: {category}\n"));
    }
    if !finding.evidence.trim().is_empty() {
        body.push_str("\nEvidence:\n");
        body.push_str(finding.evidence.trim());
        body.push('\n');
    }
    if !finding.body.trim().is_empty() {
        body.push('\n');
        body.push_str(finding.body.trim());
        body.push('\n');
    }
    body.trim().to_string()
}

fn section_after_heading<'a>(report: &'a str, heading: &str) -> Option<&'a str> {
    let wanted = heading.trim().to_ascii_lowercase();
    let start = report.lines().scan(0usize, |offset, line| {
        let current = *offset;
        *offset += line.len() + 1;
        Some((current, line))
    });
    let mut body_start = None;
    for (offset, line) in start {
        let trimmed = line.trim();
        if let Some((level, title)) = markdown_heading(trimmed)
            && level == 2
            && title.eq_ignore_ascii_case(&wanted)
        {
            body_start = Some(offset + line.len() + 1);
            break;
        }
    }
    let body_start = body_start?;
    let rest = report.get(body_start..).unwrap_or_default();
    let mut body_end = rest.len();
    let mut offset = 0usize;
    for line in rest.lines() {
        let trimmed = line.trim();
        if let Some((level, _)) = markdown_heading(trimmed)
            && level <= 2
        {
            body_end = offset;
            break;
        }
        offset += line.len() + 1;
    }
    Some(rest[..body_end].trim())
}

fn parse_heading_findings(source: FindingSource, text: &str, level: usize) -> Vec<EnhanceFinding> {
    let mut findings = Vec::new();
    let mut current_title: Option<String> = None;
    let mut current_body = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some((line_level, title)) = markdown_heading(trimmed)
            && line_level == level
        {
            push_heading_finding(source, &mut findings, current_title.take(), &current_body);
            current_title = Some(title.to_string());
            current_body.clear();
            current_body.push_str(line);
            current_body.push('\n');
            continue;
        }
        if current_title.is_some() {
            current_body.push_str(line);
            current_body.push('\n');
        }
    }
    push_heading_finding(source, &mut findings, current_title, &current_body);
    findings
}

fn push_heading_finding(
    source: FindingSource,
    findings: &mut Vec<EnhanceFinding>,
    title: Option<String>,
    body: &str,
) {
    let Some(title) = title else {
        return;
    };
    if is_no_finding_title(&title) {
        return;
    }
    findings.push(EnhanceFinding {
        source,
        title: clean_title(&title),
        body: body.trim().to_string(),
    });
}

fn parse_summary_bullets(source: FindingSource, text: &str) -> Vec<EnhanceFinding> {
    text.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let body = trimmed
                .strip_prefix("- ")
                .or_else(|| trimmed.strip_prefix("* "))?;
            if is_no_finding_title(body) {
                return None;
            }
            Some(EnhanceFinding {
                source,
                title: clean_title(body),
                body: trimmed.to_string(),
            })
        })
        .collect()
}

fn markdown_heading(line: &str) -> Option<(usize, &str)> {
    let hashes = line.chars().take_while(|ch| *ch == '#').count();
    if !(1..=6).contains(&hashes) || !line.chars().nth(hashes).is_some_and(char::is_whitespace) {
        return None;
    }
    Some((hashes, line[hashes..].trim()))
}

fn clean_title(title: &str) -> String {
    let title = title
        .trim()
        .trim_matches(['`', '*', ' '])
        .replace("**", "")
        .replace('`', "");
    crate::ui::truncate_chars(&title, 120)
}

fn is_no_finding_title(title: &str) -> bool {
    let lower = title.trim().to_ascii_lowercase();
    lower.contains("no findings")
        || lower.contains("no major")
        || lower.contains("no addressable")
        || lower.contains("nothing to fix")
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

    #[test]
    fn structured_findings_block_is_preferred_over_markdown() {
        let report = r#"# Audit Issues

## Detailed findings

### Low: stale markdown
Evidence: src/old.rs:1

## Machine-readable findings

```json oy-findings
[
  {
    "source": "audit",
    "severity": "High",
    "title": "typed source of truth",
    "locations": [{ "path": "src/new.rs", "line": 7 }],
    "evidence": "src/new.rs:7 proves it",
    "body": "Fix the typed path.",
    "category": "security"
  }
]
```
"#;

        let findings = findings_from_report(report);

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, "High");
        assert_eq!(findings[0].title, "typed source of truth");
        assert_eq!(
            findings[0].primary_code_ref().as_deref(),
            Some("src/new.rs:7")
        );
    }

    #[test]
    fn structured_findings_block_can_be_added_from_legacy_markdown() {
        let report = with_structured_findings_block(
            "# Audit Issues\n\n## Detailed findings\n\n### Medium: legacy finding\nEvidence: src/lib.rs:3\n",
            "audit",
        );

        assert!(report.contains("## Machine-readable findings"));
        assert!(report.contains("```json oy-findings"));
        let findings = structured_findings_from_report(&report);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].source, "audit");
        assert_eq!(
            findings[0].primary_code_ref().as_deref(),
            Some("src/lib.rs:3")
        );
    }

    #[test]
    fn parses_detailed_markdown_findings() {
        let report = "# Audit Issues\n\n## Detailed findings\n\n### High: path traversal\n\n- Evidence: `src/files.rs:42`\n\n### Low: retry loop\n\n- Evidence: `src/retry.rs:7`\n";
        let findings = parse_findings(FindingSource::Audit, report);
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].title, "High: path traversal");
        assert!(findings[0].body.contains("src/files.rs:42"));
    }

    #[test]
    fn parses_summary_bullets_when_details_are_absent() {
        let report = "# Code Quality Review\n\n## Findings summary\n\n- **High** `src/lib.rs:1` — split large function\n";
        let findings = parse_findings(FindingSource::Review, report);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].title.contains("split large function"));
    }

    #[test]
    fn parses_typed_findings_before_markdown_fallback() {
        let report = r#"# Code Quality Review

## Detailed findings

### Low: stale markdown
Evidence: src/old.rs:1

## Machine-readable findings

```json oy-findings
[
  {
    "source": "review",
    "severity": "High",
    "title": "split report API from prose",
    "locations": [{ "path": "src/report.rs", "line": 12 }],
    "evidence": "src/report.rs:12 reparses markdown",
    "body": "Use the typed finding list.",
    "category": "design"
  }
]
```
"#;

        let findings = parse_findings(FindingSource::Review, report);

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].source, FindingSource::Review);
        assert_eq!(findings[0].title, "High: split report API from prose");
        assert!(findings[0].body.contains("src/report.rs:12"));
        assert!(!findings[0].body.contains("src/old.rs:1"));
    }
}
