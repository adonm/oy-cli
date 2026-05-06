# Audit Issues

> Generated with [oy-cli](https://github.com/wagov-dtt/oy-cli): `OY_MODEL=opencode-go/deepseek-v4-pro oy audit` · 2026-05-06

## Findings summary

- **High** `src/agent/model.rs::resolve_chat_route` / `src/cli/chat.rs:159` — API key leakage through error reporting
- **High** `src/cli/ui/render.rs::render_markdown`, `paint` — Terminal ANSI escape injection from unsanitised model output
- **Medium** `src/tools/network.rs::is_public_ipv4` — Missing IPv4 special‑purpose address range allows non‑public webfetch
- **Medium** `src/cli/chat/commands.rs:111-114` — Unsafe, non‑thread‑safe process environment mutation
- **Medium** `src/tools/network.rs::is_public_ipv4` – manual IP classification with high audit burden (complexity)
- **Low**  `src/tools/shell.rs::tool_bash` — Shell commands inherit full environment including secrets

## Detailed findings

### High: API key leakage through error reporting

- **Category**: V7 Error Handling & Logging  
- **Evidence**  
  - `src/agent/model.rs::resolve_chat_route` loads API keys from environment or files and passes them to the Rig client builder.  
  - `src/agent/model.rs::execute_chat_route` calls `agent.prompt(…)` and propagates errors via `anyhow`.  
  - `src/cli/chat.rs:159` prints the full error chain with `{…:#}`:  
    `crate::ui::err_line(format_args!("model call failed: {err:#}"));`  
    When the HTTP client includes credential headers in error details (e.g. a `reqwest` failure), the API key appears in the terminal.  
- **Trust boundary / sink**  
  Authentication secrets (OpenAI, Copilot, OpenCode keys) cross from environment into the provider client. The error path writes them to stderr.  
- **Impact**  
  An attacker who can trigger an authentication‑related chat failure (e.g. by choosing an invalid provider/model combination) may see the raw API key in the output. The key may also persist in shell history or log capture.  
- **Exploitability / preconditions**  
  The attacker needs the ability to influence the model spec or prompt such that the resulting API call fails. In a multi‑user or shared environment this could expose credentials to other users.  
- **Reference**  
  OWASP ASVS V7.3 – “Verify that error handling logic in security controls denies access by default and does not disclose sensitive information.”  
- **Fix**  
  Wrap the rig client or the `exec_chat` result in an error type that redacts `Authorization` and other sensitive headers before the error is surfaced. Avoid printing `{:#}` for errors that may contain secrets; use a safe error representation.

---

### High: Terminal ANSI escape injection from unsanitised model output

- **Category**: V5 Validation (output encoding)  
- **Evidence**  
  - `src/cli/ui/render.rs::render_markdown` processes model‑generated text line‑by‑line and adds ANSI colour codes via the `paint` helper.  
  - The raw model text is never scanned or escaped for existing control sequences.  
  - `src/cli/ui.rs::paint` wraps text with `\x1b[code m … \x1b[0m`, but if the input already contains `\x1b` sequences those will be passed through.  
- **Trust boundary / sink**  
  Model output (untrusted) is written directly to the terminal. A prompt‑injection attack or a malicious model response can embed ANSI escape sequences that reposition the cursor, clear the screen, or trigger terminal‑specific command execution (e.g. OSC 52 clipboard injection).  
- **Impact**  
  Terminal manipulation can hide subsequent output, trick the user into approving dangerous actions, or, on vulnerable terminals, achieve arbitrary command execution.  
- **Exploitability / preconditions**  
  Any session where the model generates output. An attacker only needs to inject specially crafted text into a prompt or to poison the model’s training data.  
- **Reference**  
  CWE-150 (Improper Neutralization of Escape, Meta, or Control Sequences). OWASP ASVS V5.3.4 – “Verify that output encoding is applied to prevent … terminal injection attacks.”  
- **Fix**  
  Strip or escape the `\x1b` (ESC) character from every line of model output before adding colour codes. Use a library that sanitises terminal output, or simply replace `\x1b` with a visible placeholder.

---

### Medium: Missing IPv4 special‑purpose address range in webfetch public‑IP filter

- **Category**: V5 Validation (SSRF / network boundary)  
- **Evidence**  
  - `src/tools/network.rs::is_public_ipv4` performs manual range checks but does not cover `192.0.0.0/24` (IETF Protocol Assignments).  
  - Other special‑purpose blocks (e.g. `0.0.0.0/8`, `100.64.0.0/10`, `198.18.0.0/15`) are handled, but the function relies on a custom list rather than an exhaustive source.  
  - `tool_webfetch` calls `public_socket_addrs`, which calls `is_public_ip` on each resolved address. An address in `192.0.0.0/24` would be classified as public, bypassing the restriction.  
- **Trust boundary / sink**  
  Network boundary: the model is allowed to fetch only from **public** IPs. A non‑public address in the `192.0.0.0/24` block can be used to reach internal services (e.g. DNS‑SD proxies).  
- **Impact**  
  An attacker‑controlled URL that resolves to `192.0.0.1` (or similar) could cause the agent to interact with local network services, potentially exfiltrating data or triggering side‑effects.  
- **Exploitability / preconditions**  
  The model must be persuaded to fetch a URL with a hostname or IP in the missing range. Likelihood is low but not zero, especially on networks where such addresses are reachable.  
- **Reference**  
  OWASP ASVS V5.2.6 – “Verify that the application protects against Server‑Side Request Forgery (SSRF) attacks by validating all internal or remote requests.”  
- **Fix**  
  Replace the manual IPv4 classification with a well‑tested library (e.g. `ipnet`) that supports the full IANA special‑purpose address registry. At minimum, add the `192.0.0.0/24` block and any other missing reserved ranges (e.g. `240.0.0.0/4` is already covered, but verify).

---

### Medium: Unsafe, non‑thread‑safe process environment mutation in `/thinking` command

- **Category**: V14 Configuration (unsafe defaults) / Code Correctness  
- **Evidence**  
  - `src/cli/chat/commands.rs:111-114` uses `unsafe { std::env::set_var("OY_THINKING", …) }` and `remove_var` inside an `async` context.  
  - Rust’s environment functions are not thread‑safe; concurrent access (e.g. Tokio tasks) can cause data races and undefined behaviour.  
- **Trust boundary / sink**  
  No direct security boundary, but the environment is process‑global; improper synchronisation can corrupt the environment used by other parts of the system (e.g. spawned subprocesses).  
- **Impact**  
  Subtle bugs, environment inconsistencies, or even crashes when multiple tasks race on the same environment variable. While currently low risk in a single‑user CLI, it introduces undefined behaviour that is hard to diagnose.  
- **Exploitability / preconditions**  
  Requires calling `/thinking` while other parts of the application (or Tokio runtime tasks) read `OY_THINKING`.  
- **Reference**  
  Rust `std::env::set_var` safety documentation.  
- **Fix**  
  Replace direct environment manipulation with a synchronised configuration store (e.g. `OnceCell`/`LazyLock`/`RwLock<HashMap>`). Read the setting from that store in the model routing code instead of depending on the environment after initialisation.

---

### Medium (Complexity): Manual IPv4 range classification in webfetch is fragile and hard to audit

- **Category**: Implementation quality (grugbrain: `local reasoning` deficit)  
- **Evidence**  
  `src/tools/network.rs::is_public_ipv4` contains ~20 raw octet comparisons that re‑implement public‑/private‑address classification. The logic duplicates what standard library functions (`ip.is_private()`, `ip.is_loopback()`, etc.) provide, but omits some blocks and requires careful line‑by‑line review to ensure completeness.  
- **Impact**  
  Increased chance of missing reserved ranges (as noted in the finding above). Future maintainers are likely to introduce regressions.  
- **Reference**  
  grugbrain: “complexity very bad”, “local reasoning”.  
- **Fix**  
  Delegate IP classification to a dedicated, tested library (`ipnet`, `cidr-utils`). Remove the custom `is_public_ipv4` and rely on a single source of truth.

---

### Low: Shell commands inherit full process environment, exposing secrets

- **Category**: V2 Authentication / V8 Data Protection (operational risk)  
- **Evidence**  
  `src/tools/shell.rs::tool_bash` spawns `bash -c` with `Stdio::null()` for stdin but **does not sanitise the environment**; it inherits the parent process’s entire environment, including `OPENAI_API_KEY`, `COPILOT_GITHUB_TOKEN`, etc.  
- **Trust boundary / sink**  
  The model can request arbitrary shell commands. If the model is compromised or the user instructs it to run a command that logs environment variables (e.g. `env`), secrets will be exposed.  
- **Impact**  
  Accidental credential leakage to the terminal or to files written by the shell.  
- **Exploitability / preconditions**  
  Requires `bash` tool to be enabled (non‑plan mode) and a prompt that causes the model to expose the environment.  
- **Reference**  
  OWASP ASVS V8.1.1 – “Verify that the application protects sensitive data from being … inadvertently exposed during processing.”  
- **Fix**  
  For the `bash` tool, provide a configuration option to clear or filter environment variables before execution. Set a safe default that removes known credential variables, and document the behaviour.

---
