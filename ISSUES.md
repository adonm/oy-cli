# Audit Findings

> **Last audit**: 2026-04-13 · commit `042569a` (`Embed richer audit reference guidance`) · cross-checked against [OWASP ASVS 5.0](https://owasp.org/www-project-application-security-verification-standard/) and [grugbrain.dev](https://grugbrain.dev/)
>
> **Codebase**: `oy-cli` — local coding CLI with workspace-bound file tools, shell execution, outbound fetch, transcript/debug logging, saved sessions, agent profiles, and Open Responses-oriented provider shims.
>
> | Metric | Value |
> |---|---|
> | Repo files counted by `sloc` | 26 |
> | Total code lines | 7,239 |
> | Python files | 14 |
> | Python code lines | 7,117 |
> | Total repo lines | 10,890 |
> | Largest modules (total lines) | `providers.py` 2,073; `tools.py` 1,868; `runtime.py` 1,652; `cli.py` 1,426 |
> | Agent tools | 9 (`ask`, `bash`, `list`, `read`, `replace`, `search`, `sloc`, `todo`, `webfetch`) |
> | Shim families | 6 (`openai`, `codex`, `bedrock-mantle`, `copilot`, `opencode`, `local-<port>`) |
>
> **Audit lens**: critical security boundaries first; then material complexity and performance risks that are likely to cause review failure, unsafe defaults, or production-scale stalls.

## H1 · `sec-noninteractive-autoapprove` — Default non-interactive runs auto-approve mutating tools, including shell execution

| | |
|---|---|
| **Location** | `oy_cli/cli.py:1094-1122`, `oy_cli/tools.py:333-342`, `oy_cli/tools.py:426-431`, `oy_cli/tools.py:637-657` |
| **Category** | Security |
| **Reference** | OWASP ASVS 5.0 V1/V15; grugbrain.dev |
| **Severity** | High |
| **Status** | Open |
| **Exploitability** | High if prompts, repo content, or model output are untrusted; default one-shot `oy "..."` is non-interactive. |
| **Recommendation** | Require explicit `--allow-bash` / `--yolo`-style opt-in for shell in non-interactive runs, or keep non-interactive mode read-only / edits-only until a user-approved checkpoint. |

Evidence: `run()` resolves `interactive=False`; `_approve_mutating_tool()` returns `True` whenever `not interactive`; `tool_bash()` executes `[bash, "-c", command]` with `require_command_env()`.

---

## H2 · `sec-readonly-webfetch-exfil` — Read-only modes still permit outbound `webfetch`, creating a direct data-exfil path

| | |
|---|---|
| **Location** | `oy_cli/cli.py:67`, `oy_cli/runtime.py:915,985`, `oy_cli/session_text.toml:46,56`, `oy_cli/tools.py:735-847` |
| **Category** | Security |
| **Reference** | OWASP ASVS 5.0 V12/V14/V15 |
| **Severity** | High |
| **Status** | Open |
| **Exploitability** | Needs the model to see sensitive prompt or file text and then follow a malicious instruction; `/ask` and plan mode still have network egress. |
| **Recommendation** | Remove `webfetch` from read-only modes or require explicit network opt-in; if kept, strip custom headers and make the no-write/not-no-network behavior explicit in UX. |

Evidence: `_READ_ONLY_TOOLS` includes `webfetch`; `/ask` and plan text explicitly allow it; `tool_webfetch()` permits arbitrary public URLs and custom headers except a short denylist.

---

## H3 · `sec-webfetch-redirect-ssrf` — `webfetch` validates only the initial URL, so redirects can bypass the public-host restriction

| | |
|---|---|
| **Location** | `oy_cli/tools.py:664-688`, `oy_cli/tools.py:830-847`, `oy_cli/providers.py:531-566` |
| **Category** | Security |
| **Reference** | OWASP ASVS 5.0 V12 |
| **Severity** | High |
| **Status** | Open |
| **Exploitability** | Requires `follow_redirects=True` plus an attacker-controlled public URL that redirects to a private target. |
| **Recommendation** | Re-validate every redirect target against the same public-IP policy and pin connections to validated addresses, or keep redirects disabled for `webfetch`. |

Evidence: `_validate_url_safe()` resolves and checks only the original hostname; `HTTPClient.request()` enables `urllib3` redirects when `follow_redirects=True` and does not re-check the destination.

---

## H4 · `sec-devcontainer-docker-sock` — The provided devcontainer mounts `docker.sock`, giving container code host-equivalent Docker authority

| | |
|---|---|
| **Location** | `.devcontainer/devcontainer.json:1-13` |
| **Category** | Security |
| **Reference** | OWASP ASVS 5.0 V13/V15 |
| **Severity** | High |
| **Status** | Accepted risk |
| **Exploitability** | Applies only when contributors use the checked-in devcontainer, but then any shell run inside it can control host Docker. |
| **Recommendation** | Document this as host-root-equivalent, remove the mount by default, or gate it behind a separate profile for users who explicitly need Docker control. |

Evidence: `.devcontainer/devcontainer.json` bind-mounts `/var/run/docker.sock` into the container.

---

## S1 · `sec-debug-log-secret-retention` — Debug logging still persists raw prompts and model replies without redaction or retention limits

| | |
|---|---|
| **Location** | `oy_cli/runtime.py:989-1013`, `oy_cli/agent.py:375-427` |
| **Category** | Security |
| **Reference** | OWASP ASVS 5.0 V14/V16 |
| **Severity** | Medium |
| **Status** | Open |
| **Exploitability** | Requires `OY_DEBUG=1` or `/debug`, but then prompts, file excerpts, and model replies are written to disk verbatim. |
| **Recommendation** | Default to metadata-only logging, add secret-pattern redaction plus size/retention controls, and warn explicitly when debug mode is enabled. |

Evidence: `_init_debug_log()` writes `~/.config/oy/debug.jsonl`; `_debug_log()` appends raw JSON; `run_turn()` logs full prepared messages and assistant responses.

---

## C1 · `cx-providers-monolith` — `providers.py` is still a trust-boundary monolith

| | |
|---|---|
| **Location** | `oy_cli/providers.py` (2,073 total lines; 1,448 code) |
| **Category** | Complexity |
| **Reference** | grugbrain.dev; OWASP ASVS 5.0 V1/V16 |
| **Severity** | Medium |
| **Status** | Open |
| **Recommendation** | Split transport/retry, credential storage, model discovery, and per-provider adapters into smaller modules with narrow invariants. |

Evidence: one file still owns subprocess auth probes, HTTP transport, retries, credential loading/saving, model listing, and all shim implementations.

---

## C2 · `cx-tools-monolith` — `tools.py` remains a second monolith spanning approvals, filesystem, network, archives, and repo analysis

| | |
|---|---|
| **Location** | `oy_cli/tools.py` (1,868 total lines; 1,444 code) |
| **Category** | Complexity |
| **Reference** | grugbrain.dev |
| **Severity** | Medium |
| **Status** | Open |
| **Recommendation** | Split approval/policy, filesystem helpers, network fetch, and repo-analysis code into separate modules with explicit interfaces. |

Evidence: `tools.py` mixes mutating-tool approval, shell execution, `webfetch`, globbing, archive readers, threaded search/replace, and `pygount` integration.

---

## C3 · `cx-model-discovery-fail-fast` — Model discovery is serial, subprocess-heavy, and fail-fast on the first broken shim

| | |
|---|---|
| **Location** | `oy_cli/runtime.py:1517-1535`, `oy_cli/providers.py:1945-1963` |
| **Category** | Complexity |
| **Reference** | grugbrain.dev; OWASP ASVS 5.0 V15 |
| **Severity** | Medium |
| **Status** | Open |
| **Recommendation** | Memoize shim availability, parallelize safe model-list probes, and return partial results instead of aborting on the first broken backend. |

Evidence: `detect_available_shims()` walks `SHIM_ORDER` serially; GitHub token lookup shells out; `list_all_model_ids()` loads shims one at a time and raises on the first exception.

---

## P1 · `pf-webfetch-full-buffer` — `webfetch` buffers full responses in memory before truncating output

| | |
|---|---|
| **Location** | `oy_cli/providers.py:531-566`, `oy_cli/tools.py:762-878` |
| **Category** | Performance |
| **Reference** | OWASP ASVS 5.0 V12/V15 |
| **Severity** | Medium |
| **Status** | Open |
| **Recommendation** | Stream into a bounded buffer and fail early once body size limits are exceeded. |

Evidence: `HTTPClient.request()` sets `preload_content=True` and copies `raw.data` into `bytes`; `tool_webfetch()` truncates only after the full body is resident.

---

## P2 · `pf-archive-search-unbounded` — Search scans archives and compressed files without decompression bounds

| | |
|---|---|
| **Location** | `oy_cli/tools.py:958-1040` |
| **Category** | Performance |
| **Reference** | OWASP ASVS 5.0 V5/V15 |
| **Severity** | Medium |
| **Status** | Open |
| **Recommendation** | Cap archive size, member count, and decompressed bytes, or disable archive scanning by default. |

Evidence: `_streams()` opens `.zip`, tar variants, `.gz`, `.bz2`, `.xz`, and `.zst` inputs with no member-count or decompressed-byte limits, and `search()` can fan this out across many files.

---

## P3 · `pf-search-limit-late` — `search` collects the full match set before applying the visible limit

| | |
|---|---|
| **Location** | `oy_cli/tools.py:1053-1088`, `oy_cli/tools.py:1420-1528` |
| **Category** | Performance |
| **Reference** | grugbrain.dev |
| **Severity** | Medium |
| **Status** | Open |
| **Recommendation** | Add a global match budget and stop workers once enough results are collected for display. |

Evidence: `search()` extends `results` with every worker batch, and only `_search_payload()` trims to the requested `limit`.

## Resolved or improved since earlier audits

| Item | Status | Notes |
|---|---|---|
| Release workflow action refs | **Resolved** | `.github/workflows/release.yml` is already on current major action tags. |
| Private config/session/debug directories | **Resolved** | Directory creation hardens to `0o700`, files to `0o600`. |
| Default redirect behaviour | **Resolved** | Provider and tool HTTP sessions default to `follow_redirects=False`; explicit opt-in remains risky because redirect targets are not revalidated. |
| HTTP client lifecycle leak | **Improved** | `HTTPClient` now has `close()` and context-manager support. |
| Workspace path traversal | **Resolved** | `resolve_path()` and glob validation keep file tools inside the workspace root. |
| Streaming file reads | **Resolved** | `tool_read()` stops once `offset + limit` is satisfied rather than loading the whole file. |

## Short audit log

- 2026-04-13: refreshed for commit `042569a` (`Embed richer audit reference guidance`).
  - Header updated from current `sloc`: 7,117 Python code lines, 7,239 total code lines, 10,890 total repo lines.
  - Revalidated findings against OWASP ASVS 5.0 and grugbrain.dev.
  - Inspected `.tmp/renovate-2026-04-14.json`: no actionable dependency or GitHub Actions updates; only Renovate's local RE2 fallback warning was reported.
  - Replaced the earlier generic `bash` accepted-risk wording with the sharper non-interactive auto-approval finding.
  - Main open risks remain non-interactive mutating-tool execution, read-only egress via `webfetch`, redirect-target revalidation gaps, debug-log secret retention, and unbounded search/webfetch work on hostile inputs.
