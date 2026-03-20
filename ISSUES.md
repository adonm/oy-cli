# Audit Findings

> **Last audit**: 2026-03-19 · commit `3cf9ec2` · OWASP ASVS 5.0 / grugbrain.dev cross-check
>
> **Codebase**: `oy-cli` v0.4.1b1 — tiny local coding CLI with a small tool surface
>
> | Metric | Value |
> |--------|-------|
> | Python files | 6 |
> | Python LoC | 5,123 code lines (6,268 total), complexity 984 |
> | `__init__.py` | 2,206 code lines, complexity 495 |
> | `providers.py` | 1,608 code lines, complexity 397 |
> | `shim.py` | 223 code lines (thin facade) |
> | Tests | 1,086 lines across 3 files |
> | Agent tools | 7 (`list`, `read`, `bash`, `search`, `ask`, `webfetch`, `todowrite`) |
> | Provider shims | 7 (`openai`, `codex`, `bedrock`, `bedrock-mantle`, `copilot`, `opencode`, `opencode-go`) |
> | Runtime dependencies | 10 (`boto3`, `defopt`, `httpx`, `openai`, `rich`, `tiktoken`, `tenacity`, `msgspec`, `headroom-ai`, `prompt-toolkit`) |
>
> **Audit lens**: CLI agent that reads/writes workspace files via `bash`, executes shell commands, makes outbound API calls, and fetches arbitrary URLs via `webfetch`. Priority: workspace-boundary safety, secret handling, SSRF prevention, provider complexity, and keeping a deliberately small codebase from accreting unnecessary weight.

## H1 · Debug log may persist full prompts, tool outputs, and secrets without permission hardening or redaction

| | |
|---|---|
| **Location** | `__init__.py:147-173`, `__init__.py:184-194` |
| **Category** | Security |
| **Standard** | [OWASP ASVS 5.0 §V14.2.4](https://github.com/OWASP/ASVS/blob/master/5.0/en/0x23-V14-Data-Protection.md) — implement controls per data protection level; [§V16.2.5](https://github.com/OWASP/ASVS/blob/master/5.0/en/0x25-V16-Security-Logging-and-Error-Handling.md) — do not log credentials or sensitive data; grugbrain: avoid hidden footguns |
| **Status** | **Resolved** |

When `OY_DEBUG` is enabled, the process opens `~/.config/oy/debug.jsonl` via `logging.FileHandler` and logs full request/response payloads including repository contents, tool results, and any secrets pasted by users. The file is created without explicit `chmod(0o600)`, so permissions depend on umask and pre-existing state. The runtime toggle (`/debug`) guards against duplicate handlers but `_init_debug_log()` at import time does not.

**Resolution**: File is now created via `os.open()` with `0o600` mode and `chmod()`'d on every startup. Handler guard (`if not logger.handlers`) added to `_init_debug_log()`. Docstring documents the sensitivity of the log content.

---

## H2 · `bash` tool executes arbitrary shell with inherited credentials and broad ambient authority

| | |
|---|---|
| **Location** | `__init__.py:1441-1466` |
| **Category** | Security |
| **Standard** | [OWASP ASVS 5.0 §V8.3.1](https://github.com/OWASP/ASVS/blob/master/5.0/en/0x17-V8-Authorization.md) — enforce authorization at a trusted service layer; grugbrain: make risk obvious |
| **Status** | Accepted risk / Open |

`tool_bash()` runs `bash -c` inside the workspace with the full launch environment. This is a core product feature, but the model has access to everything the invoking user can reach: git credentials, cloud credentials, SSH agents, package managers, and destructive filesystem commands.

**Recommendation**: Keep the feature but make the risk more explicit. Provide a documented `--safe` or env-based mode that strips high-risk environment variables (e.g., `AWS_SECRET_ACCESS_KEY`, `OPENAI_API_KEY`) from the bash subprocess environment, or requires confirmation for destructive commands. Document this clearly as a trusted-local-user feature.

---

## H3 · `webfetch` SSRF check is vulnerable to DNS rebinding (TOCTOU between resolve and request)

| | |
|---|---|
| **Location** | `__init__.py:1605-1633` |
| **Category** | Security |
| **Standard** | [OWASP ASVS 5.0 §V15.4.2](https://github.com/OWASP/ASVS/blob/master/5.0/en/0x24-V15-Secure-Coding-and-Architecture.md) — perform state checks and dependent actions atomically to prevent TOCTOU; grugbrain: beware gap between check and use |
| **Status** | **Partially resolved** |

`_validate_url_safe()` resolves the hostname via `socket.getaddrinfo()` and checks all IPs are public, then separately hands the URL to `xh` which resolves the hostname *again*. Between these two steps, a DNS rebinding attack could change the resolution from a public IP to a private/loopback address, bypassing the SSRF protection.

**Resolution**: Removed `--follow` from the `xh` call to prevent redirect-based SSRF bypass. The DNS TOCTOU gap remains an accepted limitation (blast radius limited to `GET`/`HEAD`/`OPTIONS`).

---

## H4 · HTTP clients follow redirects broadly, expanding trust boundaries for provider calls

| | |
|---|---|
| **Location** | `providers.py:491-510` |
| **Category** | Security |
| **Standard** | [OWASP ASVS 5.0 §V15.3.2](https://github.com/OWASP/ASVS/blob/master/5.0/en/0x24-V15-Secure-Coding-and-Architecture.md) — do not follow redirects on backend calls unless intended |
| **Status** | **Resolved** |

Both `http_client()` and `async_http_client()` previously enabled `follow_redirects=True`. These clients are used for provider API calls (OAuth token refreshes, model list fetches, Codex/Copilot endpoints) and for the Bedrock SigV4 flow.

**Resolution**: Changed both `http_client()` and `async_http_client()` to `follow_redirects=False` to prevent bearer tokens from leaking to unintended redirect targets.

---

## H5 · `webfetch` passes model-supplied headers to `xh` without sanitization

| | |
|---|---|
| **Location** | `__init__.py:1681-1684` |
| **Category** | Security |
| **Standard** | [OWASP ASVS 5.0 §V1.2.5](https://github.com/OWASP/ASVS/blob/master/5.0/en/0x10-V1-Encoding-and-Sanitization.md) — protect against OS command injection and use contextual encoding; grugbrain: validate at boundary |
| **Status** | **Resolved** |

The `tool_webfetch()` function passes header key-value pairs from the model directly to `xh` as `key:value` command-line arguments. While `subprocess.run` with a list prevents shell injection, the model could craft headers like `Authorization: Bearer <stolen-token>` or `Host: evil.com` to manipulate the HTTP request.

**Resolution**: Added `_sanitize_webfetch_headers()` with a blocklist (`Authorization`, `Cookie`, `Host`, `Proxy-Authorization`, `X-Forwarded-For`, `X-Real-IP`) and CRLF rejection for header values.

---

## M1 · Two modules concentrate most behavior and provider complexity

| | |
|---|---|
| **Location** | `__init__.py` (2,206 code lines, complexity 495), `providers.py` (1,608 code lines, complexity 397) |
| **Category** | Complexity |
| **Standard** | grugbrain: complexity very, very bad; [OWASP ASVS 5.0 §V15.1.5](https://github.com/OWASP/ASVS/blob/master/5.0/en/0x24-V15-Secure-Coding-and-Architecture.md) — document dangerous functionality |
| **Status** | Open (stable — `__init__.py` grew +19 net lines for headroom hooks and hardening) |

The project intentionally keeps file count low, but `__init__.py` mixes CLI flow, tool dispatch, transcript management, prompt parsing, Rich rendering, and the chat REPL, while `providers.py` contains all seven provider adapters, OAuth flows, retry logic, codec abstractions, and HTTP signing. The `shim.py` extraction was a positive step.

**Recommendation**: Split by responsibility: `tools.py` (tool dispatch + registry), `transcript.py` (message management + headroom integration). Break `providers.py` into `providers/core.py` + per-provider modules only if further growth occurs. Keep the public surface tiny.

---

## M2 · Provider-specific OAuth and model plumbing in `providers.py` is growing

| | |
|---|---|
| **Location** | `providers.py` broadly — OAuth flows, credential persistence, model caching, retry logic, Bedrock signing, 7 provider adapters |
| **Category** | Complexity |
| **Standard** | grugbrain: one day simple system become complicated mess |
| **Status** | Open (stable since last audit) |

`providers.py` owns provider discovery, OAuth device/refresh flows for 2 providers (Codex, Copilot), credential persistence, model caching, dual retry frameworks (`_call_with_retry` for HTTP and SDK), Bedrock SigV4 signing, and codec adapters for 2 wire formats (OpenAI, Bedrock). Each piece is individually reasonable but together they make the module harder to audit.

**Recommendation**: Define a narrow provider interface (already partially done with `ShimSpec`/`HttpChatEndpoint`) and move each provider's auth/model quirks behind isolated modules. The goal is not lots of files; it is to stop every new provider from expanding shared branching logic.

---

## M3 · Codex ChatGPT client retry loop is hand-rolled

| | |
|---|---|
| **Location** | `providers.py:1677-1721` |
| **Category** | Complexity |
| **Standard** | grugbrain: same thing done same way; [OWASP ASVS 5.0 §V13.1.3](https://github.com/OWASP/ASVS/blob/master/5.0/en/0x22-V13-Configuration.md) — document retry limits and back-off |
| **Status** | Open |

The `_codex_chatgpt_client()` uses a manual `for attempt in range(2)` loop with inline SSE parsing and 401 handling, while all other providers use the shared `_call_with_retry()` infrastructure. This means the Codex ChatGPT path lacks exponential backoff, retry-after header parsing, and the shared `on_retry` callback for spinner updates.

**Recommendation**: Refactor to use `_send_with_retry()` or `_call_with_retry()` like the other clients, with the SSE decode as the response decoder.

---

## M4 · `_refresh_mise_env` mutates `os.environ` globally

| | |
|---|---|
| **Location** | `__init__.py:791-831` |
| **Category** | Complexity |
| **Standard** | grugbrain: avoid surprising side effects |
| **Status** | **Resolved** |

`_refresh_mise_env()` applies `mise env -J` output by directly mutating `os.environ`, including deleting keys where values are `None`. This is done intentionally so that subsequent `command_env()` calls pick up new PATH entries, but it means any code holding a reference to the old environment or relying on a specific env var may be silently broken.

**Resolution**: Added a comprehensive docstring documenting the global mutation side effect and cache invalidation behavior.

---

## M5 · Reasoning fallback cache is a global mutable dict without thread safety

| | |
|---|---|
| **Location** | `providers.py:1136-1149` |
| **Category** | Performance / Complexity |
| **Standard** | [OWASP ASVS 5.0 §V15.4.1](https://github.com/OWASP/ASVS/blob/master/5.0/en/0x24-V15-Secure-Coding-and-Architecture.md) — access shared objects in multi-threaded code safely |
| **Status** | **Resolved** |

`_REASONING_SUPPORT_CACHE` is a plain dict used to remember which `(api_kind, model)` pairs don't support reasoning. The chat REPL runs background threads for `/ask` and `/audit` commands, each with their own `asyncio.run()`, which means concurrent writes to this dict are possible.

**Resolution**: Added `_REASONING_CACHE_LOCK = threading.Lock()` to guard all reads and writes to the cache dict.

---

## M6 · `_decode_tool_call_arguments` uses a brute-force mid-scan for duplicated JSON

| | |
|---|---|
| **Location** | `providers.py:1095-1126` |
| **Category** | Complexity |
| **Standard** | grugbrain: don't be too clever |
| **Status** | **Resolved** |

When a provider returns duplicated JSON (e.g., `{"ok":true}{"ok":true}`), the function scanned ±15 characters around the midpoint looking for `{` and tried to decode from each position. This was fragile and also had a Python 2-style `except` clause.

**Resolution**: Replaced the midpoint scan with a wider ±40-character boundary detection around the midpoint. Fixed the `except` syntax from bare `except A, B:` to parenthesised `except (A, B):` throughout `providers.py`.

---

## M7 · Tests cover recent safety fixes well, but riskiest new paths lack direct coverage

| | |
|---|---|
| **Location** | `tests/test_oy_cli.py`, `tests/test_shim.py`, `tests/test_async_cleanup.py` |
| **Category** | Complexity |
| **Standard** | [OWASP ASVS 5.0 §V15.1.1](https://github.com/OWASP/ASVS/blob/master/5.0/en/0x24-V15-Secure-Coding-and-Architecture.md) — remediation timeframes for component vulnerabilities |
| **Status** | Open (improved — background thread lifecycle concern eliminated) |

Current tests cover path traversal, tool dispatch, JSON summarization, transcript lifecycle, shim bridge delegation, reasoning fallback, provider encoding, SSRF URL validation, `todowrite` schema validation, webfetch header blocklist, and `_refresh_mise_env` global mutation. Good. But there is no visible direct test coverage for:
- DNS rebinding / TOCTOU in `_validate_url_safe` → `xh` flow
- Debug log permissions (`0o600` enforcement)
- Provider OAuth refresh error paths (Codex token refresh failures)
- The Bedrock SigV4 signing path (`make_bedrock_token`)
- Headroom compression hooks (`_HeadroomHooks` bias logic)

**Recommendation**: Add small focused tests around high-risk invariants: SSRF boundary, debug log file mode, OAuth refresh error handling, and headroom compression with thought signatures. These targeted tests will do more for auditability than broad integration tests.

---

## M8 · Config and session directories are created without restrictive permissions

| | |
|---|---|
| **Location** | `__init__.py:158`, `__init__.py:2098`, `__init__.py:2351`, `__init__.py:2369` |
| **Category** | Security |
| **Standard** | [OWASP ASVS 5.0 §V14.2.4](https://github.com/OWASP/ASVS/blob/master/5.0/en/0x23-V14-Data-Protection.md) — implement controls per data protection level; grugbrain: apply defense at every layer |
| **Status** | Open (new finding) |

The `~/.config/oy/` directory, its `sessions/` subdirectory, and parent directories are created via `Path.mkdir(parents=True, exist_ok=True)` without specifying `mode=0o700`. While individual files (`debug.jsonl`, `config.json`, `history`, session files) are correctly `chmod`'d to `0o600`, the directories themselves inherit the process umask. On a shared system with a permissive umask (e.g., `0o022`), the directories will be world-readable, allowing other users to enumerate file names even if file contents remain protected.

**Recommendation**: Add `mode=0o700` to `mkdir()` calls for the config and sessions directories. This provides defense-in-depth alongside the per-file `0o600` permissions.

---

## Resolved or improved since the previous audit

| Finding | Status | Notes |
|---------|--------|-------|
| H2 (old) · `httpx` tool allows unrestricted outbound network access | **Resolved** | The dedicated `httpx` fetch tool was previously removed. The new `webfetch` tool replaces it with SSRF protection (URL validation + method restriction). See findings H3/H5 for remaining gaps. |
| M3 (old) · Arbitrary file writes do not enforce restrictive mode | **Previously resolved** | File writes via `bash` only; inherits normal filesystem behavior. |
| M4 (old) · Response buffering in `httpx` tool | **Previously resolved** | Tool removed; no longer applicable. |
| H5/H6 (old) · Two monolithic modules + provider complexity | **Improved** | Gemini and Claude shims removed (`ab473ac`, -846 lines). OpenCode shims added but simpler (thin OpenAI-compatible wrappers). Net reduction in complexity. |
| Workspace path traversal | **Previously resolved** | `resolve_path()` and `_glob_paths()` remain effective with test coverage. |
| Credential persistence permissions | **Previously resolved** | `save_json()` continues to use `chmod(0o600)`. |
| Sensitive HTTP headers in output | **Previously resolved** | Redaction remains in place. |
| H1 (debug log permissions) | **Resolved** | `0o600` enforced via `os.open()` + `chmod()`; handler guard added. |
| H3 (webfetch redirect SSRF) | **Partially resolved** | `--follow` removed from `xh`; DNS TOCTOU accepted. |
| H4 (provider redirect following) | **Resolved** | `follow_redirects=False` on both HTTP clients. |
| H5 (webfetch header injection) | **Resolved** | Blocklist + CRLF rejection in `_sanitize_webfetch_headers()`. |
| M4 (_refresh_mise_env docs) | **Resolved** | Comprehensive docstring added. |
| M5 (reasoning cache thread safety) | **Resolved** | `threading.Lock` guards added. |
| M6 (duplicate JSON parsing) | **Resolved** | Wider boundary scan; `except` syntax fixed throughout. |
| JSON path depth cap | **Previously resolved** | `_summarize_json_value` depth=6 cap with test coverage. |
| Debug handler duplication (H3 old) | **Resolved** | `_init_debug_log()` now guards with `if not logger.handlers:`. |
| Background thread lifecycle | **Resolved** | `/ask` and `/audit` now run synchronously via `asyncio.run()` (commit `3cf9ec2`); threading dropped from `__init__.py`. |
| `except A, B:` syntax errors | **Resolved** | All four instances in `providers.py` fixed to parenthesised `except (A, B):` form (commit `3cf9ec2`). |

## Short audit log

- 2026-03-19: Refreshed audit for commit `3cf9ec2`, version 0.4.1b1, OWASP ASVS 5.0 (May 2025 release).
  - Cross-checked against ASVS 5.0 chapters V1 (Encoding & Sanitization), V8 (Authorization), V13 (Configuration), V14 (Data Protection), V15 (Secure Coding & Architecture), V16 (Logging & Error Handling).
  - Cross-checked against grugbrain.dev simplicity guidance (complexity very, very bad; avoid hidden footguns; same thing done same way; validate at boundary).
  - Notable changes since commit `c0a0300`: security hardening (except syntax fixes, redirect-following disabled, header sanitization), dropped threading from `__init__.py` in favor of synchronous `asyncio.run()` for `/ask` and `/audit`, Python target lowered to 3.13, headroom compression hooks added.
  - New finding: M8 (config/session directory permissions).
  - Background thread lifecycle concern eliminated — `/ask` and `/audit` no longer use background threads.
  - `except A, B:` bare syntax fully resolved across providers.py.
  - All 13 findings carried forward; 1 new finding (M8); 13 total (within 10-15 target).
  - 5 high findings, 8 medium findings.
- 2026-03-19: Previous audit for commit `c0a0300`, version 0.4.1b1. Added `todowrite` tool review; all 12 findings stable.
- 2026-03-19: Previous audit for commit `3f8cfad`, version 0.4.1b1. New findings: H3 (SSRF TOCTOU), H5 (header injection in webfetch).
- 2026-03-18: Previous audit for commit `e7d77b1`, version 0.4.0.
