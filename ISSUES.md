# Audit Issues

> Generated with [oy-cli](https://github.com/wagov-dtt/oy-cli): `OY_MODEL=opencode-go/deepseek-v4-pro oy audit` · 2026-05-09

## Findings summary

No unresolved findings are currently tracked from this audit.

Fixed in `Unreleased`:

- **Medium** `src/tools/output.rs::note_tool` / `src/cli/ui/progress.rs::tool_start`, `tool_result` — Terminal ANSI escape injection via unsanitised tool call previews and tool output
- **Medium** `src/tools/network.rs::is_public_ip` — Manual IPv4 classification remained fragile and hard to audit
- **Low**  `src/tools/shell.rs::tool_bash` — Shell commands inherited credential-like environment variables

## Detailed findings

### Medium: Terminal ANSI escape injection via unsanitised tool call previews and tool output

- **Status**: Superseded in `Unreleased` — direct progress/error metadata sinks escape terminal controls, but bat-backed output/content paths preserve raw ANSI/terminal bytes so `bash`, markdown, and diff output can be formatted by bat/the terminal.
- **Category**: V5 Validation (output encoding) / CWE-150
- **Evidence**
  - `src/tools/output.rs::note_tool` builds a summary string from model-supplied tool arguments and passes it to `crate::ui::tool_start`, which writes to stderr via `err_line()` without escape-filtering.
  - `src/cli/ui/progress.rs::tool_result` and `tool_error` similarly emit preview text and error messages to `err_line()` without sanitisation.
  - Earlier escaping was scattered across render paths and direct terminal sinks, which made it unclear which bytes were content and which bytes were terminal-control metadata.
  - Attacker-controlled strings appear early in the pipeline: the tool call summary is displayed *before* any approval.
- **Trust boundary / sink**
  Model → tool call arguments / tool output → `err_line()` → terminal.
- **Impact**
  A malicious model (or a prompt injection) can embed ANSI escape sequences in a tool’s arguments or in the output of a tool (e.g. `bash` stdout/stderr). These are printed to the user’s terminal without filtering, allowing screen-clearing, cursor manipulation, or terminal-specific command injection (OSC 52 clipboard write, etc.). The user could be deceived into approving dangerous actions or lose visibility of true agent activity.
- **Exploitability / preconditions**
  The model needs to invoke any tool that accepts text arguments (e.g. `bash`, `search`, `replace`) or produce output that contains escape sequences. No user approval is required for the tool-start message; the injection is executed as soon as the tool call is processed. The attack works even in modes where the tool itself is denied, because the summary is printed before the policy check.
- **Reference**
  OWASP ASVS V5.3.4 – “Verify that output encoding is applied to prevent … terminal injection attacks.” CWE-150.
- **Fix**
  Escape untrusted metadata immediately before it enters direct terminal sinks (`err_line`/`line`) while preserving raw content sent through bat-backed output paths. Keep terminal-control escaping at direct progress/error/title metadata boundaries rather than pre-processing `bash`, markdown, diff, or bat input.

---

### Medium: Manual IPv4 classification remains fragile and hard to audit

- **Status**: Fixed in `Unreleased` — `webfetch` now delegates global address classification to `ip_rfc`, keeps explicit multicast/site-local denials for this public-fetch boundary, and has expanded regression coverage.
- **Category**: Implementation quality / security maintainability
- **Original evidence**
  - `src/tools/network.rs::is_public_ipv4` used a custom combination of standard library methods and manual octet comparisons to decide whether an address was public.
  - While the specific `192.0.0.0/24` omission was fixed (v0.8.7), the function remained a hand-rolled list that had to be revised whenever the IANA registry changed.
  - Auditors and maintainers had to manually verify the complete block set.
- **Impact**
  Future oversight when adding or removing reserved ranges could reintroduce an SSRF bypass (public-only webfetch restriction).
- **Exploitability / preconditions**
  Requires a new special-purpose range to be assigned that is not caught by the current code (currently low probability, but the risk grows over time).
- **Reference**
  OWASP ASVS V5.2.6; grugbrain “local reasoning” deficit.
- **Fix**
  Replace the manual classification with an authoritative, maintained IP classification library (e.g. `ipnet` + the official IANA registry, or a crate that provides `is_global` semantics). This removes the need for future manual updates.

---

### Low: Shell commands inherit full process environment exposing secrets

- **Status**: Fixed in `Unreleased` — `bash` child processes now remove credential-like environment variables before launch, with regression coverage and updated security docs.
- **Category**: V8 Data Protection / Configuration
- **Original evidence**
  - `src/tools/shell.rs::tool_bash` spawned `bash -c` with `Stdio::null()` for stdin but did not filter the subprocess environment; it inherited the parent environment, including `OPENAI_API_KEY`, `COPILOT_GITHUB_TOKEN`, etc.
- **Trust boundary / sink**
  The model can request arbitrary shell commands. A crafted command (e.g. `env`) will dump all environment variables to stdout/stderr, exposing secrets to the transcript and terminal.
- **Impact**
  Accidental credential leakage to the terminal, log files, or saved sessions.
- **Exploitability / preconditions**
  The `bash` tool must be enabled (non‑plan mode). The model could be tricked into running `env` or the user may not notice the exposure.
- **Reference**
  OWASP ASVS V8.1.1 – “Verify that the application protects sensitive data from being … inadvertently exposed during processing.”
- **Fix**
  Provide a configuration option (or environment variable) to clear or selectively filter environment variables before shell execution. Set a safe default that removes known credential variables. Document the risk explicitly.
