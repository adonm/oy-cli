# Audit Findings

> **Last audit**: 2026-04-03 · base commit `8f2e67e` (`Split provider shim hotspot`) · refreshed after the tool fs split · cross-checked against [OWASP ASVS 5.0](https://github.com/OWASP/ASVS/tree/master/5.0) and [grugbrain.dev](https://grugbrain.dev/)
>
> **Codebase**: `oy-cli` — Go local coding CLI with workspace-bound file tools, shell execution, outbound fetch, transcript/debug logging, session save/load, and 6 provider shims.
>
> | Metric | Value |
> |---|---|
> | Repo files counted by `git ls-files` text scan | 39 |
> | Go files | 27 |
> | Go LoC | 5,017 non-comment, non-empty lines |
> | Total repo lines | 7,933 |
> | Largest modules (total lines) | `internal/oy/cli/cli.go` 1,221; `internal/oy/cli/cli_test.go` 592; `internal/oy/runtime/runtime.go` 552; `internal/oy/agent/agent.go` 464 |
> | Agent tools | 9 (`ask`, `bash`, `list`, `read`, `replace`, `search`, `sloc`, `todo`, `webfetch`) |
> | Provider shims | 6 (`openai`, `codex`, `bedrock-mantle`, `copilot`, `opencode`, `opencode-go`) |
>
> **Audit lens**: dangerous local execution (`bash`), outbound network use (`webfetch` + provider APIs), secret handling, workspace-boundary enforcement, module growth, and obvious latency/memory blow-ups on large repos.

## H1 · `bash` runs `bash -c` with full user authority and inherited credentials

| | |
|---|---|
| **Location** | `internal/oy/tools/exec.go:25-38`, `internal/oy/providers/files.go:81-128` |
| **Category** | Security |
| **Reference** | OWASP ASVS 5.0 (configuration / verification) |
| **Recommendation** | Keep this as an explicit trusted-local-user feature only. Add a `--safe` / env-stripped mode and stronger checkpoints for destructive commands. |
| **Status** | Accepted risk / Open |

Evidence: `ToolBash()` builds a shell command as `[bash, "-c", command]`, pulls the ambient environment via `providers.CommandEnv(...)`, and executes it via `providers.RunCmd(...)`, so git, cloud, SSH, and other caller credentials remain in scope.

---

## H2 · `/ask` is still network-capable despite being framed as research-only

| | |
|---|---|
| **Location** | `internal/oy/runtime/runtime.go:51`, `internal/oy/cli/cli.go:64-66`, `internal/oy/runtime/session_text.toml:32` |
| **Category** | Security |
| **Reference** | OWASP ASVS 5.0 (API / web service) |
| **Recommendation** | Remove `webfetch` from `/ask`, or rename the mode so the remaining outbound network side effect stays explicit. |
| **Status** | Open |

Evidence: `ReadOnlyTools` still includes `webfetch`, and both CLI help and prompt text describe `/ask` as no-write while allowing public web access.

---

## H3 · `webfetch` validates only the initial URL; redirect re-resolution can bypass the check

| | |
|---|---|
| **Location** | `internal/oy/tools/web.go:13-75`, `internal/oy/providers/http.go:128-159` |
| **Category** | Security |
| **Reference** | OWASP ASVS 5.0 (API / web service) |
| **Recommendation** | Re-validate every redirect target and pin requests to the checked IP, or keep redirects permanently disabled for `webfetch`. |
| **Status** | Open |

Evidence: `ValidateURLSafe()` resolves and checks the hostname once, but `ToolWebfetch()` can pass `follow_redirects=true`, and `HTTPClient` then follows redirects without repeating the public-IP safety check.

---

## H4 · Debug logging still writes raw prompts and model output without redaction

| | |
|---|---|
| **Location** | `internal/oy/runtime/runtime.go:103-163`, `internal/oy/agent/agent.go:227-275` |
| **Category** | Security |
| **Reference** | OWASP ASVS 5.0 (logging / verification) |
| **Recommendation** | Add secret redaction plus retention/size controls, and warn more clearly that debug logs can persist prompts, file content, tool results, and provider responses. |
| **Status** | Partially resolved |

Evidence: file permissions are hardened, but `runtime.DebugLog()` still serializes the full request message list, chosen assistant response, and tool results whenever `OY_DEBUG=1`.

---

## M1 · Provider shim hotspot split; `protocol.go` is now the main provider complexity center

| | |
|---|---|
| **Location** | `internal/oy/providers/protocol.go` (431 total lines; 295 code), `internal/oy/providers/codex.go` (243 total lines; 164 code), `internal/oy/providers/registry.go` (236 total lines; 153 code) |
| **Category** | Complexity |
| **Reference** | OWASP ASVS 5.0 (verification); grugbrain.dev |
| **Recommendation** | Keep the provider package API flat, but continue trimming `protocol.go` if more request/response conversion or reasoning-fallback behavior lands there. |
| **Status** | Improved |

Evidence: the old `internal/oy/providers/shims.go` monolith is gone; provider code is now split across focused files (`registry.go`, `openai.go`, `codex.go`, `copilot.go`, `bedrock.go`, `protocol.go`, plus the existing `files.go`, `http.go`, and `types.go`). The remaining provider complexity is concentrated in shared request/response conversion and reasoning-fallback helpers inside `protocol.go` instead of one catch-all file.

---

## M2 · Tool package filesystem/search hotspot split is complete

| | |
|---|---|
| **Location** | `internal/oy/tools/helpers.go` (354 total lines; 210 code), `internal/oy/tools/search.go` (195 total lines; 132 code), `internal/oy/tools/list_read.go` (135 total lines; 93 code) |
| **Category** | Complexity |
| **Reference** | OWASP ASVS 5.0 (verification); grugbrain.dev |
| **Recommendation** | Keep the public tool surface flat, but continue trimming shared helpers if more filesystem/search behaviour lands there. |
| **Status** | Resolved |

Evidence: the old `tools.go` and later `fs.go` hotspots are gone; the package now keeps `list`/`read` in `list_read.go`, `search` in `search.go`, `replace` in `replace.go`, and `sloc` in `sloc.go` while preserving the same public tool surface. The remaining shared complexity is smaller and concentrated in reusable helpers rather than one catch-all filesystem file.

---

## P1 · `webfetch` buffers entire responses in memory before truncating output

| | |
|---|---|
| **Location** | `internal/oy/providers/http.go:148-159`, `internal/oy/tools/web.go:64-75` |
| **Category** | Performance |
| **Reference** | OWASP ASVS 5.0 (API / web service) |
| **Recommendation** | Stream into a bounded buffer and reject oversized bodies early instead of after full download. |
| **Status** | Open |

Evidence: `HTTPClient.Request()` calls `io.ReadAll(response.Body)` and materializes the whole body in memory; `ToolWebfetch()` summarizes only after the response is already resident.

---

## P2 · `HTTPClient` has no explicit transport pool or back-pressure limits

| | |
|---|---|
| **Location** | `internal/oy/providers/http.go:123-136` |
| **Category** | Performance |
| **Reference** | OWASP ASVS 5.0 (API / web service) |
| **Recommendation** | Construct an explicit `http.Transport` with documented idle-connection and per-host limits instead of relying on package defaults. |
| **Status** | Open |

Evidence: `NewHTTPClient()` builds a bare `&http.Client{Timeout: timeout}` and does not tune connection pooling or host-level concurrency limits.

---

## P3 · `search` does all matching work before applying the visible result limit

| | |
|---|---|
| **Location** | `internal/oy/tools/search.go:13-54` |
| **Category** | Performance |
| **Reference** | OWASP ASVS 5.0 (verification); grugbrain.dev |
| **Recommendation** | Add a global match budget and stop walking once enough results have been collected for display. |
| **Status** | Open |

Evidence: `ToolSearch()` appends every match into `matches` while walking the tree, and only slices to `limit` after the full scan completes.

---

## P4 · `read` loads the whole file before applying `offset` and `limit`

| | |
|---|---|
| **Location** | `internal/oy/tools/list_read.go:102-133` |
| **Category** | Performance |
| **Reference** | OWASP ASVS 5.0 (availability); grugbrain.dev |
| **Recommendation** | Stream or scan line-by-line until the requested slice is satisfied instead of `os.ReadFile()` on every target. |
| **Status** | Open |

Evidence: `ToolRead()` calls `os.ReadFile(target)` and `splitLines()` before the requested `offset`/`limit` window is applied.

---

## M3 · Model discovery is still serial and can silently degrade

| | |
|---|---|
| **Location** | `internal/oy/providers/registry.go:147-153`, `internal/oy/providers/registry.go:216-233`, `internal/oy/providers/protocol.go:313-326` |
| **Category** | Complexity |
| **Reference** | OWASP ASVS 5.0 (verification); grugbrain.dev |
| **Recommendation** | Memoize shim availability for the process lifetime, parallelize model discovery, and surface degraded fallback paths more explicitly. |
| **Status** | Open |

Evidence: `DetectAvailableShims()` walks `ShimOrder` serially, `ListModelsForShim()` can suppress failures via `ignoreErrors`, and `listModels()` falls back to cached/default lists after provider lookup errors.

---

## S1 · Release workflow action refs likely lag current major versions

| | |
|---|---|
| **Location** | `.github/workflows/release.yml:13-34` |
| **Category** | Security |
| **Reference** | OWASP ASVS 5.0 (verification / dependency hygiene) |
| **Recommendation** | Review and pin the current supported major versions for GitHub Actions in the release workflow, and keep Renovate or an equivalent lookup in CI. |
| **Status** | Open |

Evidence: the workflow still pins `actions/checkout@v4`, `actions/setup-go@v5`, and `actions/upload-artifact@v4`; earlier local Renovate lookup already surfaced GitHub Actions major updates.

## Resolved or improved since earlier audits

| Item | Status | Notes |
|---|---|---|
| Private config/session/debug directories | **Resolved** | `providers.EnsurePrivateDir()` hardens directories to `0o700`, and `runtime.InitDebugLog()` creates debug logs as `0o600`. |
| Bedrock signing split out | **Resolved** | SigV4 logic lives in `internal/oy/aws/sigv4.go`. |
| Default redirect behaviour | **Resolved** | Provider and tool HTTP sessions default to `followRedirects=false`; explicit opt-in remains risky. |
| Reasoning cache thread safety | **Resolved** | `reasoningSupport.mu` guards the reasoning-support cache. |
| HTTP dependency surface | **Improved** | Provider HTTP uses the Go standard library (`net/http`) in `internal/oy/providers/http.go`. |
| Workspace path traversal | **Resolved** | `runtime.ResolvePath()` and tool-level traversal checks enforce workspace confinement. |

## Short audit log

- 2026-04-03: refreshed after splitting the remaining `internal/oy/tools/fs.go` hotspot into focused filesystem/search files.
  - Header updated from a tracked-file text scan: 27 Go files, 5,017 non-comment/non-empty Go lines, 7,933 total repo lines, 39 tracked text files.
  - M2 moved from Improved to Resolved because `fs.go` is gone; `list_read.go`, `search.go`, `replace.go`, and `sloc.go` now carry the split behavior directly.
  - Repointed `read` and `search` findings to `list_read.go` and `search.go`, and updated the largest-module list for the post-split layout.
- 2026-04-03: refreshed after splitting the former `internal/oy/providers/shims.go` hotspot into focused provider files.
  - Header updated from a tracked-file text scan: 22 Go files, 5,767 non-comment/non-empty Go lines, 7,090 total repo lines, 34 tracked text files.
  - M1 moved from Open to Improved because the provider monolith is gone; remaining follow-up is concentrated in `protocol.go` plus a few smaller provider helpers.
  - Repointed stale `shims.go` evidence to `registry.go`, `openai.go`, `codex.go`, `copilot.go`, `bedrock.go`, and `protocol.go`.
- 2026-04-03: refreshed for the Go-only tree after retiring the in-tree Python baseline.
  - Header updated from a tracked-file text scan: 16 Go files, 5,637 non-comment/non-empty Go lines, 6,951 total repo lines, 28 tracked text files.
  - Repointed finding locations and evidence from the retired Python baseline to the current Go implementation.
  - Revalidated open items against OWASP ASVS 5.0 and grugbrain.dev.
  - Release workflow action-version drift remains open; no new workspace-boundary regressions were found.
- 2026-04-03: split the former `internal/oy/tools/tools.go` hotspot into focused tool package files and refreshed issue locations/status.
  - M2 moved from Open to Improved because the monolith was gone; remaining follow-up at that point was concentrated in `fs.go`.
  - Repointed `bash`, `webfetch`, `read`, `search`, and related evidence to `exec.go`, `web.go`, `fs.go`, and `helpers.go`.
  - Header module list reflected the first post-split layout.
