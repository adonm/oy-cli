# Audit Findings

> **Last audit**: 2026-04-13 · commit `042569a` · logic-focused review against [OWASP ASVS 5.0](https://owasp.org/www-project-application-security-verification-standard/) and [grugbrain.dev](https://grugbrain.dev/)
>
> **Codebase**: `oy-cli` — local coding CLI with workspace-bound file tools, shell execution, outbound fetch, transcript/debug logging, saved sessions, agent profiles, and provider shims.
>
> | Metric | Value |
> |---|---|
> | Repo files counted by `sloc` | 26 |
> | Total code lines | 7,239 |
> | Python code lines | 7,117 |
> | Largest modules | `providers.py` 2,073; `tools.py` 1,868; `runtime.py` 1,652; `cli.py` 1,426 |
>
> **Condensation note**: the highest-value behavioural findings stay detailed below; lower-priority complexity/perf issues are rolled up.

## H1 · `sec-noninteractive-autoapprove` — Non-interactive runs auto-approve mutating tools, including shell execution

| | |
|---|---|
| **Location** | `oy_cli/cli.py:1094-1122`, `oy_cli/tools.py:333-342`, `oy_cli/tools.py:426-431`, `oy_cli/tools.py:637-657` |
| **Category** | Security |
| **Reference** | ASVS 5.0 V1, V15 |
| **Severity** | High |
| **Status** | Open |
| **Exploitability** | High when prompts, repo content, or model output are untrusted; default one-shot `oy "..."` is non-interactive. |
| **Recommendation** | Make shell and other mutating tools explicit opt-ins in non-interactive mode, or keep non-interactive mode read-only until a user-approved checkpoint. |

Evidence: `run()` resolves `interactive=False`; `_approve_mutating_tool()` returns `True` whenever `not interactive`; `tool_bash()` executes `[bash, "-c", command]` once `require_command_env()` passes.

---

## H2 · `sec-readonly-webfetch-exfil` — “Read-only” modes still allow outbound `webfetch`, so secrets can leave the machine

| | |
|---|---|
| **Location** | `oy_cli/cli.py:67`, `oy_cli/runtime.py:915,985`, `oy_cli/session_text.toml:46,56`, `oy_cli/tools.py:735-847` |
| **Category** | Security |
| **Reference** | ASVS 5.0 V12, V14, V15 |
| **Severity** | High |
| **Status** | Open |
| **Exploitability** | Needs the model to see sensitive prompt/file content and then follow hostile instructions; `/ask` and plan mode still have network egress. |
| **Recommendation** | Remove `webfetch` from read-only modes or require a separate network opt-in; if retained, drop custom headers and make “no writes” vs “no egress” explicit in UX. |

Evidence: `_READ_ONLY_TOOLS` includes `webfetch`; `/ask` and plan text allow it; `tool_webfetch()` accepts arbitrary public URLs and caller-supplied headers except a short denylist.

---

## H3 · `sec-webfetch-redirect-ssrf` — `webfetch` checks only the first URL, so redirects can pivot to private targets

| | |
|---|---|
| **Location** | `oy_cli/tools.py:664-688`, `oy_cli/tools.py:830-847`, `oy_cli/providers.py:531-566` |
| **Category** | Security |
| **Reference** | ASVS 5.0 V12 |
| **Severity** | High |
| **Status** | Open |
| **Exploitability** | Requires `follow_redirects=True` and an attacker-controlled public URL that redirects internally. |
| **Recommendation** | Re-validate every redirect hop against the same public-IP policy and pin connections to validated addresses, or keep redirects disabled for `webfetch`. |

Evidence: `_validate_url_safe()` resolves only the original hostname; `HTTPClient.request()` lets `urllib3` follow redirects and never re-checks the destination.

---

## H4 · `sec-symlink-workspace-escape` — Repo symlinks let `search` and audit reads escape the workspace root

| | |
|---|---|
| **Location** | `oy_cli/tools.py:945-960,1032-1050`, `oy_cli/cli.py:525-533,602-611,1030-1037` |
| **Category** | Security |
| **Reference** | ASVS 5.0 V5, V14, V15 |
| **Severity** | High |
| **Status** | Open |
| **Exploitability** | High for untrusted repos: a committed symlink like `loot -> ~/.ssh/id_rsa` or `/etc/shadow` is enough once the model runs `search`, `/ask`, or audit mode. |
| **Recommendation** | Never follow enumerated symlinks during search/audit planning; resolve every discovered path back against the workspace root before opening, and surface symlinks as metadata-only entries. |

Evidence: `_iter_files()` yields symlinked files; `_search_file()` opens them via `_streams(path)`; `_audit_file_tokens()` and `_audit_file_excerpt()` read `(workspace / path).read_text(...)` directly. Only explicit path tools go through `resolve_path()`.

---

## H5 · `sec-devcontainer-docker-sock` — The checked-in devcontainer gives in-container code host-equivalent Docker power

| | |
|---|---|
| **Location** | `.devcontainer/devcontainer.json:1-13` |
| **Category** | Security |
| **Reference** | ASVS 5.0 V13, V15 |
| **Severity** | High |
| **Status** | Accepted risk |
| **Exploitability** | Applies only when contributors use the provided devcontainer; then any shell command inside it can control host Docker. |
| **Recommendation** | Remove the socket by default, gate it behind a separate opt-in profile, and document it as effectively host-root-equivalent. |

Evidence: `.devcontainer/devcontainer.json` bind-mounts `/var/run/docker.sock` into the container.

---

## S1 · `sec-debug-log-secret-retention` — Debug mode writes raw prompts and replies to disk without redaction or retention bounds

| | |
|---|---|
| **Location** | `oy_cli/runtime.py:989-1013`, `oy_cli/agent.py:375-427` |
| **Category** | Security |
| **Reference** | ASVS 5.0 V14, V16 |
| **Severity** | Medium |
| **Status** | Open |
| **Exploitability** | Requires `OY_DEBUG=1` or `/debug`; after that, prompts, file excerpts, and model replies are persisted verbatim. |
| **Recommendation** | Default to metadata-only logs, add redaction plus size/retention controls, and emit an explicit warning when debug logging is enabled. |

Evidence: `_init_debug_log()` creates `~/.config/oy/debug.jsonl`; `_debug_log()` appends raw JSON; `run_turn()` logs full prepared messages and assistant responses.

---

## S2 · `sec-packed-history-system-role` — Packed transcript history is reintroduced as a `system` message

| | |
|---|---|
| **Location** | `oy_cli/agent.py:99-146,204-237`, `oy_cli/session_text.toml:[transcript].packed_history_note` |
| **Category** | Security |
| **Reference** | ASVS 5.0 V15; grugbrain: local reasoning |
| **Severity** | Medium |
| **Status** | Open |
| **Exploitability** | Needs a long-enough session plus attacker-controlled earlier prompt text or assistant-echoed instructions. |
| **Recommendation** | Keep packed history in original roles or a lower-trust data channel; add regression tests proving stale prompt injection cannot influence later tool policy. |

Evidence: `prepared_messages()` calls `_pack_messages_with_toons()`; `_packed_history_note()` serializes earlier `user`/`assistant` text and wraps it in `SystemMessage(...)`. The text says “read-only context”, but the API role still becomes `system`.

---

## S3 · `sec-sigv4-canonicalization-bug` — Custom SigV4 signing mis-handles encoded paths and query parameters

| | |
|---|---|
| **Location** | `oy_cli/aws_sigv4.py:13-19,41-80` |
| **Category** | Security |
| **Reference** | ASVS 5.0 V11, V15 |
| **Severity** | Medium |
| **Status** | Open |
| **Exploitability** | Triggered wherever signed AWS URLs include reserved characters, encoded slashes, repeated keys, or user-controlled object names. |
| **Recommendation** | Replace the custom signer with `botocore`/AWS CRT or implement RFC3986 canonicalization exactly and add regression tests for `%2F`, spaces, `+`, empty values, and repeated keys. |

Evidence: `_canonical_query_string()` splits raw query text on `&` and rejoins without proper percent-encoding before sorting; `_normalize_path()` quotes the full parsed path, double-encoding existing escapes like `%2F -> %252F`.

---

## S4 · `sec-release-workflow-manual-publish` — Manual workflow dispatch can publish arbitrary refs to PyPI

| | |
|---|---|
| **Location** | `.github/workflows/release.yml:3-39` |
| **Category** | Security |
| **Reference** | ASVS 5.0 V13, V15 |
| **Severity** | Medium |
| **Status** | Open |
| **Exploitability** | Requires `workflow_dispatch` rights or compromise of such an account; then a non-release ref can be published through trusted publishing. |
| **Recommendation** | Remove `workflow_dispatch` from the publish path, or add protected-environment approval plus explicit tag/ref/version checks and keep manual runs build-only. |

Evidence: the workflow triggers on both `release.published` and `workflow_dispatch`; `publish` always runs `pypa/gh-action-pypi-publish@release/v1` with `id-token: write` and no ref/tag guard.

---

## P1 · `pf-webfetch-full-buffer` — `webfetch` reads full responses into memory before enforcing output limits

| | |
|---|---|
| **Location** | `oy_cli/providers.py:531-566`, `oy_cli/tools.py:762-878` |
| **Category** | Performance |
| **Reference** | ASVS 5.0 V12, V15 |
| **Severity** | Medium |
| **Status** | Open |
| **Recommendation** | Stream into a bounded buffer and abort once body-size limits are exceeded. |

Evidence: `HTTPClient.request()` uses `preload_content=True` and copies `raw.data` into `bytes`; `tool_webfetch()` truncates only after the whole body is resident.

---

## P2 · `pf-archive-search-unbounded` — `search` decompresses archives and compressed files without byte/member caps

| | |
|---|---|
| **Location** | `oy_cli/tools.py:958-1040` |
| **Category** | Performance |
| **Reference** | ASVS 5.0 V5, V15 |
| **Severity** | Medium |
| **Status** | Open |
| **Recommendation** | Cap archive size, member count, and decompressed bytes, or disable archive scanning by default. |

Evidence: `_streams()` opens `.zip`, tar variants, `.gz`, `.bz2`, `.xz`, and `.zst` with no decompression budget; `search()` can fan that work across many files.

## Concise rollups

| ID | Location | Category | Reference | Severity | Status | Why it matters | Recommendation |
|---|---|---|---|---|---|---|---|
| `cx-providers-monolith` | `oy_cli/providers.py` | Complexity | grugbrain: too much abstraction; ASVS 5.0 V15 | Medium | Open | One 2k-line module owns transport, retries, credential IO, model discovery, and all shims, which makes trust-boundary review and safe changes harder. | Split transport/auth/model-discovery/provider adapters into smaller modules with narrow invariants. |
| `cx-tools-monolith` | `oy_cli/tools.py` | Complexity | grugbrain: local reasoning | Medium | Open | One file mixes approvals, shell, filesystem, network, archives, and repo analysis, so policy and sink changes are easy to miss. | Separate approval policy, filesystem helpers, network fetch, and repo-analysis code. |
| `cx-model-discovery-fail-fast` | `oy_cli/runtime.py:1517-1535`, `oy_cli/providers.py:1945-1963` | Complexity | grugbrain: boring code; ASVS 5.0 V15 | Medium | Open | Model discovery is serial, subprocess-heavy, and aborts on the first broken shim, causing brittle startup and poor operator visibility. | Cache shim availability, parallelize safe probes, and return partial results instead of failing on one backend. |
| `pf-search-limit-late` | `oy_cli/tools.py:1053-1088`, `oy_cli/tools.py:1420-1528` | Performance | grugbrain: complexity very bad | Medium | Open | `search()` collects full worker result sets before trimming to the requested limit, so hostile repos can force unnecessary work and memory use. | Add a global match budget and stop workers once the visible result limit is satisfied. |

## Resolved or improved since earlier audits

| Item | Status | Note |
|---|---|---|
| Release workflow action refs | Resolved | Workflow already uses current major tags. |
| Private config/session/debug permissions | Resolved | Directories harden to `0o700`, files to `0o600`. |
| Default redirect behaviour | Resolved | Redirects now default off; explicit opt-in is still risky because redirect targets are not revalidated. |
| HTTP client lifecycle leak | Improved | `HTTPClient` now supports `close()` and context-manager cleanup. |
| Explicit workspace path traversal | Resolved | `resolve_path()` and glob validation keep direct file-tool paths inside the workspace root. |
| Streaming file reads | Resolved | `tool_read()` stops once `offset + limit` is satisfied. |

## Short audit log

- 2026-04-13: refreshed for commit `042569a`.
  - Revalidated the open findings against current runtime behaviour.
  - Inspected `.tmp/renovate-2026-04-14.json`: no actionable dependency or Actions update risk; only a local RE2 fallback warning.
  - Final condensation keeps the highest-risk behavioural bugs detailed and collapses lower-priority maintainability/perf findings into rollups.
