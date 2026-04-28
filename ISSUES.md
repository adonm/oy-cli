# Audit Issues

> Generated with [oy-cli](https://github.com/wagov-dtt/oy-cli): `OY_MODEL=openai_resp::gpt-5.5 oy audit` · 2026-04-28

This is a point-in-time security audit of the Rust CLI. Each finding names a trust boundary, the relevant sink, and the expected impact so fixes can be reviewed without re-running the audit. Fixed findings stay listed with **Status: Fixed** until the next full audit refresh to preserve remediation history.

## Findings summary

| Severity | Finding | Code reference |
|---|---|---|
| High | Read-only mode permits workspace-read + arbitrary public egress, enabling secret exfiltration via `webfetch` | `src/tools.rs::ToolPolicy::read_only`, `src/tools.rs::tool_enabled`, `src/tools.rs::tool_webfetch` |
| Fixed | Bedrock Mantle model discovery could leak `OPENAI_API_KEY` to a non-OpenAI endpoint | `src/agent.rs::model::{inspect_models, openai_compatible_endpoints, shim_endpoint_config, fetch_openai_compatible_models}` |
| High | Workspace output path containment can be bypassed through symlink ancestors | `src/cli.rs::config::{resolve_workspace_output_path, write_workspace_file}` |
| High | `webfetch` public-IP validation misses non-public and IPv4-mapped ranges | `src/tools.rs::{validate_public_url_parts, public_socket_addrs, ensure_public_ip, is_public_ip}` |
| Medium | Audit mode can send hidden and likely-secret workspace files to the model provider | `src/audit.rs::{collect_files, should_skip_path, chunk_text, audit_full_prompt, audit_chunk_prompt}` |
| Medium | Passive auth detection executes PATH-resolved `gh`/`aws` during no-tools/read-only setup flows | `src/agent.rs::{model::gh_auth_token, bedrock::aws_cli_available}`, `src/cli.rs::app::{doctor_command, model_command}` |
| Fixed | OpenAI credentials/base URL could be applied to non-OpenAI model requests | `src/agent.rs::model::{auth_resolver, service_target_resolver}` |
| Medium | File tools enforce limits after unbounded reads/search accumulation | `src/tools.rs::{read_text_file, tool_read, tool_search, search_text_grep, tool_replace}` |

## Detailed findings

### 1. Read-only mode permits workspace-read + arbitrary public egress

- **Severity:** High
- **Category:** Data exfiltration / confused deputy
- **Reference:** `src/tools.rs::ToolPolicy::read_only`, `src/tools.rs::tool_enabled`, `src/tools.rs::tool_webfetch`
- **Evidence:**
  - `ToolPolicy::read_only` sets `network: true`.
  - `tool_enabled` exposes always-on read tools such as `list`, `read`, `search`, `sloc`, `todo`, and also enables `ToolGate::Network` tools when `ctx.policy.network` is true.
  - The read-only test asserts `webfetch` is available in read-only mode.
  - `tool_webfetch` sends the model-supplied request via `client.request(...).send().await?` without a separate approval step.
- **Trust boundary / sink:** Untrusted repository text or model-generated tool arguments → workspace file reads → outbound HTTP request to model-chosen public URL.
- **Impact:** A malicious repository prompt can instruct the model to read workspace secrets such as `.env`, tokens, or config files and exfiltrate them in a `webfetch` URL/query while the user believes they are in read-only/plan mode. No shell or file-write approval is needed.
- **Exploitability / preconditions:** User runs read-only/plan mode on a workspace containing secrets; model follows repo-controlled instructions.
- **Fix:** Default read-only/untrusted modes to `network: false`. Require explicit `--allow-network` or per-request approval before enabling `webfetch`. Keep workspace-read and public-network capabilities separate.

---

### 2. Bedrock Mantle model discovery could leak `OPENAI_API_KEY` to a non-OpenAI endpoint

- **Severity:** High
- **Status:** Fixed in `src/agent.rs`: Bedrock Mantle no longer accepts `OPENAI_API_KEY` or `OPENAI_BASE_URL`; it requires `BEDROCK_MANTLE_API_KEY` or `AWS_BEARER_TOKEN_BEDROCK`.
- **Category:** Credential confusion / secret disclosure
- **Reference:** `src/agent.rs::model::{inspect_models, openai_compatible_endpoints, shim_endpoint_config, fetch_openai_compatible_models}`
- **Evidence:**
  - `openai_compatible_endpoints()` probes every shim in `SHIM_ORDER`, including `SHIM_BEDROCK_MANTLE`.
  - `shim_endpoint_config(SHIM_BEDROCK_MANTLE)` accepts `OPENAI_API_KEY` as a credential source.
  - Bedrock Mantle’s default target is `https://bedrock-mantle.{region}.api.aws/v1`.
  - `fetch_openai_compatible_models()` sends `.bearer_auth(&endpoint.api_key)` to `{base_url}/models`.
- **Trust boundary / sink:** Local OpenAI credential environment variable → outbound Bearer token HTTP request to Bedrock Mantle or configured base URL.
- **Impact:** Running `oy model` or `oy doctor` with only `OPENAI_API_KEY` configured can disclose the OpenAI API key to Bedrock Mantle without the user explicitly selecting that shim. If `BEDROCK_MANTLE_BASE_URL` or `OPENAI_BASE_URL` is attacker-controlled/misconfigured, the key can be sent there.
- **Exploitability / preconditions:** User has `OPENAI_API_KEY` set and invokes model discovery/doctor; Bedrock Mantle probing is enabled by default.
- **Fix:** Do not use `OPENAI_API_KEY` as an implicit Bedrock Mantle credential. Require `BEDROCK_MANTLE_API_KEY` or `AWS_BEARER_TOKEN_BEDROCK`, and only probe Bedrock Mantle when explicitly selected/configured.

---

### 3. Workspace output path containment can be bypassed through symlink ancestors

- **Severity:** High
- **Category:** Path traversal / filesystem boundary escape
- **Reference:** `src/cli.rs::config::{resolve_workspace_output_path, write_workspace_file}`
- **Evidence:**
  - `resolve_workspace_output_path()` rejects absolute paths and `..`, but does not reject Windows `Component::Prefix`.
  - It canonicalizes `parent` only when `parent.exists()`.
  - `reject_symlink_destination()` checks only the final path, not every ancestor.
  - `write_workspace_file()` then runs `fs::create_dir_all(parent)` and opens the file, following symlink ancestors.
- **Trust boundary / sink:** Untrusted workspace tree and requested output path → `create_dir_all` / `OpenOptions::open`.
- **Impact:** A symlink inside the repo can redirect writes outside the workspace when the full parent path does not yet exist. Example: if `reports -> /tmp/outside`, writing `reports/new/out.md` creates/writes `/tmp/outside/new/out.md`, violating the workspace write boundary.
- **Exploitability / preconditions:** User runs a command that writes an output path inside an attacker-controlled repository containing a symlink ancestor.
- **Fix:** Resolve writes relative to an opened workspace directory. Reject symlinks at every existing component, reject Windows prefix components, re-canonicalize after creating parents and verify containment, and use `openat`/`O_NOFOLLOW`-style APIs for final opens where available.

---

### 4. `webfetch` public-IP validation misses non-public and IPv4-mapped ranges

- **Severity:** High
- **Category:** SSRF / incomplete network validation
- **Reference:** `src/tools.rs::{validate_public_url_parts, public_socket_addrs, ensure_public_ip, is_public_ip}`
- **Evidence:**
  - `validate_public_url_parts` / `public_socket_addrs` rely on `ensure_public_ip`.
  - `is_public_ip` rejects only selected ranges:
    - IPv4: private, loopback, link-local, broadcast, documentation, unspecified.
    - IPv6: loopback, unspecified, unique-local, link-local.
  - It does not reject IPv4 shared space `100.64.0.0/10`, benchmarking `198.18.0.0/15`, multicast/reserved ranges, deprecated IPv6 site-local ranges, or IPv4-mapped IPv6 such as `::ffff:127.0.0.1`.
- **Trust boundary / sink:** Model-controlled URL → DNS resolution / socket address validation → `reqwest::Client` connection in `tool_webfetch`.
- **Impact:** The advertised public-only network boundary can be bypassed to reach non-public networks. Depending on platform and resolver behavior, IPv4-mapped IPv6 may also allow access to localhost/internal IPv4 services.
- **Exploitability / preconditions:** `webfetch` is enabled; the runner can reach an internal or non-public service; attacker/model supplies a URL resolving to one of the missed ranges.
- **Fix:** Replace the handcrafted denylist with a strict globally-routable allowlist. Normalize IPv4-mapped IPv6 before validation. Add regression tests for `100.64.0.1`, `198.18.0.1`, multicast/reserved ranges, `fec0::1`, and `::ffff:127.0.0.1`.

---

### 5. Audit mode can send hidden and likely-secret workspace files to the model provider

- **Severity:** Medium
- **Category:** Sensitive data exposure
- **Reference:** `src/audit.rs::{collect_files, should_skip_path, chunk_text, audit_full_prompt, audit_chunk_prompt}`
- **Evidence:**
  - `collect_files` uses `WalkBuilder::hidden(false)` and `git_global(false)`.
  - It reads every non-empty UTF-8 file up to `MAX_FILE_BYTES`.
  - `should_skip_path` excludes `.git/`, build/dependency directories, `.tmp/`, and lockfiles, but not `.env`, `.npmrc`, private keys, cloud credentials, or local config/session files.
  - `chunk_text` appends `file.text`; `audit_full_prompt` / `audit_chunk_prompt` send that text through `session::run_prompt_once_no_tools`.
- **Trust boundary / sink:** Local workspace files, including hidden files → external configured model provider.
- **Impact:** Running `oy audit` can disclose API keys, tokens, private keys, or local config files that are present in the workspace but not gitignored.
- **Exploitability / preconditions:** Secret-bearing hidden/config file exists under the workspace and is not excluded by repo ignore rules; user audits with a remote provider.
- **Fix:** Skip hidden files and known secret filenames/extensions by default, respect global git excludes, and require explicit `--include-hidden` / `--include-sensitive` overrides. Add pre-send redaction/deny rules for common credential patterns.

---

### 6. Passive auth detection executes PATH-resolved `gh`/`aws` during no-tools/read-only setup flows

- **Severity:** Medium
- **Category:** Unexpected process execution / malicious executable resolution
- **Reference:** `src/agent.rs::{model::gh_auth_token, bedrock::aws_cli_available, bedrock::run_sso_login}`, `src/agent.rs::model::{auth_statuses, recommended_models}`, `src/cli.rs::app::{doctor_command, model_command}`
- **Evidence:**
  - `gh_auth_token()` runs `Command::new("gh").arg("auth").arg("token").output()`.
  - `aws_cli_available()` runs `Command::new("aws").arg("--version")`.
  - `recommended_models()` calls `auth_statuses()`.
  - `doctor` / `model` call model inspection paths that trigger these checks.
- **Trust boundary / sink:** User `PATH` / current workspace executable lookup → process spawn without tool approval.
- **Impact:** If the workspace or another attacker-controlled directory appears before trusted locations in `PATH`, a malicious `gh` or `aws` binary can execute during passive setup/model discovery flows. These commands also lack timeouts, so a malicious/broken binary can hang the CLI.
- **Exploitability / preconditions:** User runs `oy doctor`, `oy model`, or an error path that builds model recommendations with an attacker-controlled directory in `PATH`.
- **Fix:** Do not invoke external CLIs during passive auth detection or error-message construction. Use env/config-only checks by default. If CLI helpers are retained, require explicit opt-in, resolve trusted absolute paths, and add timeouts.

---

### 7. OpenAI credentials/base URL could be applied to non-OpenAI model requests

- **Severity:** Medium
- **Status:** Fixed in `src/agent.rs`: OpenAI env credentials and `OPENAI_BASE_URL` now apply only to OpenAI/OpenAIResp models when no routing shim is active.
- **Category:** Credential/provider confusion
- **Reference:** `src/agent.rs::model::{auth_resolver, service_target_resolver}`
- **Evidence:**
  - `auth_resolver()` returns `AuthData::from_single(api_key.clone())` for any model unless the model has a routing shim or `OY_SHIM` is set; it does not scope this by `model.adapter_kind`.
  - `service_target_resolver()` applies `OPENAI_BASE_URL` to any model whose namespace is absent or not a routing shim, and rewrites the model adapter with `openai_adapter_for_model(&model_name)`.
- **Trust boundary / sink:** Saved/CLI/env model selection plus `OPENAI_API_KEY` / `OPENAI_BASE_URL` → outbound authenticated model request.
- **Impact:** Selecting a non-routing/non-OpenAI model while OpenAI environment variables are set can attach the OpenAI key to the wrong provider path or route source prompts to an unintended OpenAI-compatible endpoint. This weakens provider isolation and can cause credential disclosure or prompt disclosure to the wrong backend.
- **Exploitability / preconditions:** User has OpenAI env vars set and selects a model/provider that is not explicitly routed through a supported shim.
- **Fix:** Scope `OPENAI_API_KEY` and `OPENAI_BASE_URL` handling to explicit OpenAI/OpenAIResp adapters or an explicit `openai` routing shim. Let provider-specific auth handle other adapters and fail closed on unknown namespaces.

---

### 8. File tools enforce limits after unbounded reads/search accumulation

- **Severity:** Medium
- **Category:** Resource exhaustion / operational DoS
- **Reference:** `src/tools.rs::{read_text_file, tool_read, tool_search, search_text_grep, tool_replace}`
- **Evidence:**
  - `read_text_file` calls `fs::read(path)` with no size cap.
  - `tool_read` slices requested lines only after the entire file is loaded.
  - `tool_search` accumulates every match and applies `args.limit` only when building the JSON response.
  - `search_text_grep` pushes every matching line.
  - `tool_replace` walks files and stores every changed file plus per-file diff before applying a result limit.
- **Trust boundary / sink:** Model/user-selected workspace paths and patterns → local memory/CPU.
- **Impact:** Large or adversarial repositories can make `read`, `search`, or `replace` consume excessive memory/CPU despite small limits, hanging or crashing normal/read-only CLI workflows.
- **Exploitability / preconditions:** User/model reads a large file, searches many matching files, or runs replace over a repository with generated/high-match text files.
- **Fix:** Add per-file byte caps and global traversal/match caps. Stream search results and stop once the requested limit plus a truncation marker is reached. Skip/summarize oversized files and cap total diff memory for `replace`.
