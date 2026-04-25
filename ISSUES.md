# Audit Issues

Scope: pinned workspace chunk `.devcontainer/devcontainer.json..src/ui.rs` plus directly relevant docs/context.  
Workspace: Rust CLI that exposes model tools for file IO, shell execution, web fetch, search, replace, audit, and chat workflows.

## Detailed findings

### 1. Non-interactive approval bypass lets the model run commands and edit files without explicit auto-approve

- **Severity:** High
- **Category:** security
- **Status:** confirmed
- **Location:** `src/tools.rs:985`, `src/tools.rs:1137`, `src/tools.rs:1746-1753`; entry via `src/cli.rs:137-145`
- **Evidence:** `run_command` creates a non-interactive session from the default/user-selected agent and enters the model tool loop (`src/cli.rs:137-145`). Mutating tools call `require_mutation_approval`: `replace` at `src/tools.rs:985`, `bash` at `src/tools.rs:1137`. The approval guard permits mutation whenever the session is non-interactive: `if !ctx.interactive { return Ok(()); }` (`src/tools.rs:1750-1752`), regardless of `auto_approve_edits` or `auto_approve_bash`.
- **Exploitability / preconditions:** User runs `oy "prompt"` or pipes stdin in normal/default mode with provider credentials. A malicious/compromised model response, prompt injection from repo content, or hostile task text can call `bash` or `replace`; no approval is possible or enforced.
- **Impact:** Arbitrary shell execution and workspace modification under the user account, including access to inherited environment credentials. This contradicts the README safety model where only `auto-approve`/`OY_YOLO` should auto-run bash and edits (`README.md:104-119`).
- **Fix:** In non-interactive mode, deny mutating tools unless the relevant policy bit is set. Keep `auto_approved(ctx, tool)` as the only non-prompt bypass, e.g.:
  - if `auto_approved(ctx, tool)`, allow;
  - else if `!ctx.interactive`, return an error;
  - else prompt.
  Add regression tests for default non-interactive `bash`/`replace` denial and `--agent auto-approve` allowance.
- **References:** ASVS `v5.0.0-1.2.5` lookup: command execution protection; ASVS `v5.0.0-15.x` lookup: secure-by-default trust boundaries; `grugbrain: local reasoning`.

### 2. `oy audit` reviews untrusted repo content with auto-approved bash and edits

- **Severity:** Medium
- **Category:** security
- **Status:** confirmed
- **Location:** `src/cli.rs:345-354`, `src/cli.rs:377`, `src/cli.rs:388`
- **Evidence:** `audit_command` constructs a session with agent `"auto-approve"` and `config::tool_policy("auto-approve")` (`src/cli.rs:349-354`). It then feeds docs/source-derived prompts into `agent::run_prompt` for each chunk and for final reduction (`src/cli.rs:377`, `src/cli.rs:388`). Auto-approve permits both `replace` and `bash` via the tool policy (`src/tools.rs:79-83`).
- **Exploitability / preconditions:** User audits an unfamiliar or malicious repository containing prompt injection in source/docs. The audit flow passes that content to the model while exposing auto-approved `bash` and `replace`.
- **Impact:** A malicious repo can influence the audit model to execute commands or modify files during an operation users reasonably expect to read code and write only `ISSUES.md`. This is high-risk because audit is commonly run on untrusted code.
- **Fix:** Make `oy audit` read-only by default. Use a read-only/`plan` policy for chunk review and final reduction, and write `ISSUES.md` only from host code. If write/shell capability is needed, add an explicit opt-in such as `oy audit --auto-approve` with a warning.
- **References:** ASVS `v5.0.0-15.x` lookup: secure architecture and trust boundaries; `grugbrain: boring code`.

### 3. `webfetch` pre-validates DNS but does not verify the connected peer, leaving an SSRF gap

- **Severity:** Medium
- **Category:** security
- **Status:** confirmed
- **Location:** `src/tools.rs:1164-1205`, `src/tools.rs:1694-1729`
- **Evidence:** `tool_webfetch` calls `validate_public_url(&args.url).await?` before sending the request (`src/tools.rs:1168-1200`). `validate_public_url` resolves the host and rejects non-public IPs (`src/tools.rs:1694-1714`). The actual `reqwest` request then performs its own DNS/connect (`src/tools.rs:1200`). After the response, the code validates only `response.url()` (`src/tools.rs:1203-1205`), not the IP actually connected to.
- **Exploitability / preconditions:** Attacker controls a public hostname and DNS answers. It returns a public IP during preflight validation, then a private/link-local/loopback IP during reqwest’s lookup or redirect resolution.
- **Impact:** The model-exposed `webfetch` can reach internal metadata/admin services despite the documented “public-only” boundary (`README.md:119`), exposing response bodies or headers into model context.
- **Fix:** Enforce the public-IP allowlist at connection time. Options:
  - resolve once and connect only to vetted IPs while preserving safe Host/SNI behavior;
  - implement a reqwest resolver/connect policy that rejects private/link-local/loopback peers;
  - validate each redirect target before following, or keep redirects disabled and document no redirect following.
- **References:** ASVS `v5.0.0-12.x` lookup: outbound communication and SSRF controls.

## Other findings summarized

- **Low / performance:** `webfetch` and archive readers buffer unbounded bodies before truncating. `response.text().await?` (`src/tools.rs:1230`), `response.bytes().await?` (`src/tools.rs:1253`), gzip `read_to_end` (`src/tools.rs:1538`), zip member `read_to_end` (`src/tools.rs:1568`), and tar entry `read_to_end` (`src/tools.rs:1602`) can cause memory exhaustion or long stalls on large responses, decompression bombs, or large archive members. Fix with content-length checks, streaming caps, decompressed byte limits, and explicit `too_large/truncated` results. Reference: `grugbrain: small sharp tools`.

No additional high-confidence security, complexity, or material performance findings were retained from the collected draft.
