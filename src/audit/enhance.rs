//! Enhance pipeline: parse audit/review findings from reports and shape
//! them for the per-finding remediation session.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::findings::{Finding, structured_findings_from_report};

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

    #[test]
    fn markdown_heading_parses_levels_and_titles() {
        assert_eq!(markdown_heading("# Title"), Some((1, "Title")));
        assert_eq!(
            markdown_heading("## Detailed findings"),
            Some((2, "Detailed findings"))
        );
        assert_eq!(markdown_heading("#### Low: stale"), Some((4, "Low: stale")));
        assert_eq!(markdown_heading("plain text"), None);
        assert_eq!(markdown_heading("####### too deep"), None);
    }

    #[test]
    fn finding_source_round_trips_through_label() {
        assert_eq!(FindingSource::Audit.label(), "audit");
        assert_eq!(FindingSource::Review.label(), "review");
        assert_eq!(FindingSource::parse("audit").unwrap(), FindingSource::Audit);
        assert_eq!(
            FindingSource::parse("review").unwrap(),
            FindingSource::Review
        );
        assert!(FindingSource::parse("other").is_err());
    }
}
