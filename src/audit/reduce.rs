use std::fmt::Write as _;

use crate::compaction;

use super::prompts;

pub(super) fn compact_to_tokens<'a>(
    model_spec: &str,
    text: &'a str,
    max_tokens: usize,
) -> std::borrow::Cow<'a, str> {
    if compaction::count_tokens(model_spec, text) <= max_tokens {
        return std::borrow::Cow::Borrowed(text);
    }
    std::borrow::Cow::Owned(compact_owned_to_tokens(model_spec, text, max_tokens))
}

fn compact_owned_to_tokens(model_spec: &str, text: &str, max_tokens: usize) -> String {
    let findings = split_candidate_findings(text);
    if findings.len() <= 1 {
        return compact_head_tail_to_tokens(model_spec, text, max_tokens);
    }

    let mut per_finding_chars = max_tokens.saturating_mul(4) / findings.len().max(1);
    per_finding_chars = per_finding_chars.clamp(80, 4000);
    loop {
        let compact = compact_findings_by_section(&findings, per_finding_chars);
        if compaction::count_tokens(model_spec, &compact) <= max_tokens {
            return compact;
        }
        if per_finding_chars <= 80 {
            return compact_head_tail_to_tokens(model_spec, &compact, max_tokens);
        }
        per_finding_chars = (per_finding_chars.saturating_mul(3) / 4).max(80);
    }
}

fn compact_head_tail_to_tokens(model_spec: &str, text: &str, max_tokens: usize) -> String {
    let mut max_chars = max_tokens.saturating_mul(4).max(2000);
    loop {
        let (short, truncated) = crate::ui::head_tail(text, max_chars);
        if !truncated
            || compaction::count_tokens(model_spec, &short) <= max_tokens
            || max_chars <= 512
        {
            return short;
        }
        max_chars = max_chars.saturating_mul(3) / 4;
    }
}

fn split_candidate_findings(text: &str) -> Vec<String> {
    let mut sections = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        let is_boundary = line.starts_with("## Candidate findings from chunk ")
            || line.starts_with("### ")
            || line.starts_with("#### ");
        if is_boundary && !current.trim().is_empty() {
            sections.push(current.trim().to_string());
            current.clear();
        }
        let _ = writeln!(current, "{line}");
    }
    if !current.trim().is_empty() {
        sections.push(current.trim().to_string());
    }
    sections
}

fn compact_findings_by_section(findings: &[String], per_finding_chars: usize) -> String {
    let mut out = String::new();
    for finding in findings {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        if finding.chars().count() <= per_finding_chars {
            out.push_str(finding);
            continue;
        }
        let mut lines = finding.lines();
        let heading = lines.next().unwrap_or_default();
        let body = lines.collect::<Vec<_>>().join("\n");
        let body_budget = per_finding_chars.saturating_sub(heading.chars().count() + 64);
        let preview = crate::ui::truncate_chars(&body.replace('\n', " "), body_budget.max(80));
        let _ = write!(out, "{heading}\n… details compacted: {preview}");
    }
    out
}

pub(super) fn reduce_candidate_findings_budget(
    model_spec: &str,
    focus: &str,
    manifest: &str,
    existing_issues: Option<&str>,
    max_prompt_tokens: usize,
    min_tokens: usize,
    reserve_tokens: usize,
) -> usize {
    let prompt_without_findings = prompts::audit_reduce_prompt(focus, manifest, "", existing_issues);
    let overhead_tokens = compaction::count_tokens(model_spec, &prompt_without_findings);
    max_prompt_tokens
        .saturating_sub(overhead_tokens)
        .saturating_sub(reserve_tokens)
        .max(min_tokens)
}

pub(super) fn bounded_reduce_findings(
    model_spec: &str,
    focus: &str,
    manifest: &str,
    findings: &str,
    existing_issues: Option<&str>,
    max_prompt_tokens: usize,
    min_tokens: usize,
    reserve_tokens: usize,
) -> String {
    let prompt_tokens = |findings: &str| {
        let prompt = prompts::audit_reduce_prompt(focus, manifest, findings, existing_issues);
        compaction::count_tokens(model_spec, &prompt)
    };
    if prompt_tokens(findings) <= max_prompt_tokens {
        return findings.to_string();
    }

    let findings_budget = reduce_candidate_findings_budget(
        model_spec,
        focus,
        manifest,
        existing_issues,
        max_prompt_tokens,
        min_tokens,
        reserve_tokens,
    );
    let mut current_budget = findings_budget;
    let mut bounded = compact_owned_to_tokens(model_spec, findings, current_budget);

    while prompt_tokens(&bounded) > max_prompt_tokens && current_budget > min_tokens {
        current_budget = (current_budget.saturating_mul(3) / 4).max(min_tokens);
        bounded = compact_owned_to_tokens(model_spec, findings, current_budget);
    }

    bounded
}
