//! Audit finding types, markdown/JSON extraction, and structured-findings
//! round-trip helpers.

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::transparency::{finish_markdown, finish_markdown_owned};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FindingSummary {
    pub(crate) severity: String,
    pub(crate) title: String,
    pub(crate) code_ref: String,
}

pub(crate) fn extract_findings(lines: &[&str]) -> Vec<FindingSummary> {
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

pub(super) fn severity_from_text(text: &str) -> Option<String> {
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

pub(super) fn code_ref_from_line(line: &str) -> Option<String> {
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
    pub(crate) fn from_summary(source: &str, summary: FindingSummary) -> Self {
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

    pub(crate) fn to_summary_markdown(&self) -> String {
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

pub(super) fn non_empty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
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
}
