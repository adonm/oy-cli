use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::path::Path;

use super::report;

pub(super) fn render_sarif(report: &str) -> Result<String> {
    let findings = report::extract_findings(&report.lines().collect::<Vec<_>>());
    let mut rules = std::collections::BTreeMap::<String, Value>::new();
    let mut results = Vec::new();

    for finding in findings {
        let Some(location) = sarif_location(&finding.code_ref)? else {
            continue;
        };
        let rule_id = sarif_rule_id(&finding);
        let level = sarif_level(&finding.severity);
        rules.entry(rule_id.clone()).or_insert_with(|| {
            json!({
                "id": rule_id,
                "name": finding.title,
                "shortDescription": { "text": finding.title },
                "defaultConfiguration": { "level": level },
                "properties": {
                    "severity": finding.severity,
                    "security-severity": sarif_security_severity(&finding.severity)
                }
            })
        });
        results.push(json!({
            "ruleId": rule_id,
            "level": level,
            "message": { "text": format!("{}: {}", finding.severity, finding.title) },
            "locations": [location],
            "properties": {
                "severity": finding.severity,
                "codeRef": finding.code_ref
            }
        }));
    }

    let sarif = json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "oy-cli",
                    "semanticVersion": env!("CARGO_PKG_VERSION"),
                    "informationUri": "https://github.com/wagov-dtt/oy-cli",
                    "rules": rules.into_values().collect::<Vec<_>>()
                }
            },
            "results": results,
            "columnKind": "utf16CodeUnits"
        }]
    });
    let mut out = serde_json::to_string_pretty(&sarif)?;
    out.push('\n');
    Ok(out)
}

fn sarif_rule_id(finding: &report::FindingSummary) -> String {
    let mut slug = String::new();
    for ch in finding.title.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() { "finding" } else { slug };
    format!("oy/{}/{}", finding.severity.to_ascii_lowercase(), slug)
}

fn sarif_level(severity: &str) -> &'static str {
    match severity.to_ascii_lowercase().as_str() {
        "critical" | "high" => "error",
        "medium" => "warning",
        _ => "note",
    }
}

fn sarif_security_severity(severity: &str) -> &'static str {
    match severity.to_ascii_lowercase().as_str() {
        "critical" => "9.0",
        "high" => "7.0",
        "medium" => "5.0",
        "low" => "2.0",
        _ => "0.0",
    }
}

fn sarif_location(code_ref: &str) -> Result<Option<Value>> {
    let (path, line) = split_code_ref(code_ref);
    if !is_safe_relative_path(path) {
        bail!("audit finding path escapes workspace: {path}");
    }
    let mut region = serde_json::Map::new();
    if let Some(line) = line {
        region.insert("startLine".to_string(), json!(line));
    }
    let mut physical = serde_json::Map::new();
    physical.insert(
        "artifactLocation".to_string(),
        json!({ "uri": path.replace('\\', "/"), "uriBaseId": "%SRCROOT%" }),
    );
    if !region.is_empty() {
        physical.insert("region".to_string(), Value::Object(region));
    }
    Ok(Some(json!({ "physicalLocation": Value::Object(physical) })))
}

fn split_code_ref(code_ref: &str) -> (&str, Option<u32>) {
    if let Some((path, tail)) = code_ref.rsplit_once(':')
        && !tail.contains(':')
        && let Ok(line) = tail.parse::<u32>()
    {
        return (path, Some(line));
    }
    (
        code_ref
            .split_once("::")
            .map(|(path, _)| path)
            .unwrap_or(code_ref),
        None,
    )
}

fn is_safe_relative_path(path: &str) -> bool {
    let path = Path::new(path);
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}
