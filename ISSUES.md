# Audit Findings

> **Last audit**: 2026-03-18 · commit `e7d77b1` · OWASP ASVS 5.0 / grugbrain.dev cross-check
>
> **Codebase**: `oy-cli` v0.4.0 — tiny local coding CLI with a small tool surface
>
> | Metric | Value |
> |--------|-------|
> | Python files | 6 |
> | Python LoC | 5,503 code lines (6,692 total) |
> | `oy_cli.py` | 2,230 lines (1,807 code, complexity 420) |
> | `providers.py` | 2,847 lines (2,375 code) |
> | `shim.py` | 372 lines (thin facade) |
> | Tests | 1,239 lines across 3 files |
> | Agent tools | 5 (`list`, `read`, `bash`, `search`, `ask`) |
> | Provider shims | 9 (`openai`, `codex`, `gemini`, `bedrock`, `bedrock-mantle`, `claude`, `copilot`, `opencode`, `opencode-go`) |
> | Runtime dependencies | 9 (`boto3`, `defopt`, `httpx`, `openai`, `rich`, `tiktoken`, `tenacity`, `msgspec`, `headroom-ai`) |
>
> **Audit lens**: CLI agent that reads/writes workspace files via `bash`, executes shell commands, and makes outbound API calls. Priority: workspace-boundary safety, secret handling, provider complexity, and keeping a deliberately small codebase from accreting unnecessary weight.

## H1 · Debug log may persist full prompts, tool outputs, and secrets without permission hardening or redaction

| | |
|---|---|
| **Location** | `oy_cli.py:189-202`, `oy_cli.py:1642-1707` |
| **Category** | Security |
| **Standard** | OWASP ASVS 5.0 V14.2.4, V16.2.5 (log sensitive data per protection level); grugbrain: avoid hidden footguns |
| **Status** | Open |

When `OY_DEBUG` is enabled, the process opens `~/.config/oy/debug.jsonl` via `logging.FileHandler` and logs full request/response payloads including repository contents, tool results, and any secrets pasted by users. The file is created without explicit `chmod(0o600)`, so permissions depend on umask and pre-existing state.

**Recommendation**: Enforce `0o600` on the debug log file at creation and on reuse. Document the risk in `--help` output. Redact obviously sensitive fields (e.g., bearer tokens, API keys in environment dumps). Consider a `OY_DEBUG=meta` mode that logs timing and tool names without full content.

---

## H2 · `bash` tool executes arbitrary shell with inherited credentials and broad ambient authority

| | |
|---|---|
| **Location** | `oy_cli.py:1372-1398`, `providers.py:556-575` |
| **Category** | Security |
| **Standard** | OWASP ASVS 5.0 V15.2.5, V8 (authorization); grugbrain: make risk obvious |
| **Status** | Accepted risk / Open |

`tool_bash()` runs `bash -c` inside the workspace with the full launch environment. This is a core product feature, but the model has access to everything the invoking user can reach: git credentials, cloud credentials, SSH agents, package managers, and destructive filesystem commands.

**Recommendation**: Keep the feature but make the risk more explicit. Provide a documented `--safe` or env-based mode that strips high-risk environment variables (e.g., `AWS_SECRET_ACCESS_KEY`, `OPENAI_API_KEY`) from the bash subprocess environment, or requires confirmation for destructive commands. Document this clearly as a trusted-local-user feature.

---

## H3 · Debug logger setup may duplicate handlers across imports or in test scenarios

| | |
|---|---|
| **Location** | `oy_cli.py:189-202` |
| **Category** | Security / Complexity |
| **Standard** | OWASP ASVS 5.0 V16.4.1 (log injection prevention); grugbrain: avoid surprising behavior |
| **Status** | Open |

`_init_debug_log()` attaches a new `FileHandler` to the `oy.debug` logger every time it runs. In normal CLI use this happens once, but in reload scenarios, embedded use, or tests this can duplicate log lines and retain extra file handles. The `logger.propagate = False` mitigates propagation, but the handler list itself is unchecked.

**Recommendation**: Guard with `if not logger.handlers:` before adding a new handler, or use a module-local sentinel to ensure one-shot initialization.

---

## H4 · HTTP clients follow redirects broadly, expanding trust boundaries for provider calls

| | |
|---|---|
| **Location** | `providers.py:618-633` |
| **Category** | Security |
| **Standard** | OWASP ASVS 5.0 V15.3.2 ("backend calls to external URLs should not follow redirects unless intended") |
| **Status** | Open |

Both `http_client()` and `async_http_client()` enable `follow_redirects=True`. These clients are used for provider API calls (OAuth token refreshes, model list fetches, Gemini/Claude/Codex endpoints) and for the Bedrock SigV4 flow. While redirects are often benign for well-known API endpoints, a compromised or misconfigured upstream could redirect to internal addresses or unexpected hosts.

**Recommendation**: For provider API clients where redirect behavior is not expected, disable `follow_redirects` or limit it to only the specific endpoints that need it. This is now the only HTTP concern since the dedicated `httpx` fetch tool was removed.

---

## H5 · Two modules concentrate most behavior and provider complexity

| | |
|---|---|
| **Location** | `oy_cli.py` (2,230 lines), `providers.py` (2,685 lines) |
| **Category** | Complexity |
| **Standard** | OWASP ASVS 5.0 V15.1.5 (document dangerous functionality); grugbrain: complex thing bad |
| **Status** | Open (improved since last audit — `shim.py` extracted as facade) |

The project intentionally keeps file count low, but `oy_cli.py` mixes CLI flow, tool dispatch, transcript management, and prompt parsing, while `providers.py` contains all seven provider adapters, OAuth flows, retry logic, codec abstractions, and HTTP signing. The `shim.py` extraction was a positive step, but reviewer cognitive load for the two big modules remains high.

**Recommendation**: Split by responsibility, not by framework fashion. Candidates: `tools.py` (tool dispatch + registry), `transcript.py` (message management + headroom integration), and breaking `providers.py` into `providers/core.py` + per-provider modules. Keep the public surface tiny.

---

## H6 · Provider-specific OAuth and model plumbing is drifting toward a mini platform inside `providers.py`

| | |
|---|---|
| **Location** | `providers.py` broadly — OAuth flows, credential persistence, model caching, retry logic, Bedrock signing, 7 provider adapters |
| **Category** | Complexity |
| **Standard** | grugbrain: one day simple system become complicated mess |
| **Status** | Open |

`providers.py` now owns provider discovery, OAuth device/refresh flows for 4 providers (Codex, Gemini, Claude, Copilot), credential persistence, model caching, dual retry frameworks (`_send_with_retry` for HTTP and `_call_with_retry` for SDK), Bedrock SigV4 signing, and codec adapters for 3 wire formats (OpenAI, Bedrock, Anthropic/Vertex). Each piece is individually reasonable but together they make the module harder to audit than the product concept suggests.

**Recommendation**: Define a narrow provider interface (already partially done with `ShimSpec`/`HttpChatEndpoint`) and move each provider's auth/model quirks behind isolated modules. The goal is not lots of files; it is to stop every new provider from expanding shared branching logic.

---

## M1 · Codex ChatGPT client retry loop is hand-rolled rather than using shared retry infrastructure

| | |
|---|---|
| **Location** | `providers.py:2365-2408` |
| **Category** | Complexity |
| **Standard** | grugbrain: same thing done same way; OWASP ASVS 5.0 V15.1.3 (document resource-demanding functionality) |
| **Status** | Open |

The `_codex_chatgpt_client()` uses a manual `for attempt in range(2)` loop with inline SSE parsing and 401 handling, while all other providers use the shared `_send_with_retry()` or `_call_with_retry()` infrastructure. This means the Codex ChatGPT path lacks exponential backoff, retry-after header parsing, and the shared on_retry callback for spinner updates.

**Recommendation**: Refactor to use `HttpChatEndpoint` + `_send_with_retry()` like the Claude and Gemini clients, with the SSE decode as the response decoder. This eliminates the hand-rolled auth retry and brings consistent retry behavior.

---

## M2 · `_refresh_mise_env` mutates `os.environ` globally, which may surprise callers

| | |
|---|---|
| **Location** | `oy_cli.py:797-827` |
| **Category** | Complexity |
| **Standard** | grugbrain: avoid surprising side effects |
| **Status** | Open |

`_refresh_mise_env()` applies `mise env -J` output by directly mutating `os.environ`, including deleting keys where values are `None`. This is done intentionally so that subsequent `command_env()` calls pick up new PATH entries, but it means any code holding a reference to the old environment or relying on a specific env var being present may be silently broken.

**Recommendation**: Document this side effect with a prominent docstring and log a debug message when env vars are added/removed. Consider returning the updated env dict directly rather than relying on global mutation.

---

## M3 · Reasoning fallback cache is a global mutable dict without thread safety

| | |
|---|---|
| **Location** | `providers.py:1621-1633` |
| **Category** | Performance / Complexity |
| **Standard** | OWASP ASVS 5.0 V15.4.1 (shared objects accessed safely) |
| **Status** | Open |

`_REASONING_SUPPORT_CACHE` is a plain dict used to remember which (api_kind, model) pairs don't support reasoning parameters. In the current single-threaded asyncio CLI this is fine, but the pattern is fragile: if the codebase ever runs parallel requests or is embedded in a threaded host, concurrent reads/writes to this dict would be a data race.

**Recommendation**: Use `functools.cache` or a simple frozen sentinel pattern instead of a bare dict. Or document the single-threaded assumption explicitly.

---

## M4 · Tests cover recent safety fixes well, but riskiest paths still lack direct coverage

| | |
|---|---|
| **Location** | `tests/test_oy_cli.py`, `tests/test_shim.py`, `tests/test_async_cleanup.py` |
| **Category** | Complexity |
| **Standard** | OWASP ASVS 5.0 V15.1.1 (remediation timeframes for component vulnerabilities) |
| **Status** | Open |

Current tests cover path traversal, tool dispatch, JSON summarization, transcript lifecycle, shim bridge delegation, reasoning fallback, and provider encoding. That is good. But there is no visible direct test coverage for:
- Debug log permissions (`0o600` enforcement)
- `_refresh_mise_env` global mutation behavior
- Provider OAuth refresh error paths (Gemini, Claude, Codex token refresh failures)
- The Bedrock SigV4 signing path (`make_bedrock_token`)

**Recommendation**: Add small focused tests around high-risk invariants: debug log file mode, OAuth refresh error handling, and SigV4 signing correctness. These targeted tests will do more for auditability than broad integration tests.

---

## M5 · `_decode_tool_call_arguments` uses a brute-force mid-scan for duplicated JSON

| | |
|---|---|
| **Location** | `providers.py:1581-1611` |
| **Category** | Complexity |
| **Standard** | grugbrain: don't be too clever |
| **Status** | Open |

When a provider returns duplicated JSON (e.g., `{"ok":true}{"ok":true}`), the function scans ±15 characters around the midpoint looking for `{` and tries to decode from each position. While this handles a real provider quirk, it is fragile: off-by-one in the scan range could miss the split, and the approach doesn't generalize.

**Recommendation**: Replace the midpoint scan with a simpler approach: try to decode the full string, and on failure, find the first `}{` boundary and decode from after it. This is more robust and easier to understand.

---

## Resolved or improved since the previous audit

| Finding | Status | Notes |
|---------|--------|-------|
| H2 (old) · `httpx` tool allows unrestricted outbound network access | **Resolved** | The dedicated `httpx` fetch tool has been removed from the tool surface. HTTP calls are now only possible through `bash` (e.g., via `xh`), which is already an accepted-risk channel. |
| M3 (old) · Arbitrary file writes do not enforce restrictive mode | **Resolved** | The `write`/`replace` tools have been removed. File writes are now done via `bash` commands (e.g., `ast-grep`), inheriting normal filesystem behavior. |
| M4 (old) · Response buffering in `httpx` tool | **Resolved** | Tool removed; no longer applicable. |
| H4 (old) · Two monolithic modules | **Improved** | `shim.py` extracted as a thin facade, and `providers.py` split from it. Still open as H5/H6 above because the two big modules remain dense. |
| Workspace path traversal | **Previously resolved** | `resolve_path()` and `_glob_paths()` remain effective with test coverage. |
| Credential persistence permissions | **Previously resolved** | `save_json()` continues to use `chmod(0o600)`. |
| Sensitive HTTP headers in output | **Previously resolved** | Redaction remains in place. |
| JSON path depth cap | **Previously resolved** | `_summarize_json_value` depth=6 cap with test coverage. |

## Short audit log

- 2026-03-18: Refreshed audit for commit `e7d77b1`, version 0.4.0, OWASP ASVS 5.0 (May 2025 release).
- Cross-checked against ASVS 5.0 chapters V14 (Data Protection), V15 (Secure Coding), V16 (Logging).
- Cross-checked against grugbrain.dev simplicity guidance.
- Noted tool surface reduction: `httpx`, `write`, `replace` tools removed since prior audit.
- Noted `shim.py` extraction as positive complexity reduction; `providers.py` now the largest module.
- 3 prior findings marked resolved; 2 marked improved; 10 current findings written.
