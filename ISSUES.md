# Audit Findings

> **Last audit**: 2026-04-03 · commit `5e8a8e3` (`Retry malformed model outputs`) · cross-checked against [OWASP ASVS 5.0](https://github.com/OWASP/ASVS/tree/master/5.0) and [grugbrain.dev](https://grugbrain.dev/)
>
> **Codebase**: `oy-cli` — local coding CLI with workspace-bound file tools, shell execution, outbound fetch, transcript/debug logging, session save/load, and 6 provider shims.
>
> | Metric | Value |
> |---|---|
> | Repo files counted by `sloc` | 26 |
> | Python files | 14 |
> | Python LoC | 6,314 code lines |
> | Total repo lines | 9,568 |
> | Largest modules (total lines) | `providers.py` 2,203; `tools.py` 1,867; `runtime.py` 1,481; `cli.py` 912 |
> | Agent tools | 9 (`ask`, `bash`, `list`, `read`, `replace`, `search`, `sloc`, `todo`, `webfetch`) |
> | Provider shims | 6 (`openai`, `codex`, `bedrock-mantle`, `copilot`, `opencode`, `opencode-go`) |
>
> **Audit lens**: dangerous local execution (`bash`), outbound network use (`webfetch` + provider APIs), secret handling, workspace-boundary enforcement, module growth, and obvious latency/memory blow-ups on large repos.

## H1 · `bash` runs `bash -c` with full user authority and inherited credentials

| | |
|---|---|
| **Location** | `oy_cli/tools.py:635-660` |
| **Category** | Security |
| **Reference** | OWASP ASVS 5.0 (configuration / verification) |
| **Recommendation** | Keep this as an explicit trusted-local-user feature only. Add a `--safe` / env-stripped mode and stronger checkpoints for destructive commands. |
| **Status** | Accepted risk / Open |

Evidence: `tool_bash()` executes `[bash_path, "-c", command]` with `rt.require_command_env(...)`, so commands inherit the caller's git, cloud, SSH, and other environment credentials.

---

## H2 · `/ask` is still network-capable despite being framed as research-only

| | |
|---|---|
| **Location** | `oy_cli/runtime.py:844-865`, `oy_cli/cli.py:42-43`, `oy_cli/session_text.toml:33-34` |
| **Category** | Security |
| **Reference** | OWASP ASVS 5.0 (API / web service) |
| **Recommendation** | Remove `webfetch` from `/ask`, or rename the mode so the remaining outbound network side effect is explicit. |
| **Status** | Open |

Evidence: `_READ_ONLY_TOOLS` includes `webfetch`, and both CLI help and prompt text say “public webfetch still allowed”.

---

## H3 · `webfetch` validates only the initial URL; redirect/re-resolution can bypass the check

| | |
|---|---|
| **Location** | `oy_cli/tools.py:663-688`, `oy_cli/tools.py:859-864`, `oy_cli/providers.py:516-580` |
| **Category** | Security |
| **Reference** | OWASP ASVS 5.0 (API / web service) |
| **Recommendation** | Re-validate every redirect target and pin requests to the checked IP, or keep redirects permanently disabled for `webfetch`. |
| **Status** | Open |

Evidence: `_validate_url_safe()` resolves and checks the hostname once, but `HTTPClient.request()` later performs the actual request path independently; `tool_webfetch()` exposes `follow_redirects=True` to callers.

---

## H4 · Debug logging still writes raw prompts and model output without redaction

| | |
|---|---|
| **Location** | `oy_cli/runtime.py:868-901`, `oy_cli/agent.py:375-423` |
| **Category** | Security |
| **Reference** | OWASP ASVS 5.0 (logging / verification) |
| **Recommendation** | Add secret redaction plus retention/size controls, and warn more clearly that debug logs can persist prompts, file content, and provider responses. |
| **Status** | Partially resolved |

Evidence: file permissions are hardened, but `_debug_log()` still JSON-serializes full request messages and assistant responses whenever `OY_DEBUG=1`.

---

## M1 · `providers.py` remains the dominant complexity hotspot

| | |
|---|---|
| **Location** | `oy_cli/providers.py` (2,203 total lines; 1,561 code) |
| **Category** | Complexity |
| **Reference** | OWASP ASVS 5.0 (verification); grugbrain.dev |
| **Recommendation** | Split transport/retry code, credential/session loading, model discovery, and per-provider adapters before adding more shims. |
| **Status** | Open |

Evidence: one module owns subprocess auth checks, token refresh, HTTP transport, error translation, model listing, and six shim implementations.

---

## M2 · `tools.py` is still a second monolith mixing nine tools and archive/network logic

| | |
|---|---|
| **Location** | `oy_cli/tools.py` (1,867 total lines; 1,443 code) |
| **Category** | Complexity |
| **Reference** | OWASP ASVS 5.0 (verification); grugbrain.dev |
| **Recommendation** | Split filesystem tools, network tools, and repo-analysis helpers; keep shared path/ignore/budget logic in one narrow boundary module. |
| **Status** | Open |

Evidence: `tools.py` contains tool schema/approval flow, `bash`, `webfetch`, `.gitignore` walking, archive readers, threaded search/replace, and `pygount` integration.

---

## P1 · `webfetch` buffers entire responses in memory before truncating output

| | |
|---|---|
| **Location** | `oy_cli/providers.py:516-580`, `oy_cli/tools.py:859-878` |
| **Category** | Performance |
| **Reference** | OWASP ASVS 5.0 (API / web service) |
| **Recommendation** | Stream into a bounded buffer and reject oversized bodies early instead of after full download. |
| **Status** | Open |

Evidence: `HTTPClient.request()` sets `preload_content=True` and copies `raw.data` into `bytes`; `tool_webfetch()` summarizes only after the full body is already resident.

---

## P2 · `HTTPClient` has no explicit pool size or back-pressure limits

| | |
|---|---|
| **Location** | `oy_cli/providers.py:516-563` |
| **Category** | Performance |
| **Reference** | OWASP ASVS 5.0 (API / web service) |
| **Recommendation** | Configure `PoolManager(maxsize=..., block=True)` and document per-service connection limits. |
| **Status** | Open |

Evidence: `HTTPClient.__init__()` uses bare `urllib3.PoolManager()` defaults and only tunes redirects/timeouts per request.

---

## P3 · `search` does all matching work before applying the visible result limit

| | |
|---|---|
| **Location** | `oy_cli/tools.py:1022-1085`, `oy_cli/tools.py:1419-1474` |
| **Category** | Performance |
| **Reference** | OWASP ASVS 5.0 (verification); grugbrain.dev |
| **Recommendation** | Add a global match budget and stop workers once enough results are collected for display. |
| **Status** | Open |

Evidence: `search()` collects every batch into `results`, and `_search_payload()` slices to `limit` only when formatting the payload.

---

## P4 · Archive and compressed-file scanning has no explicit decompression bounds

| | |
|---|---|
| **Location** | `oy_cli/tools.py:957-974`, `oy_cli/tools.py:1022-1048` |
| **Category** | Performance |
| **Reference** | OWASP ASVS 5.0 (API / web service / availability) |
| **Recommendation** | Cap archive size, member count, and decompressed bytes, or disable archive scanning by default. |
| **Status** | Open |

Evidence: `_streams()` opens `zip`, `tar`, `gz`, `bz2`, `xz`, and `zst` inputs with no explicit size/member limits, and search fans that work out across worker threads.

---

## M3 · Model discovery is serial, subprocess-heavy, and still hides failures behind broad `except Exception`

| | |
|---|---|
| **Location** | `oy_cli/providers.py:1862-1926`, `oy_cli/providers.py:2071-2084`, `oy_cli/runtime.py:1357-1370`, `oy_cli/agent.py:495-515` |
| **Category** | Complexity |
| **Reference** | OWASP ASVS 5.0 (verification); grugbrain.dev |
| **Recommendation** | Memoize shim availability for the process lifetime, parallelize model listing, and replace broad exception swallowing with narrower warnings. |
| **Status** | Open |

Evidence: `detect_available_shims()` walks `SHIM_ORDER` serially; Copilot checks can spawn `gh`; model listing loops shims one by one; several paths fall back through broad `except Exception` and silently degrade.

---

## S1 · Release workflow action refs lag current major versions

| | |
|---|---|
| **Location** | `.github/workflows/release.yml:15-39` |
| **Category** | Security |
| **Reference** | OWASP ASVS 5.0 (verification / dependency hygiene) |
| **Recommendation** | Bump `actions/checkout` to `v6`, `actions/setup-python` to `v6`, `actions/upload-artifact` to `v7`, and `actions/download-artifact` to `v8`; keep Renovate or equivalent lookup in CI. |
| **Status** | Open |

Evidence: local Renovate lookup on 2026-04-03 surfaced 4 GitHub Actions major updates; no `pep621` package updates or vulnerability metadata were reported.

## Resolved or improved since earlier audits

| Item | Status | Notes |
|---|---|---|
| Private config/session/debug directories | **Resolved** | Directory creation hardens to `0o700`, files to `0o600`. |
| Bedrock signing split out | **Resolved** | SigV4 logic lives in `oy_cli/aws_sigv4.py`. |
| Default redirect behaviour | **Resolved** | Provider and tool HTTP sessions default to `follow_redirects=False`; explicit opt-in remains risky. |
| HTTP client lifecycle leak | **Improved** | `HTTPClient` now has `close()` and context-manager support. |
| Reasoning cache thread safety | **Resolved** | `_REASONING_SUPPORT_CACHE` is guarded by `_REASONING_CACHE_LOCK`. |
| HTTP dependency surface | **Improved** | `httpx` was replaced with `urllib3`, reducing runtime dependencies. |
| Workspace path traversal | **Resolved** | `resolve_path()` enforces the workspace boundary. |
| Streaming file reads | **Resolved** | `tool_read()` stops once `offset + limit` is satisfied instead of loading the whole file. |

## Short audit log

- 2026-04-03: refreshed for commit `5e8a8e3` (`Retry malformed model outputs`).
  - Header updated from current `sloc`: 6,314 Python code lines, 9,568 total repo lines.
  - Revalidated findings against OWASP ASVS 5.0 and grugbrain.dev.
  - Ran local Renovate lookup: 0 `pep621` updates surfaced; 4 GitHub Actions major updates surfaced; no vulnerability metadata was present.
  - Previous findings remain open or partially resolved; no new workspace-boundary regressions found.

- 2026-04-02: refreshed for commit `c77e07e` (`Tighten docs and collapse small helpers`).
  - Header updated from `sloc`: 6,339 Python code lines, 9,503 total repo lines.
  - Revalidated findings against OWASP ASVS 5.0 and grugbrain.dev.
  - Ran local Renovate lookup: 0 `pep621` updates surfaced; 4 GitHub Actions major updates surfaced; no vulnerability metadata was present in the report.
  - Previous findings remain open or partially resolved; no new path-boundary regressions found.

- 2026-03-28: refreshed for commit `25a4e2e` (`Reorganise tests into logical modules`).
  - Header updated from `sloc`: 5,657 Python code lines, 8,298 total repo lines.
  - Updated line numbers for all findings reflecting codebase changes.
  - Noted `tools.py` growth from 1,643 to 1,906 lines; complexity concern deepened.
  - Added workspace path traversal protection to resolved items.
