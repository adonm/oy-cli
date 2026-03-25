# Audit Findings

> **Last audit**: 2026-03-25 · commit `37275d2` (`Inline SigV4 signing and finish provider cleanup`) · cross-checked against OWASP ASVS 5.0.0 (May 2025) and [grugbrain.dev](https://grugbrain.dev/)
>
> **Codebase**: `oy-cli` v0.4.2 — local coding CLI with workspace-bound file tools, shell execution, outbound fetch, and 6 provider shims.
>
> | Metric | Value |
> |---|---|
> | Python files | 11 total (8 package modules + 3 tests) |
> | Python LoC | 5,147 code lines (7,021 total) |
> | Largest modules | `providers.py` 1,220 code / 1,611 total; `tools.py` 1,138 / 1,480; `runtime.py` 557 / 805; `cli.py` 525 / 691 |
> | Tests | 1,131 code lines across 3 files |
> | Agent tools | 9 (`ask`, `bash`, `list`, `read`, `replace`, `search`, `sloc`, `todo`, `webfetch`) |
> | Provider shims | 6 (`openai`, `codex`, `bedrock-mantle`, `copilot`, `opencode`, `opencode-go`) |
> | Runtime dependencies | 14 direct packages |
>
> **Audit lens**: dangerous local execution (`bash`), outbound network use (`webfetch` + provider APIs), secret handling, workspace boundary enforcement, module growth, and avoidable latency/memory blow-ups on large repos.

## H1 · `bash` executes arbitrary shell with inherited credentials and full user authority

| | |
|---|---|
| **Location** | `oy_cli/tools.py:466-489` |
| **Category** | Security |
| **Reference** | [OWASP ASVS 5.0 V8 §8.3.1](https://raw.githubusercontent.com/OWASP/ASVS/master/5.0/en/0x17-V8-Authorization.md), [OWASP ASVS 5.0 V15 §15.1.5](https://raw.githubusercontent.com/OWASP/ASVS/master/5.0/en/0x24-V15-Secure-Coding-and-Architecture.md), [grugbrain.dev](https://grugbrain.dev/) |
| **Recommendation** | Keep as an explicit trusted-local-user feature, but add a `--safe` / env-stripped mode, stronger destructive-command checkpoints, and docs that this inherits git/cloud/SSH credentials. |
| **Status** | Accepted risk / Open |

Evidence: `tool_bash()` pulls the full command environment via `rt.require_command_env(state.root)` and runs `[bash, "-c", command]` in the workspace. In normal `run`, `chat`, and `audit` flows this gives the model the same ambient authority as the invoking user.

---

## H2 · `/ask` is described as “read-only”, but it still permits outbound network requests via `webfetch`

| | |
|---|---|
| **Location** | `oy_cli/runtime.py:327-348`, `oy_cli/cli.py:202-203`, `oy_cli/cli.py:323-327`, `oy_cli/tools.py:655-690` |
| **Category** | Security |
| **Reference** | [OWASP ASVS 5.0 V13 §13.2.4](https://raw.githubusercontent.com/OWASP/ASVS/master/5.0/en/0x22-V13-Configuration.md), [OWASP ASVS 5.0 V14 §14.2.3](https://raw.githubusercontent.com/OWASP/ASVS/master/5.0/en/0x23-V14-Data-Protection.md) |
| **Recommendation** | Remove `webfetch` from the read-only tool set, or rename the mode to make network side effects explicit and require confirmation before first external fetch. |
| **Status** | Open (new) |

Evidence: `_READ_ONLY_TOOLS` includes `webfetch`, while `/ask` is presented as “research-only” and “read-only, no changes”. That mode still lets the model contact arbitrary public URLs, which is enough for network side effects and query-string/path exfiltration.

---

## H3 · `webfetch` can follow redirects without re-validating the target, reopening SSRF paths

| | |
|---|---|
| **Location** | `oy_cli/tools.py:662-690` |
| **Category** | Security |
| **Reference** | [OWASP ASVS 5.0 V15 §15.3.2](https://raw.githubusercontent.com/OWASP/ASVS/master/5.0/en/0x24-V15-Secure-Coding-and-Architecture.md), [OWASP ASVS 5.0 V13 §13.2.4](https://raw.githubusercontent.com/OWASP/ASVS/master/5.0/en/0x22-V13-Configuration.md) |
| **Recommendation** | Hard-disable redirects for `webfetch`, or intercept and re-run the same SSRF policy on every hop before following. |
| **Status** | Open (regression) |

Evidence: `_validate_url_safe(url)` is applied once to the initial URL, but `tool_webfetch()` passes `follow_redirects=options.follow_redirects` straight into `httpx.Client`. A public URL can redirect to a blocked address after the one-time check.

---

## H4 · `webfetch` SSRF validation is still vulnerable to DNS rebinding / TOCTOU

| | |
|---|---|
| **Location** | `oy_cli/tools.py:493-521`, `oy_cli/tools.py:686-690` |
| **Category** | Security |
| **Reference** | [OWASP ASVS 5.0 V15 §15.4.2](https://raw.githubusercontent.com/OWASP/ASVS/master/5.0/en/0x24-V15-Secure-Coding-and-Architecture.md), [OWASP ASVS 5.0 V1 §1.5.3](https://raw.githubusercontent.com/OWASP/ASVS/master/5.0/en/0x10-V1-Encoding-and-Sanitization.md) |
| **Recommendation** | Pin requests to the validated IP, or use a transport that resolves once and enforces the checked address family/range. At minimum, document this limitation next to the tool. |
| **Status** | Open |

Evidence: `_validate_url_safe()` resolves the hostname with `socket.getaddrinfo()` and rejects private/reserved results, then the later `httpx` request resolves again. That leaves a check/use gap for DNS rebinding.

---

## H5 · Debug logging still persists raw prompts and transcript data without redaction or retention control

| | |
|---|---|
| **Location** | `oy_cli/runtime.py:355-382`, `oy_cli/agent.py:285-323`, `oy_cli/cli.py:307-318` |
| **Category** | Security |
| **Reference** | [OWASP ASVS 5.0 V14 §14.2.4](https://raw.githubusercontent.com/OWASP/ASVS/master/5.0/en/0x23-V14-Data-Protection.md), [OWASP ASVS 5.0 V16](https://raw.githubusercontent.com/OWASP/ASVS/master/5.0/en/0x25-V16-Security-Logging-and-Error-Handling.md) |
| **Recommendation** | Keep the `0o600` hardening, but add redaction for likely secrets, an obvious warning in README/help, and optional rotation / TTL so sensitive logs do not quietly accumulate. |
| **Status** | Partially resolved |

Evidence: permissions are now hardened, but `_debug_log("request", messages=[...prepared...])` serializes the full prepared transcript, including file content and prior tool output. There is still no redaction or documented retention policy.

---

## M1 · `providers.py` remains the dominant complexity hotspot

| | |
|---|---|
| **Location** | `oy_cli/providers.py` (1,220 code lines / 1,611 total) |
| **Category** | Complexity |
| **Reference** | [OWASP ASVS 5.0 V15 §15.1.5](https://raw.githubusercontent.com/OWASP/ASVS/master/5.0/en/0x24-V15-Secure-Coding-and-Architecture.md), [grugbrain.dev](https://grugbrain.dev/) |
| **Recommendation** | Keep `ShimSpec`, but split auth/session persistence, transport/retry helpers, and per-provider adapters before the next provider addition grows shared branching again. |
| **Status** | Open |

Evidence: one file owns credential loading, token refresh, retry logic, wire-format translation, SSE parsing, Bedrock signing integration, provider discovery, and six shim implementations.

---

## M2 · `tools.py` is now a second monolith mixing nine tools, HTTP fetch, archive parsing, regex search, and SLOC

| | |
|---|---|
| **Location** | `oy_cli/tools.py` (1,138 code lines / 1,480 total) |
| **Category** | Complexity |
| **Reference** | [grugbrain.dev](https://grugbrain.dev/) |
| **Recommendation** | Split at least into filesystem tools, network tools, and repo-analysis helpers; keep shared path/ignore/budget logic in one small boundary module. |
| **Status** | Open (new hotspot) |

Evidence: `tools.py` now combines tool schema generation, `bash`, `webfetch`, `.gitignore` walking, archive readers, threaded search/replace, and `pygount` integration.

---

## M3 · Provider HTTP clients are created repeatedly and never explicitly closed

| | |
|---|---|
| **Location** | `oy_cli/providers.py:499-507`, `oy_cli/runtime.py:636-639`, `oy_cli/agent.py:374-382` |
| **Category** | Performance |
| **Reference** | [OWASP ASVS 5.0 V13 §13.1.3](https://raw.githubusercontent.com/OWASP/ASVS/master/5.0/en/0x22-V13-Configuration.md) |
| **Recommendation** | Cache one client per session/shim, or wrap client creation in explicit lifetime management and call `close()` / `aclose()` when a run ends. |
| **Status** | Open |

Evidence: `_openai_client_pair()` creates owned `httpx` clients; `run_agent()` asks `rt.get_client(model)` for each run/turn; the repo has no matching client close path. Long chat sessions can accumulate connection pools until GC happens.

---

## M4 · `search` builds the full match set before applying `limit`

| | |
|---|---|
| **Location** | `oy_cli/tools.py:818-866`, `oy_cli/tools.py:1112-1153` |
| **Category** | Performance |
| **Reference** | [OWASP ASVS 5.0 V15 §15.1.3](https://raw.githubusercontent.com/OWASP/ASVS/master/5.0/en/0x24-V15-Secure-Coding-and-Architecture.md), [grugbrain.dev](https://grugbrain.dev/) |
| **Recommendation** | Add a global match budget / early stop, and let the tool stream or cut work once the displayed limit is satisfied. |
| **Status** | Open |

Evidence: `search()` appends every `SearchMatch` across all files and archives; only `_search_payload()` slices to the user-visible `limit`. Broad regexes on large repos pay the full memory and CPU cost anyway.

---

## M5 · Archive scanning has no explicit decompression bounds

| | |
|---|---|
| **Location** | `oy_cli/tools.py:802-816`, `oy_cli/tools.py:820-865` |
| **Category** | Performance |
| **Reference** | [OWASP ASVS 5.0 V13 §13.1.3](https://raw.githubusercontent.com/OWASP/ASVS/master/5.0/en/0x22-V13-Configuration.md), [OWASP ASVS 5.0 V15 §15.2.2](https://raw.githubusercontent.com/OWASP/ASVS/master/5.0/en/0x24-V15-Secure-Coding-and-Architecture.md) |
| **Recommendation** | Cap archive size, member count, and decompressed bytes, or disable archive scanning by default and require an explicit opt-in. |
| **Status** | Open |

Evidence: `_streams()` walks zip/tar/gz/bz2/xz/zst inputs, and `search()` can fan them out across up to 32 threads. A malicious or accidental giant archive in the repo can turn `search`/`audit` into a decompression DoS.

---

## M6 · `tool_read()` loads whole files into memory just to return a small slice

| | |
|---|---|
| **Location** | `oy_cli/tools.py:1060-1077` |
| **Category** | Performance |
| **Reference** | [OWASP ASVS 5.0 V15 §15.1.3](https://raw.githubusercontent.com/OWASP/ASVS/master/5.0/en/0x24-V15-Secure-Coding-and-Architecture.md) |
| **Recommendation** | Stream line-by-line until `offset + limit` is satisfied instead of `read_text().splitlines()` on the full file. |
| **Status** | Open |

Evidence: `tool_read()` resolves the target, then reads the entire file and splits it before applying `offset`/`limit`. Large logs or generated files make a simple preview request unnecessarily expensive.

---

## M7 · Provider discovery and model listing are both slow and opaque

| | |
|---|---|
| **Location** | `oy_cli/providers.py:1248-1250`, `oy_cli/providers.py:1271-1289`, `oy_cli/providers.py:1502-1513`, `oy_cli/providers.py:1574-1579`, `oy_cli/runtime.py:698-708` |
| **Category** | Complexity / Performance |
| **Reference** | [OWASP ASVS 5.0 V13 §13.1.3](https://raw.githubusercontent.com/OWASP/ASVS/master/5.0/en/0x22-V13-Configuration.md), [OWASP ASVS 5.0 V16 §16.5](https://raw.githubusercontent.com/OWASP/ASVS/master/5.0/en/0x25-V16-Security-Logging-and-Error-Handling.md) |
| **Recommendation** | Memoize shim availability for the process lifetime, parallelize model listing with bounded concurrency, and surface one-line warnings instead of silently dropping exceptions into `[]`. |
| **Status** | Open |

Evidence: shim discovery is serial and may call AWS CLI or `gh auth token`; model listing is also serial; `_shim_env_error()` and `list_models_for_shim()` broadly catch `Exception`, so failures often disappear instead of explaining why a shim vanished.

---

## M8 · Codex ChatGPT still uses a bespoke retry/auth loop outside the shared retry plumbing

| | |
|---|---|
| **Location** | `oy_cli/providers.py:612-643`, `oy_cli/providers.py:1124-1168` |
| **Category** | Complexity |
| **Reference** | [OWASP ASVS 5.0 V13 §13.1.3](https://raw.githubusercontent.com/OWASP/ASVS/master/5.0/en/0x22-V13-Configuration.md), [grugbrain.dev](https://grugbrain.dev/) |
| **Recommendation** | Move the Codex SSE path onto `_call_with_retry()` so it inherits the same backoff, retry-after handling, and UI retry reporting as the other providers. |
| **Status** | Open |

Evidence: the general retry path already exists, but `_codex_chatgpt_client()` still runs its own `for attempt in range(2)` loop with inline 401 handling and separate HTTP exception translation.

---

## Resolved or improved since the previous audit

| Item | Status | Notes |
|---|---|---|
| Private config/session/debug directories | **Resolved** | Directory creation now hardens to `0o700`, and files are re-`chmod`'d to `0o600` (`oy_cli/runtime.py:350-362`, `oy_cli/providers.py:188-196`, covered in `tests/test_oy_cli.py:661-780`). |
| Giant `__init__.py` implementation hub | **Improved** | Runtime logic now lives mainly in `agent.py`, `cli.py`, `runtime.py`, and `tools.py`; `__init__.py` is back to a thin re-export facade. |
| Bedrock signing mixed into provider glue | **Improved** | SigV4 bearer-token logic now lives in `oy_cli/aws_sigv4.py`, which is smaller and directly testable. |
| Provider redirect following | **Still resolved** | Shared provider HTTP clients continue to default to `follow_redirects=False` (`oy_cli/providers.py:284-292`). |
| Thread-safety around reasoning support cache | **Still resolved** | Cache access remains guarded by `_REASONING_CACHE_LOCK` (`oy_cli/providers.py:753-764`). |

## Short audit log

- 2026-03-25: refreshed for commit `37275d2`, version 0.4.2.
  - Cross-checked against OWASP ASVS 5.0.0 (May 2025 release): V1, V8, V13, V14, V15, V16.
  - Cross-checked against [grugbrain.dev](https://grugbrain.dev/) for simplicity pressure and “same thing done same way” guidance.
  - Main changes vs the previous audit: directory-permission hardening is now in place; `__init__.py` was significantly slimmed; Bedrock signing moved into `aws_sigv4.py`.
  - New or sharper findings: H2 (`/ask` is not truly read-only), H3 (`webfetch` redirect validation gap), M3 (provider client lifecycle leak), M5 (archive scan bounds), M7 (slow/opaque shim discovery).
  - Total findings kept to 13: 5 security, 5 complexity, 3 performance.
