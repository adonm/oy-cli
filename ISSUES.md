# Audit Findings

> **Last audit**: 2026-03-28 · commit `25a4e2e` (`Reorganise tests into logical modules`) · cross-checked against [OWASP ASVS 5.0](https://github.com/OWASP/ASVS/tree/master/5.0) and [grugbrain.dev](https://grugbrain.dev/)
>
> **Codebase**: `oy-cli` — local coding CLI with workspace-bound file tools, shell execution, outbound fetch, transcript/debug logging, and 6 provider shims.
>
> | Metric | Value |
> |---|---|
> | Repo files counted by `sloc` | 24 |
> | Python files | 14 package modules |
> | Python LoC | 5,657 code lines |
> | Total repo lines | 8,298 |
> | Largest modules (total lines) | `providers.py` 2,048; `tools.py` 1,906; `runtime.py` 1,310; `cli.py` 756 |
> | Agent tools | 9 (`ask`, `bash`, `list`, `read`, `replace`, `search`, `sloc`, `todo`, `webfetch`) |
> | Provider shims | 6 (`openai`, `codex`, `bedrock-mantle`, `copilot`, `opencode`, `opencode-go`) |
>
> **Audit lens**: dangerous local execution (`bash`), outbound network use (`webfetch` + provider APIs), secret handling, workspace-boundary enforcement, module growth, and obvious latency/memory blow-ups on large repos.

## H1 · `bash` runs `bash -c` with full user authority and inherited credentials

| | |
|---|---|
| **Location** | `oy_cli/tools.py:661-685` |
| **Category** | Security |
| **Reference** | OWASP ASVS 5.0 `1.2.5`, `15.1.5`; grugbrain.dev |
| **Recommendation** | Keep as an explicit trusted-local-user feature only. Add a `--safe` / env-stripped mode, stronger checkpoints for destructive commands, and clearer docs that git/cloud/SSH credentials are inherited. |
| **Status** | Accepted risk / Open |

Evidence: `tool_bash()` calls `rt.require_command_env(...)` and executes `[bash_path, "-c", command]` in the workspace.

---

## H2 · `/ask` is labelled read-only, but still permits outbound network access

| | |
|---|---|
| **Location** | `oy_cli/runtime.py:789`, `oy_cli/session_text.toml:26-28` |
| **Category** | Security |
| **Reference** | OWASP ASVS 5.0 `13.2.4`, `14.2.3` |
| **Recommendation** | Remove `webfetch` from the `/ask` tool set, or rename the mode so network side effects are explicit. |
| **Status** | Open |

Evidence: `_READ_ONLY_TOOLS = {"list", "read", "search", "sloc", "webfetch"}` while `/ask` is presented as "research-only" and "read-only, no changes".

---

## H3 · `webfetch` validates only the initial URL; redirects and re-resolution can bypass the public-IP check

| | |
|---|---|
| **Location** | `oy_cli/tools.py:688-720`, `oy_cli/tools.py:854-892`, `oy_cli/providers.py:483-562` |
| **Category** | Security |
| **Reference** | OWASP ASVS 5.0 `1.5.3`, `13.2.4`, `15.3.2`, `15.4.2` |
| **Recommendation** | Re-validate every redirect target and bind requests to the checked IP, or keep redirects permanently disabled for `webfetch`. Document the limitation if it remains. |
| **Status** | Open |

Evidence: `_validate_url_safe()` checks one `getaddrinfo()` result before the request; `HTTPClient.request()` may re-resolve the host and, when `follow_redirects=True`, follow redirect targets that were never validated.

---

## H4 · Debug logging writes raw prompts, transcript content, and model output without redaction

| | |
|---|---|
| **Location** | `oy_cli/runtime.py:821-850`, `oy_cli/agent.py:340-395` |
| **Category** | Security |
| **Reference** | OWASP ASVS 5.0 `14.2.4`, `16.1.1`, `16.5.1` |
| **Recommendation** | Add secret redaction, retention/rotation controls, and a clearer warning that enabling debug logs may persist file contents and provider responses. |
| **Status** | Partially resolved |

Evidence: file permissions are hardened to `0o600`, but `_debug_log("request", messages=[...])` and `_debug_log("response", ...)` still serialize full session content.

---

## M1 · `providers.py` remains the dominant complexity hotspot

| | |
|---|---|
| **Location** | `oy_cli/providers.py` (2,048 total lines) |
| **Category** | Complexity |
| **Reference** | OWASP ASVS 5.0 `15.1.5`; grugbrain.dev |
| **Recommendation** | Split transport/retry code, credential/session persistence, model discovery, and per-provider adapters before adding more shims. |
| **Status** | Open |

Evidence: one module owns subprocess auth checks, token refresh, HTTP transport, error translation, Bedrock integration, model listing, and six shim implementations.

---

## M2 · `tools.py` is a second monolith mixing nine tools with archive parsing and HTTP fetch logic

| | |
|---|---|
| **Location** | `oy_cli/tools.py` (1,906 total lines) |
| **Category** | Complexity |
| **Reference** | OWASP ASVS 5.0 `15.1.5`; grugbrain.dev |
| **Recommendation** | Split filesystem tools, network tools, and repo-analysis helpers; keep shared path/ignore/budget code in one narrow boundary module. |
| **Status** | Open |

Evidence: `tools.py` contains tool schema generation, approval flow, `bash`, `webfetch`, `.gitignore` walking, archive readers, threaded search/replace, and `pygount` integration.

---

## P1 · `webfetch` buffers entire responses in memory before truncating or reporting size

| | |
|---|---|
| **Location** | `oy_cli/providers.py:519-562`, `oy_cli/tools.py:854-905` |
| **Category** | Performance |
| **Reference** | OWASP ASVS 5.0 `13.1.3`, `15.1.3`, `15.2.2` |
| **Recommendation** | Add download byte caps and stream responses to a bounded buffer; reject oversized bodies early instead of after full download. |
| **Status** | Open |

Evidence: `HTTPClient.request()` sets `preload_content=True` and copies `raw.data` into `bytes`; `tool_webfetch()` only truncates after the full body is already resident.

---

## P2 · `HTTPClient` has no explicit pool size or back-pressure limits

| | |
|---|---|
| **Location** | `oy_cli/providers.py:483-494` |
| **Category** | Performance |
| **Reference** | OWASP ASVS 5.0 `13.1.2`, `13.1.3`, `13.2.6` |
| **Recommendation** | Configure `PoolManager(maxsize=..., block=True)` and document per-service connection and retry limits. |
| **Status** | Open |

Evidence: `HTTPClient.__init__()` uses `urllib3.PoolManager()` defaults and per-request retry tuning only covers redirect behaviour.

---

## P3 · `search` does all matching work before applying the user-visible limit

| | |
|---|---|
| **Location** | `oy_cli/tools.py:974-1050`, `oy_cli/tools.py:1058-1130` |
| **Category** | Performance |
| **Reference** | OWASP ASVS 5.0 `15.1.3`, `15.2.2`; grugbrain.dev |
| **Recommendation** | Introduce a global match budget and stop workers once enough results are collected for display. |
| **Status** | Open |

Evidence: `_search_file()` appends every hit; only `_search_payload()` slices to `limit` after all files have been scanned.

---

## P4 · Archive and compressed-file scanning has no explicit decompression bounds

| | |
|---|---|
| **Location** | `oy_cli/tools.py:908-972` |
| **Category** | Performance |
| **Reference** | OWASP ASVS 5.0 `13.1.3`, `15.1.3`, `15.2.2` |
| **Recommendation** | Cap archive size, member count, and decompressed bytes, or disable archive scanning by default. |
| **Status** | Open |

Evidence: `_streams()` opens `zip`, `tar`, `gz`, `bz2`, `xz`, and `zst` inputs, and `search()` fans that work out across up to 32 threads.

---

## P5 · `tool_read()` loads the whole file before returning a small slice

| | |
|---|---|
| **Location** | `oy_cli/tools.py:1333-1370` |
| **Category** | Performance |
| **Reference** | OWASP ASVS 5.0 `15.1.3` |
| **Recommendation** | Read line-by-line until `offset + limit` is satisfied instead of `read_text().splitlines()`. |
| **Status** | Open |

Evidence: `tool_read()` calls `target.read_text(...).splitlines()` before applying `offset` and `limit`.

---

## P6 · Model discovery is serial, subprocess-heavy, and hides some failures behind broad `except Exception`

| | |
|---|---|
| **Location** | `oy_cli/providers.py:1697-1730`, `oy_cli/providers.py:1940-2010` |
| **Category** | Complexity / Performance |
| **Reference** | OWASP ASVS 5.0 `13.1.3`, `16.5.2`; grugbrain.dev |
| **Recommendation** | Memoize shim availability for the process lifetime, parallelize model listing, and replace broad exception swallowing with narrower warnings. |
| **Status** | Open |

Evidence: shim detection walks `SHIM_ORDER` serially; Copilot and Mantle checks can spawn `gh` / `aws`; model loading then iterates shims serially and several helpers fall back on broad `except Exception`.

---

## Resolved or improved since the previous audit

| Item | Status | Notes |
|---|---|---|
| Private config/session/debug directories | **Resolved** | Directory creation hardens to `0o700`, files to `0o600`. |
| Giant `__init__.py` implementation hub | **Resolved** | Runtime logic is split across `agent.py`, `cli.py`, `runtime.py`, and `tools.py`. |
| Bedrock signing mixed into provider glue | **Resolved** | SigV4 logic lives in `oy_cli/aws_sigv4.py`. |
| Default redirect behaviour | **Resolved** | Provider and tool HTTP sessions default to `follow_redirects=False`. |
| HTTP client lifecycle leak | **Improved** | `HTTPClient` now has `close()` and context-manager support. |
| Reasoning cache thread safety | **Resolved** | `_REASONING_SUPPORT_CACHE` is guarded by `_REASONING_CACHE_LOCK`. |
| HTTP dependency surface | **Improved** | `httpx` was replaced with `urllib3`, reducing runtime dependencies. |
| Workspace path traversal | **Resolved** | `resolve_path()` enforces workspace boundary at `oy_cli/runtime.py:1130-1133`. |

## Short audit log

- 2026-03-28: refreshed for commit `25a4e2e` (`Reorganise tests into logical modules`).
  - Header updated from current `sloc`: 5,657 Python code lines, 8,298 total repo lines.
  - Updated line numbers for all findings reflecting codebase changes.
  - Noted `tools.py` grew from 1,643 to 1,906 lines; complexity concern deepened.
  - Added workspace path traversal protection to resolved items.
  - All 12 previous findings remain open or partially resolved; no new high-priority issues identified.

- 2026-03-27: refreshed for commit `57498c3`, version `0.4.3b2`.
  - Header updated from current `sloc`: 5,270 Python code lines, 7,845 total repo lines.
  - Revalidated findings against OWASP ASVS 5.0 and grugbrain.dev.
  - Collapsed outbound-fetch SSRF concerns into one higher-signal item covering redirect bypass and DNS/TOCTOU re-resolution.
  - Added explicit finding for full-response buffering in `webfetch` / `HTTPClient`.
  - Prioritised finding count reduced to 10 open items plus 2 accepted/partial items (12 total).
