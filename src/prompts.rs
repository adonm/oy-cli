use std::fmt::Write as _;

pub const AUDIT_REPORT_TITLE: &str = "# Audit Issues";
pub const AUDIT_TRANSPARENCY_PREFIX: &str =
    "Generated with [oy-cli](https://github.com/wagov-dtt/oy-cli):";

pub const AUDIT_SYSTEM_PROMPT: &str = r#"You are oy in audit mode. Audit the repository for security issues, unnecessary complexity, and material usability or performance problems.
Be terse, evidence-first, and repo-specific. Avoid generic best-practice advice, style nits, and speculation.

Finding quality bar:
- Report only concrete issues with a plausible attack path, trigger, broken invariant, data exposure, integrity risk, privilege impact, or material operational impact.
- For vulnerabilities, include the trust boundary, sink, affected path/symbol evidence, impact, exploitability/preconditions, and a concrete fix.
- Prefer critical/high security findings and issues likely to cause production incidents.
- Prefer simple remediations that remove whole bug classes.
- Return [] or say no concrete findings for a chunk when evidence is weak.

Use the embedded OWASP/grugbrain reference as a lightweight checklist and citation guide. Spend tokens on repository evidence, not long standards explanations."#;

pub const AUDIT_REFERENCE: &str = r#"Audit reference checklist:

OWASP ASVS 5.0 quick map:
- V1 Architecture: trust boundaries, secure design, attack surface, threat model gaps, dangerous defaults.
- V2 Authentication: credential handling, MFA, session/auth lifecycle, account recovery.
- V3 Session: cookie/token handling, fixation, expiration, revocation, CSRF-relevant state.
- V4 Access Control: object/function authorization, tenant isolation, confused deputy paths.
- V5 Validation: parser boundaries, canonicalization, path traversal, SSRF, injection, deserialization.
- V6 Cryptography: key management, weak/custom crypto, randomness, secret storage.
- V7 Error/Logging: secret leakage, unsafe diagnostics, audit trail gaps.
- V8 Data Protection: sensitive data at rest/in transit, retention, cache/backup exposure.
- V9 Communications: TLS verification, hostname validation, downgrade/debug transport.
- V10 Malicious Code: supply chain, unsafe dynamic loading, dependency/update risk.
- V11 Business Logic: state-machine bypass, race/double-submit, workflow abuse.
- V12 Files/Resources: upload/download, archive extraction, filesystem boundaries, quotas.
- V13 API/Web Service: mass assignment, schema validation, rate limits, authz on APIs.
- V14 Configuration: insecure defaults, debug flags, secret/config sprawl.

OWASP MASVS/MASWE for mobile repos only:
- STORAGE, CRYPTO, AUTH, NETWORK, PLATFORM, CODE, RESILIENCE, PRIVACY; use MASWE IDs only when a concrete mobile weakness maps cleanly.

Grugbrain complexity filter:
- Grugbrain has no formal section IDs; do not invent citations. Use exact lookup phrases only.
- Useful phrases: `complexity very bad`, `local reasoning`, `small sharp tools`, `avoid wrong abstraction`, `too much abstraction`, `closures like salt`, `reproduce bug first`, `testing`.
- Use grugbrain for complexity/maintainability findings, or as secondary support where complexity materially increases exploitability or review failure risk.

Combined heuristic:
- Security bug plus high complexity is higher priority because it is harder to review, fix safely, and prevent from recurring.
- Prefer findings where code both violates a security control and hides that violation behind abstraction, config sprawl, hidden state, or broad capability.
- If a simpler design removes an entire bug class, say so explicitly."#;

pub fn audit_chunk_prompt(
    focus: &str,
    manifest: &str,
    index: &str,
    chunk_id: usize,
    chunk_count: usize,
    chunk_text: &str,
) -> String {
    let mut prompt = String::new();
    let _ = writeln!(prompt, "Review audit chunk {chunk_id}/{chunk_count}.");
    push_focus(&mut prompt, focus);
    prompt.push_str("\nReturn concise candidate findings for this chunk only. Use markdown with one `###` heading per finding, or return `[]` if there are no concrete findings. For each finding include severity, category, evidence path/symbol, trust boundary/sink when security-relevant, impact, reference, and fix. Do not write files.\n\n");
    prompt.push_str("Repository manifest:\n");
    prompt.push_str(manifest.trim());
    prompt.push_str("\n\nSecurity-relevant index:\n");
    prompt.push_str(index.trim());
    prompt.push_str("\n\nChunk contents:\n");
    prompt.push_str(chunk_text.trim());
    prompt
}

pub fn audit_full_prompt(focus: &str, manifest: &str, index: &str, repo_text: &str) -> String {
    let mut prompt = String::new();
    prompt.push_str("Conduct a full repository audit and return the final markdown report.\n");
    push_focus(&mut prompt, focus);
    prompt.push_str("\nReport format: start with `# Audit Issues`; keep the most important concrete findings detailed; include severity, category, evidence, impact, exploitability/preconditions where relevant, reference, and fix. Avoid generic advice. Do not write files.\n\n");
    prompt.push_str("Repository manifest:\n");
    prompt.push_str(manifest.trim());
    prompt.push_str("\n\nSecurity-relevant index:\n");
    prompt.push_str(index.trim());
    prompt.push_str("\n\nRepository contents:\n");
    prompt.push_str(repo_text.trim());
    prompt
}

pub fn audit_reduce_prompt(focus: &str, manifest: &str, findings: &str) -> String {
    let mut prompt = String::new();
    prompt.push_str("Condense candidate audit findings into the final markdown report.\n");
    push_focus(&mut prompt, focus);
    prompt.push_str("\nStart with `# Audit Issues`. Dedupe overlapping findings, rank by severity/exploitability/impact, keep the strongest 10-15 issues detailed, and condense or drop weak/duplicate items. Preserve the shortest evidence needed to prove exploitability or impact.\n\n");
    prompt.push_str("Repository manifest:\n");
    prompt.push_str(manifest.trim());
    prompt.push_str("\n\nCandidate findings:\n");
    prompt.push_str(findings.trim());
    prompt
}

fn push_focus(out: &mut String, focus: &str) {
    let focus = focus.trim();
    if !focus.is_empty() {
        let _ = writeln!(out, "Additional focus: {focus}");
    }
}

pub fn audit_system_prompt() -> String {
    format!(
        "{}\n\n{}",
        AUDIT_SYSTEM_PROMPT.trim(),
        AUDIT_REFERENCE.trim()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_system_prompt_embeds_owasp_and_grugbrain_reference() {
        let prompt = audit_system_prompt();
        assert!(prompt.contains("OWASP ASVS 5.0"));
        assert!(prompt.contains("Grugbrain"));
        assert!(prompt.contains("complexity very bad"));
        assert!(prompt.contains("trust boundary"));
    }

    #[test]
    fn audit_prompts_include_focus_when_present() {
        let prompt = audit_full_prompt("auth paths", "files: 1", "- hit", "src/lib.rs");
        assert!(prompt.contains("Additional focus: auth paths"));
        assert!(prompt.contains("# Audit Issues"));
    }
}
