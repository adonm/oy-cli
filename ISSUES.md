# Audit Findings

> **Last audit**: 2026-04-02 · commit `c77e07e` (`Tighten docs and collapse small helpers`) · cross-checked against [OWASP ASVS 5.0](https://github.com/OWASP/ASVS/tree/master/5.0) and [grugbrain.dev](https://grugbrain.dev/)
>
> **Codebase**: `oy-cli` — local coding CLI with workspace-bound file tools, shell execution, outbound fetch, transcript/debug logging, session save/load, and 6 provider shims.
>
> | Metric | Value |
> |---|---|
> | Repo files counted by `sloc` | 25 |
> | Python files | 14 |
> | Python LoC | 6,339 code lines |
> | Total repo lines | 9,503 |
> | Largest modules (total lines) | `providers.py` 2,142; `tools.py` 1,867; `runtime.py` 1,510; `cli.py` 869 |
> | Agent tools | 9 (`ask`, `bash`, `list`, `read`, `replace`, `search`, `sloc`, `todo`, `webfetch`) |
> | Provider shims | 6 (`openai`, `codex`, `bedrock-mantle`, `copilot`, `opencode`, `opencode-go`) |
>
> **Audit lens**: dangerous local execution (`bash`), outbound network use (`webfetch` + provider APIs), secret handling, workspace-boundary enforcement, module growth, and obvious latency/memory blow-ups on large repos.

## H1 · `bash` runs `bash -c` with full user authority and inherited credentials

| | |
|---|---|
| **Location** | `oy_cli/tools.py:635-660` |
| **Category** | Security |
| **Reference** | OWASP ASVS 5.0 `V15.1.5`, `V15.2.5`; grugbrain.dev |
| **Recommendation** | Keep as an explicit trusted-local-user feature only. Add a `--safe` / env-stripped mode, stronger checkpoints for destructive commands, and clearer docs that git/cloud/SSH credentials are inherited. |
| **Status** | Accepted risk / Open |

Evidence: `tool_bash()` resolves `bash` and executes `[bash_path, "-c", command]` with `require_command_env(...)` in the workspace.

---

## H2 · `/ask` is still network-capable despite being presented as research-only / no-write

| | |
|---|---|
| **Location** | `oy_cli/runtime.py:847-868`, `oy_cli/cli.py:41-43`, `oy_cli/session_text.toml:32` |
| **Category** | Security |
| **Reference** | OWASP ASVS 5.0 `V13.2.4`, `V14.2.3` |
| **Recommendation** | Remove `webfetch` from `/ask`, or rename the mode so outbound network side effects are explicit. |
| **Status** | Open |

Evidence: `_READ_ONLY_TOOLS` includes `webfetch`, and chat help says `/ask` allows “public webfetch still allowed”.

---

## H3 · `webfetch` validates only the initial URL; redirects and re-resolution can bypass the public-IP check

| | |
|---|---|
| **Location** | `oy_cli/tools.py:663-864`, `oy_cli/providers.py:515-579` |
| **Category** | Security |
| **Reference** | OWASP ASVS 5.0 `V13.2.4`, `V15.3.2` |
| **Recommendation** | Re-validate every redirect target and bind requests to the checked IP, or keep redirects permanently disabled for `webfetch`. Document the TOCTOU limitation if it remains. |
| **Status** | Open |

Evidence: `_validate_url_safe()` checks `getaddrinfo()` once, but `HTTPClient.request()` may follow redirects and re-resolve the host later.

---

## H4 · Debug logging writes raw prompts, transcript content, and model output without redaction

| | |
|---|---|
| **Location** | `oy_cli/runtime.py:871-904`, `oy_cli/agent.py:468-475`, `oy_cli/agent.py:537-570` |
| **Category** | Security |
| **Reference** | OWASP ASVS 5.0 `V14.2.4`, `V16.1.1`, `V16.5.1` |
| **Recommendation** | Add secret redaction, retention/rotation controls, and a clearer warning that debug logs may persist file contents, prompts, and provider responses. |
| **Status** | Partially resolved |

Evidence: log file permissions are hardened, but `_debug_log("request", messages=[...])` and `_debug_log("response", assistants=[...])` still serialize full session content.

---

## M1 · `providers.py` is still the dominant complexity hotspot

| | |
|---|---|
| **Location** | `oy_cli/providers.py` (2,142 total lines; 1,519 code) |
| **Category** | Complexity |
| **Reference** | OWASP ASVS 5.0 `V15.1.5`; grugbrain.dev |
| **Recommendation** | Split transport/retry code, credential/session persistence, model discovery, and per-provider adapters before adding more shims. |
| **Status** | Open |

Evidence: one module owns subprocess auth checks, token refresh, HTTP transport, error translation, Bedrock integration, model listing, and six shim implementations.

---

## M2 · `tools.py` is a second monolith mixing nine tools with archive parsing and HTTP fetch logic

| | |
|---|---|
| **Location** | `oy_cli/tools.py` (1,867 total lines; 1,443 code) |
| **Category** | Complexity |
| **Reference** | OWASP ASVS 5.0 `V15.1.5`; grugbrain.dev |
| **Recommendation** | Split filesystem tools, network tools, and repo-analysis helpers; keep shared path/ignore/budget code in one narrow boundary module. |
| **Status** | Open |

Evidence: `tools.py` contains tool schema generation, approval flow, `bash`, `webfetch`, `.gitignore` walking, archive readers, threaded search/replace, and `pygount` integration.

---

## P1 · `webfetch` buffers entire responses in memory before truncating or reporting size

| | |
|---|---|
| **Location** | `oy_cli/providers.py:515-579`, `oy_cli/tools.py:859-872` |
| **Category** | Performance |
| **Reference** | OWASP ASVS 5.0 `V13.1.3`, `V15.1.3`, `V15.2.2` |
| **Recommendation** | Add download byte caps and stream responses to a bounded buffer; reject oversized bodies early instead of after full download. |
| **Status** | Open |

Evidence: `HTTPClient.request()` sets `preload_content=True` and copies `raw.data` into `bytes`; `tool_webfetch()` only truncates after the full body is already resident.

---

## P2 · `HTTPClient` has no explicit pool size or back-pressure limits

| | |
|---|---|
| **Location** | `oy_cli/providers.py:515-562` |
| **Category** | Performance |
| **Reference** | OWASP ASVS 5.0 `V13.1.2`, `V13.1.3`, `V13.2.6` |
| **Recommendation** | Configure `PoolManager(maxsize=..., block=True)` and document per-service connection and retry limits. |
| **Status** | Open |

Evidence: `HTTPClient.__init__()` uses bare `urllib3.PoolManager()` defaults and request setup only tunes redirects/retries per call.

---

## P3 · `search` does all matching work before applying the user-visible limit

| | |
|---|---|
| **Location** | `oy_cli/tools.py:1031-1048`, `oy_cli/tools.py:1075-1085`, `oy_cli/tools.py:1431-1473` |
| **Category** | Performance |
| **Reference** | OWASP ASVS 5.0 `V15.1.3`, `V15.2.2`; grugbrain.dev |
| **Recommendation** | Introduce a global match budget and stop workers once enough results are collected for display. |
| **Status** | Open |

Evidence: `_search_file()` appends every hit, and `_search_payload()` slices to `limit` only after the full scan completes.

---

## P4 · Archive and compressed-file scanning has no explicit decompression bounds

| | |
|---|---|
| **Location** | `oy_cli/tools.py:957-974`, `oy_cli/tools.py:1031-1048` |
| **Category** | Performance |
| **Reference** | OWASP ASVS 5.0 `V13.1.3`, `V15.1.3`, `V15.2.2` |
| **Recommendation** | Cap archive size, member count, and decompressed bytes, or disable archive scanning by default. |
| **Status** | Open |

Evidence: `_streams()` opens `zip`, `tar`, `gz`, `bz2`, `xz`, and `zst` inputs, and search fans that work out across a thread pool.

---

## P5 · `best_of` fan-out is unbounded and can explode cost, latency, and provider load

| | |
|---|---|
| **Location** | `oy_cli/runtime.py:1243-1256`, `oy_cli/agent.py:507-524` |
| **Category** | Performance / Complexity |
| **Reference** | OWASP ASVS 5.0 `V13.1.3`, `V15.1.3`, `V15.2.2` |
| **Recommendation** | Clamp `best_of` to a small safe maximum per model/provider and surface the effective cap in the UI. |
| **Status** | Open |

Evidence: `OY_BEST_OF` accepts any positive integer, and `run_turn()` creates `ThreadPoolExecutor(max_workers=best_of)` parallel completions.

---

## P6 · Model discovery is serial, subprocess-heavy, and still hides failures behind broad `except Exception`

| | |
|---|---|
| **Location** | `oy_cli/providers.py:1792-1905`, `oy_cli/providers.py:2010-2104`, `oy_cli/runtime.py:1377-1392` |
| **Category** | Complexity / Performance |
| **Reference** | OWASP ASVS 5.0 `V13.1.3`, `V16.5.2`; grugbrain.dev |
| **Recommendation** | Memoize shim availability for the process lifetime, parallelize model listing, and replace broad exception swallowing with narrower warnings. |
| **Status** | Open |

Evidence: shim detection walks `SHIM_ORDER` serially; Copilot and Mantle checks can spawn `gh` / `aws`; multiple helpers fall back on bare `except Exception` and silently return empty results.

---

## S1 · Release workflow action refs lag current major versions

| | |
|---|---|
| **Location** | `.github/workflows/release.yml:15-16`, `.github/workflows/release.yml:23-36`, `renovate-report.json` (local, untracked) |
| **Category** | Security |
| **Reference** | OWASP ASVS 5.0 `V15.1.1`, `V15.2.1` |
| **Recommendation** | Bump `actions/checkout` to `v6`, `actions/setup-python` to `v6`, `actions/upload-artifact` to `v7`, and `actions/download-artifact` to `v8`; keep Renovate or equivalent lookup in CI so the release workflow does not drift. |
| **Status** | Open |

Evidence: `.github/workflows/release.yml` still uses `actions/checkout@v4`, `actions/setup-python@v5`, `actions/upload-artifact@v4`, and `actions/download-artifact@v4`; local untracked Renovate lookup output and current upstream releases point to newer majors (`v6.0.2`, `v6.2.0`, `v7.0.0`, `v8.0.1`). No `pep621` package updates were surfaced.

## Resolved or improved since earlier audits

| Item | Status | Notes |
|---|---|---|
| Private config/session/debug directories | **Resolved** | Directory creation hardens to `0o700`, files to `0o600`. |
| Giant `__init__.py` implementation hub | **Resolved** | Runtime logic stays split across `agent.py`, `cli.py`, `runtime.py`, and `tools.py`. |
| Bedrock signing mixed into provider glue | **Resolved** | SigV4 logic lives in `oy_cli/aws_sigv4.py`. |
| Default redirect behaviour | **Resolved** | Provider and tool HTTP sessions default to `follow_redirects=False`; explicit opt-in remains risky. |
| HTTP client lifecycle leak | **Improved** | `HTTPClient` has `close()` and context-manager support. |
| Reasoning cache thread safety | **Resolved** | `_REASONING_SUPPORT_CACHE` is guarded by `_REASONING_CACHE_LOCK`. |
| HTTP dependency surface | **Improved** | `httpx` was replaced with `urllib3`, reducing runtime dependencies. |
| Workspace path traversal | **Resolved** | `resolve_path()` enforces the workspace boundary. |
| Streaming file reads | **Resolved** | `tool_read()` stops once `offset + limit` is satisfied instead of loading the whole file. |

## Short audit log

- 2026-04-02: refreshed for commit `c77e07e` (`Tighten docs and collapse small helpers`).
  - Header updated from current `sloc`: 6,339 Python code lines, 9,503 total repo lines.
  - Revalidated findings against OWASP ASVS 5.0 and grugbrain.dev.
  - Ran local Renovate lookup: 0 `pep621` updates surfaced; 4 GitHub Actions major updates surfaced; no vulnerability metadata was present in the report.
  - Added explicit finding for unbounded `best_of` parallel fan-out.
  - Previous findings remain open or partially resolved; no new path-boundary regressions found.

- 2026-03-28: refreshed for commit `25a4e2e` (`Reorganise tests into logical modules`).
  - Header updated from `sloc`: 5,657 Python code lines, 8,298 total repo lines.
  - Updated line numbers for all findings reflecting codebase changes.
  - Noted `tools.py` growth from 1,643 to 1,906 lines; complexity concern deepened.
  - Added workspace path traversal protection to resolved items.

- 2026-03-27: refreshed for commit `57498c3`, version `0.4.3b2`.
  - Header updated from `sloc`: 5,270 Python code lines, 7,845 total repo lines.
  - Revalidated findings against OWASP ASVS 5.0 and grugbrain.dev.
  - Collapsed outbound-fetch SSRF concerns into one higher-signal item covering redirect bypass and DNS/TOCTOU re-resolution.
  - Added explicit finding for full-response buffering in `webfetch` / `HTTPClient`.
