use crate::compaction;

use super::{REDUCE_FINDINGS_MIN_TOKENS, REDUCE_FINDINGS_TOKEN_RESERVE, prompts};

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

pub(super) fn reduce_candidate_findings_budget(
    model_spec: &str,
    focus: &str,
    manifest: &str,
    max_prompt_tokens: usize,
) -> usize {
    let prompt_without_findings = prompts::audit_reduce_prompt(focus, manifest, "");
    let overhead_tokens = compaction::count_tokens(model_spec, &prompt_without_findings);
    max_prompt_tokens
        .saturating_sub(overhead_tokens)
        .saturating_sub(REDUCE_FINDINGS_TOKEN_RESERVE)
        .max(REDUCE_FINDINGS_MIN_TOKENS)
}

pub(super) fn bounded_reduce_findings(
    model_spec: &str,
    focus: &str,
    manifest: &str,
    findings: &str,
    max_prompt_tokens: usize,
) -> String {
    let prompt_tokens = |findings: &str| {
        let prompt = prompts::audit_reduce_prompt(focus, manifest, findings);
        compaction::count_tokens(model_spec, &prompt)
    };
    if prompt_tokens(findings) <= max_prompt_tokens {
        return findings.to_string();
    }

    let findings_budget =
        reduce_candidate_findings_budget(model_spec, focus, manifest, max_prompt_tokens);
    let mut current_budget = findings_budget;
    let mut bounded = compact_owned_to_tokens(model_spec, findings, current_budget);

    while prompt_tokens(&bounded) > max_prompt_tokens && current_budget > REDUCE_FINDINGS_MIN_TOKENS
    {
        current_budget = (current_budget.saturating_mul(3) / 4).max(REDUCE_FINDINGS_MIN_TOKENS);
        bounded = compact_owned_to_tokens(model_spec, findings, current_budget);
    }

    bounded
}
