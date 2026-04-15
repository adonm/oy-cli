# Audit Issues

> **Last audit**: 2026-04-15 · commit `042569a` · final phase3 rewrite against [OWASP ASVS 5.0](https://owasp.org/www-project-application-security-verification-standard/) and [grugbrain.dev](https://grugbrain.dev/)
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
> **Condensation note**: deduped phase2 inbox. The most exploitable and operationally dangerous issues stay detailed below; lower-signal complexity and perf findings are condensed.

## Detailed findings

### H1 · `sec-noninteractive-autoapprove` — Non-interactive runs auto-approve mutating tools, including shell execution

| | |
|---|---|
| **Location** | `oy_cli/cli.py:1094-1122`, `oy_cli/tools.py:333-342,426-431,637-657` |
| **Category** | Security |
| **Reference** | ASVS 5.0 V1, V15 |
| **Severity** | High |
| **Status** | Open |
| **Exploitability** | High when prompts, repo content, or model output are untrusted; default one-shot `oy "..."` is non-interactive. |
| **Recommendation** | Make shell and other mutating tools explicit opt-ins in non-interactive mode, or keep non-interactive mode read-only until a user-approved checkpoint. |

Evidence: `run()` resolves `interactive=False`; `_approve_mutating_tool()` returns `True` whenever `not interactive`; `tool_bash()` executes `[bash, "-c", command]` once `require_command_env()` passes.

Impact: prompt injection in repo content or upstream model output can move straight from text to local command execution.

### H2 · `sec-readonly-webfetch-exfil` — “Read-only” modes still allow outbound `webfetch`, so secrets can leave the machine

| | |
|---|---|
| **Location** | `oy_cli/cli.py:67`, `oy_cli/runtime.py:915,985`, `oy_cli/session_text.toml:46,56`, `oy_cli/tools.py:735-847` |
| **Category** | Security |
| **Reference** | ASVS 5.0 V12, V14, V15 |
| **Severity** | High |
| **Status** | Open |
| **Exploitability** | Needs the model to see sensitive prompt/file content and then follow hostile instructions; `/ask` and plan mode still have network egress. |
| **Recommendation** | Remove `webfetch` from read-only modes or require a separate network opt-in; if retained, make “no writes” vs “no egress” explicit in UX. |

Evidence: `_READ_ONLY_TOOLS` includes `webfetch`; `/ask` and plan text allow it; `tool_webfetch()` accepts arbitrary public URLs and caller-supplied headers except a short denylist.

Impact: “read-only” does not preserve data locality; secrets from prompts or local files can still be exfiltrated to attacker infrastructure.

### H3 · `sec-webfetch-ssrf` — `webfetch` can still reach private targets via redirects or DNS rebinding

| | |
|---|---|
| **Location** | `oy_cli/tools.py:664-702,830-847`, `oy_cli/providers.py:531-566` |
| **Category** | Security |
| **Reference** | ASVS 5.0 V12 |
| **Severity** | High |
| **Status** | Open |
| **Exploitability** | Requires attacker-controlled DNS or a public URL that redirects internally; redirects need `follow_redirects=True`. |
| **Recommendation** | Resolve once and connect to the validated IP with Host/SNI pinned, and re-validate every redirect hop before sending the request; otherwise keep redirects disabled. |

Evidence: `_validate_url_safe()` only checks the original hostname via `socket.getaddrinfo()` and returns the URL string; `tool_webfetch()` later hands the hostname to `urllib3`; `HTTPClient.request()` can follow redirects and never re-check the destination.

Impact: attacker-controlled DNS or redirectors can bypass the private-IP block and turn `webfetch` into SSRF against loopback, link-local, or RFC1918 services.

### H4 · `sec-symlink-workspace-read-escape` — Repo symlinks let `search` and audit reads escape the workspace root

| | |
|---|---|
| **Location** | `oy_cli/tools.py:945-960,1032-1050`, `oy_cli/cli.py:525-533,602-611,1030-1037` |
| **Category** | Security |
| **Reference** | ASVS 5.0 V5, V14, V15 |
| **Severity** | High |
| **Status** | Open |
| **Exploitability** | High for untrusted repos: a committed symlink like `loot -> ~/.ssh/id_rsa` or `/etc/shadow` is enough once the model runs `search`, `/ask`, or audit mode. |
| **Recommendation** | Do not follow enumerated symlinks during search/audit planning; resolve every discovered path back under the workspace root before opening. |

Evidence: `_iter_files()` yields symlinked files; `_search_file()` opens them via `_streams(path)`; `_audit_file_tokens()` and `_audit_file_excerpt()` read `(workspace / path).read_text(...)` directly. Only explicit path tools use `resolve_path()`.

Impact: opening a malicious repo can expose arbitrary local files to the model or to later outbound requests.

### H5 · `sec-repo-symlink-write-escape` — Audit and Renovate helper writes follow repo symlinks and can clobber files outside the workspace

| | |
|---|---|
| **Location** | `oy_cli/cli.py:395-440,1066-1081,1195-1261,1438-1450` |
| **Category** | Security |
| **Reference** | ASVS 5.0 V5, V14, V15 |
| **Severity** | High |
| **Status** | Open |
| **Exploitability** | High for untrusted repos containing top-level symlinks such as `ISSUES.md -> ~/.bashrc`, `renovate.json -> ~/.gitconfig`, `.gitignore -> ~/.ssh/config`, or `.tmp -> ~/Documents`. |
| **Recommendation** | `lstat` and reject symlinks for auto-managed files and directories, resolve every write target back under the workspace, and use no-follow/atomic-create semantics where available. |

Evidence: `_audit_issues_path()` returns `workspace / "ISSUES.md"` and `_audit_write_issues()` calls `.write_text(...)` directly; audit startup and phase2 helpers reach that sink. `renovate_local()` also auto-writes `renovate.json`, `.gitignore`, and `.tmp/renovate-*.json` under repo-controlled paths without symlink checks.

Impact: `oy audit` or `oy renovate-local` in a malicious repo can overwrite arbitrary user-writable files outside the repo.

### H6 · `sec-local-shim-openai-key-forwarding` — Local shim requests forward `OPENAI_API_KEY` to localhost, and bare model specs can auto-select localhost first

| | |
|---|---|
| **Location** | `oy_cli/providers.py:117-124,154-157,624-630,1800-1824,1945-1962`, `oy_cli/runtime.py:1525-1532,1587-1591` |
| **Category** | Security |
| **Reference** | ASVS 5.0 V12, V14, V15 |
| **Severity** | High |
| **Status** | Open |
| **Exploitability** | Requires a listener on `127.0.0.1:8080` or `:11434` and either explicit `local-*` use or an unqualified model spec/no saved shim. |
| **Recommendation** | Never fall back from `LOCAL_API_KEY` to `OPENAI_API_KEY` for `local-*`; require explicit localhost provider selection; prefer cloud shims over opportunistic localhost auto-detection. |

Evidence: `SHIM_ORDER` prefers `local-8080`/`local-11434`; `resolve_shim()` returns the first detected shim when no prefix is present; `_local_api()` sets `api_key` from `LOCAL_API_KEY or OPENAI_API_KEY`; `_headers()` always emits `Authorization: Bearer ...`.

Impact: any local process answering `/models` and `/responses` can receive full prompts/tool outputs and steal the operator's OpenAI API key.

### H7 · `sec-devcontainer-host-compromise` — The checked-in devcontainer combines a mutable base image with host-equivalent Docker access

| | |
|---|---|
| **Location** | `.devcontainer/devcontainer.json:1-13` |
| **Category** | Security |
| **Reference** | ASVS 5.0 V13, V15 |
| **Severity** | High |
| **Status** | Accepted risk |
| **Exploitability** | Applies when contributors use the provided devcontainer. A compromised or retagged image then runs attacker code inside a container that can control host Docker. |
| **Recommendation** | Remove the Docker socket by default, gate it behind an explicit opt-in profile, and pin the image by immutable digest with provenance checks. |

Evidence: the repo uses `"image": "ghcr.io/wagov-dtt/devcontainer-base"` with no digest, bind-mounts `/var/run/docker.sock`, and runs `onCreateCommand: "docker-init.sh"`.

Impact: opening the repo in the provided devcontainer can escalate a bad image pull into effective host compromise.

### S1 · `sec-debug-log-secret-retention` — Debug mode writes raw prompts and replies to disk without redaction or retention bounds

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

### S2 · `sec-release-workflow-manual-publish` — Manual workflow dispatch can publish arbitrary refs to PyPI

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

### S3 · `sec-sigv4-canonicalization-bug` — Custom SigV4 signing mis-handles encoded paths and query parameters

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

Impact: signed requests can fail or target the wrong canonical resource for valid object names.

### S4 · `sec-packed-history-system-role` — Packed transcript history is reintroduced as a `system` message

| | |
|---|---|
| **Location** | `oy_cli/agent.py:99-146,204-237`, `oy_cli/session_text.toml:[transcript].packed_history_note` |
| **Category** | Security |
| **Reference** | ASVS 5.0 V15; grugbrain: local reasoning |
| **Severity** | Medium |
| **Status** | Open |
| **Exploitability** | Needs a long-enough session plus attacker-controlled earlier prompt text or assistant-echoed instructions. |
| **Recommendation** | Keep packed history in original roles or a lower-trust data channel; add regression tests proving stale prompt injection cannot influence later tool policy. |

Evidence: `prepared_messages()` calls `_pack_messages_with_toons()`; `_packed_history_note()` serializes earlier `user`/`assistant` text and wraps it in `SystemMessage(...)`.

### S5 · `sec-prompt-history-secret-retention` — Interactive prompt history is persisted to disk by default without redaction or retention controls

| | |
|---|---|
| **Location** | `oy_cli/runtime.py:138-143`, `oy_cli/cli.py:1740-1744,2090` |
| **Category** | Security |
| **Reference** | ASVS 5.0 V14, V16 |
| **Severity** | Medium |
| **Status** | Open |
| **Exploitability** | Interactive sessions only; exposure happens through local filesystem access, backups, synced home directories, or later account compromise. |
| **Recommendation** | Make file-backed history opt-in or easy to disable, add retention/clear-history controls, and warn before persisting interactive prompts. |

Evidence: `_history_path()` creates `~/.config/oy/history` with mode `0600`; `_create_prompt_session()` passes `FileHistory(str(history_path))` into the interactive prompt session.

### P1 · `pf-archive-search-unbounded` — `search` decompresses archives and compressed files without byte/member caps

| | |
|---|---|
| **Location** | `oy_cli/tools.py:958-1040` |
| **Category** | Performance |
| **Reference** | ASVS 5.0 V5, V15 |
| **Severity** | Medium |
| **Status** | Open |
| **Recommendation** | Cap archive size, member count, and decompressed bytes, or disable archive scanning by default. |

Evidence: `_streams()` opens `.zip`, tar variants, `.gz`, `.bz2`, `.xz`, and `.zst` with no decompression budget; `search()` can fan that work across many files.

Impact: a repo can force large CPU and memory spikes during search or audit planning with compressed bombs or very large archives.

## Concise findings

| ID | Location | Category | Reference | Severity | Status | Why it matters | Recommendation |
|---|---|---|---|---|---|---|---|
| `pf-webfetch-full-buffer` | `oy_cli/providers.py:531-566`, `oy_cli/tools.py:762-878` | Performance | ASVS 5.0 V12, V15 | Medium | Open | `HTTPClient.request()` uses `preload_content=True` and materializes full response bodies before `tool_webfetch()` truncates, so large responses can cause avoidable memory spikes. | Stream into a bounded buffer and abort once size limits are exceeded. |
| `pf-audit-phase1-rescans-repo` | `oy_cli/cli.py:747-820,987-999` | Performance | grugbrain: complexity very bad; ASVS 5.0 V15 | Medium | Open | Audit startup walks and tokenizes the repo multiple times before reviewing the first chunk, so latency scales with repo size instead of the next 64k chunk. | Reuse the first file walk and defer token estimation until a chunk is actually scheduled. |
| `pf-model-timeout-bypasses-unattended-budget` | `oy_cli/agent.py:378-427`, `oy_cli/tools.py:325-346,646-666,845-871`, `oy_cli/runtime.py:1296` | Performance | ASVS 5.0 V15; grugbrain: local reasoning | Medium | Open | The model can request large `timeout_seconds`; tool execution is not clamped to remaining unattended budget, so CI or non-interactive runs can hang past the advertised limit. | Clamp each tool timeout to remaining unattended budget and a sane hard maximum. |
| `pf-toolcall-token-undercount` | `oy_cli/agent.py:94-102,170-183,270-321,379-387`, `oy_cli/providers.py:1158-1161,1452-1455` | Performance | ASVS 5.0 V15; grugbrain: local reasoning | Medium | Open | Context budgeting ignores serialized assistant `tool_calls`/`thought_signatures`, so tool-heavy sessions can hit avoidable context-length failures. | Budget against serialized provider payloads, not only message text. |
| `pf-audit-context-budget-ignored` | `oy_cli/runtime.py:895-903`, `oy_cli/cli.py:764,996,1712-1719,1996-2004` | Performance | ASVS 5.0 V15; grugbrain: complexity very bad | Medium | Open | Audit planning is hard-coded to 64k-token chunks regardless of actual model context budget, producing oversize chunks on small-context models and extra turns on large-context models. | Derive chunk size from active model context and reserve prompt/tool overhead. |
| `pf-search-limit-late` | `oy_cli/tools.py:1053-1088,1420-1528` | Performance | grugbrain: complexity very bad | Medium | Open | `search()` collects full worker results before trimming to the requested limit, letting hostile repos force unnecessary work and memory use. | Add a global match budget and stop workers once the visible limit is met. |
| `cx-providers-shadowed-timeout-defaults` | `oy_cli/providers.py:146-148,302-304` | Complexity | grugbrain: local reasoning; ASVS 5.0 V15 | Medium | Open | Core timeout constants are defined twice with conflicting values (`120/15/30` then `300/30/60`), so early edits are dead code and review is misleading. | Keep each timeout default in one place and test the effective defaults. |
| `cx-providers-monolith` | `oy_cli/providers.py` | Complexity | grugbrain: too much abstraction; ASVS 5.0 V15 | Medium | Open | One 2k-line module owns transport, retries, credential I/O, model discovery, and provider shims, which makes trust-boundary review and safe changes harder. | Split transport, auth, discovery, and provider adapters into smaller modules with narrow invariants. |
| `cx-tools-monolith` | `oy_cli/tools.py` | Complexity | grugbrain: local reasoning | Medium | Open | One file mixes approvals, shell, filesystem, network, archives, and repo analysis, so policy and sink changes are easy to miss. | Separate approval policy, filesystem helpers, network fetch, and repo-analysis code. |
| `cx-model-discovery-fail-fast` | `oy_cli/runtime.py:1517-1535`, `oy_cli/providers.py:1945-1963` | Complexity | grugbrain: boring code; ASVS 5.0 V15 | Medium | Open | Model discovery is serial, subprocess-heavy, and aborts on the first broken shim, causing brittle startup and poor operator visibility. | Cache shim availability, parallelize safe probes, and return partial results instead of failing on one backend. |
| `cx-runtime-dependency-fanout` | `pyproject.toml:15,25`, `oy_cli/cli.py:15,2475`, `oy_cli/tools.py:35,1168,1835`, `uv.lock:87,101,122,264,297,514,532` | Complexity | grugbrain: small sharp tools; grugbrain: complexity very bad; ASVS 5.0 V15 | Medium | Open | `defopt` and `pygount` pull doc-parser and Git stacks into every install for two small features, increasing install size, CVE churn, and review surface. | Replace them with stdlib or optional dependencies. |

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

- 2026-04-15: phase3 final rewrite for commit `042569a`.
  - Deduped overlapping inbox items: SSRF (`redirect` + `DNS rebinding`) and devcontainer risk (`docker.sock` + floating image`) are now single findings.
  - Kept 12 detailed findings with concrete exploit or failure paths; condensed lower-value complexity/perf items.
  - Inspected newest relevant Renovate report `.tmp/renovate-2026-04-14.json`: no actionable dependency or Actions update risk; only a local RE2 fallback warning.
