# Audit Issues

> Generated with [oy-cli](https://github.com/wagov-dtt/oy-cli): `OY_MODEL=opencode-go/deepseek-v4-pro oy audit` · 2026-05-09

## Findings summary

Current triage after review:

- **Fixed** `src/tools/network.rs::is_public_ip` — `webfetch` public-address classification now delegates to `ip_rfc` global-address semantics, with explicit denials for multicast and deprecated IPv6 site-local ranges.
- **Context / no active finding** `src/cli/ui/progress.rs`, `src/cli/ui/render.rs`, `src/tools/preview.rs::preview_bash` — output safety is split by sink: direct progress/error metadata escapes terminal controls, while raw content previews intentionally go through bat/terminal rendering to preserve ANSI output. Do not chase blanket escaping here unless a new direct, non-bat content sink is added.
- **Fixed** `src/tools/shell.rs::tool_bash` — shell child processes remove credential-like environment variables by default; remaining risk is the normal approved-shell trust boundary.

## Highest-ROI fix

The highest-ROI item from this review has been fixed. Next best follow-up is to keep the `webfetch` network boundary regression tests broad whenever URL resolution, redirects, or IP classification changes. The terminal/output item is mostly policy/tooling context now, and shell env filtering is already covered.

## Detailed findings

### Fixed: `webfetch` public-IP classification uses maintained global-address semantics

- **Status**: Fixed in `Unreleased` — `is_public_ip` now uses `ip_rfc::global` plus boundary-specific denials.
- **Category**: V5 Input Validation / SSRF boundary maintainability
- **Evidence**
  - `src/tools/network.rs::is_public_ip` delegates global-address classification to `ip_rfc::global`.
  - `src/tools/network.rs` keeps explicit public-fetch denials for multicast and deprecated IPv6 site-local ranges, because those are not valid public document-fetch targets for this boundary.
  - Regression tests cover private, shared, loopback, link-local, documentation, benchmarking, protocol-assignment, multicast, reserved, deprecated site-local, and representative public ranges.
- **Trust boundary / sink**
  Model-supplied `webfetch` URL / DNS result → public-only network policy → outbound HTTP request.
- **Residual risk**
  DNS rebinding and redirect behavior should remain covered when URL-fetch logic changes. The current client disables automatic redirects and validates all resolved addresses before the request.

---

### Context: Terminal/output escape safety is sink-specific, not a blanket finding

- **Status**: Context only — not tracked as an active finding unless a new direct terminal sink prints untrusted metadata/content without escaping or bat rendering.
- **Category**: V5 Validation (output encoding) / CWE-150
- **Current shape**
  - Direct progress/error metadata escapes ESC before writing to the terminal (`src/cli/ui/progress.rs:57-94`, `src/cli/ui.rs:157-160`). This covers model-supplied tool names, tool-call summaries, progress detail, and errors.
  - Content previews are intentionally different: markdown, diffs, code/text blocks, and verbose `bash` output are rendered through bat-backed paths where ANSI/terminal bytes are preserved for formatting (`src/cli/ui/render.rs:102-121`, `src/tools/preview.rs:505-559`, `src/tools/tests.rs:971-991`).
  - `docs/tool-safety.md` documents that `bash` stdout/stderr terminal/control sequences pass through raw for bat/terminal formatting.
- **Practical rule**
  Escape untrusted metadata immediately before direct `line`/`err_line` terminal sinks. Do not pre-escape raw content that is deliberately handed to bat/terminal rendering, or it will replace useful formatted output with visible escape glyphs.
- **When to reopen**
  Reopen only if untrusted content bypasses bat-backed rendering and is written directly to the terminal, or if a new metadata path reaches `line`/`err_line` without `escape_terminal_controls`.

---

### Fixed: Shell child processes remove credential-like environment variables

- **Status**: Fixed in `Unreleased` — `bash` child processes now remove credential-like environment variables before launch, with regression coverage and updated security docs.
- **Category**: V8 Data Protection / Configuration
- **Evidence**
  - `src/tools/shell.rs:48-57` calls `remove_sensitive_child_env` before spawning `bash`.
  - `src/tools/shell.rs:112-138` removes env vars whose names look credential-bearing (`API_KEY`, `SECRET`, `TOKEN`, `PASSWORD`, `AUTH`, etc.).
  - `src/tools/tests.rs:994` covers that secret-like env vars are removed while non-secret env vars remain visible.
- **Trust boundary / sink**
  Approved model/user shell command → child process environment → stdout/stderr/transcript.
- **Impact**
  The original easy leak path (`env` dumping provider tokens inherited from `oy`) is mitigated by default. Shell remains powerful and should stay ask/deny by mode; this is documented in `docs/tool-safety.md`.
- **Follow-up only if needed**
  Add an explicit allow/deny env configuration only if users need reproducible shell environments. Otherwise avoid adding another configuration surface.
