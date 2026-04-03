# Go Port Tracker

## Goals

- [x] Full feature parity with the former Python CLI
- [ ] Smaller single-binary distribution
- [ ] Modern, low-bloat dependency set
- [ ] Preserve security boundaries and workspace confinement
- [ ] Preserve provider/tool behavior and operator UX

## Baseline

- Historical source baseline: retired Python implementation formerly in `oy_cli/`
- Historical test baseline: `67` passing pytest tests via `uv run pytest -q` before retirement
- Current implementation: Go CLI in `cmd/oy/` and `internal/oy/`
- Branch: `golang`
- Go toolchain: `go1.25.5`

## Dependency policy

Target standard library first. Add third-party packages only when they materially reduce complexity or preserve important behavior.

Approved candidates:

- `github.com/BurntSushi/toml` — tiny TOML loader for embedded prompt text
- `github.com/bmatcuk/doublestar/v4` — small glob matcher for `**` patterns

## Milestones

- [x] M1: inspect current Python implementation, tests, and parity targets
- [x] M2: add migration tracker and Go module scaffold
- [x] M3: port shared types, provider primitives, and SigV4
- [x] M4: port runtime/config/model selection helpers
- [x] M5: port tool registry and local file/web/search helpers
- [x] M6: port transcript management and agent loop
- [x] M7: port CLI commands: `run`, `chat`, `ralph`, `model`, `audit`
- [x] M8: add Go tests for provider/runtime/tool/agent/cli parity
- [x] M9: switch docs/build workflow to Go-first
- [x] M10: retire Python implementation after parity verification

## Parity checklist

### Commands

- [x] `oy "prompt"`
- [x] `oy run`
- [x] `oy chat`
- [x] `oy ralph`
- [x] `oy audit`
- [x] `oy model`
- [x] chat commands: `/help`, `/tokens`, `/model`, `/debug`, `/yolo`, `/ask`, `/audit`, `/save`, `/load`, `/undo`, `/clear`, `/quit`

### Providers and auth

- [x] OpenAI-compatible Responses API
- [x] OpenAI-compatible Chat Completions API
- [x] Codex auth and model handling
- [x] Bedrock Mantle SigV4 and model loading
- [x] Copilot auth/model detection
- [x] OpenCode Zen
- [x] OpenCode Go
- [x] retry and reasoning fallback behavior

### Tools

- [x] `list`
- [x] `read`
- [x] `search`
- [x] `replace`
- [x] `sloc`
- [x] `bash`
- [x] `webfetch`
- [x] `ask`
- [x] `todo`
- [x] mutating tool approval flow
- [x] read-only tool mode for `/ask`

### Runtime and UX

- [x] embedded system prompt + tool descriptions
- [x] config persistence
- [x] model discovery/selection
- [x] best-of defaults
- [x] context budgeting and truncation
- [x] saved sessions
- [x] debug logging
- [x] workspace path confinement

### Verification

- [x] `go test ./...`
- [x] `go build ./cmd/oy`
- [x] compare major CLI flows against Python behavior
- [x] remove in-tree Python package/tests and packaging metadata
- [x] reconcile docs and trackers to the Go-only tree

## Progress log

- 2026-04-03: Baseline inspected. Python tests pass via `uv run pytest -q`. Started Go port scaffold and tracker.
- 2026-04-03: Added initial Go module scaffold, shared types, runtime helpers, SigV4 signing, and first Go parity tests.
- 2026-04-03: Ported embedded prompt loading, model config helpers, shim registry plumbing, JSON/file helpers, HTTP response utilities, and added Go parity tests for runtime/providers.
- 2026-04-03: Ported Go transcript primitives and local tool foundations (`todo`, `bash`, `list`, `read`, `search`, `replace`, `sloc`, web payload helpers) with focused Go tests.
- 2026-04-03: Ported Go CLI/session foundations for command normalization, session resolution, Renovate config bootstrap, session save/load, and chat command handling with focused Go CLI tests.
- 2026-04-03: Added Go tool registry/spec selection, mutating approval flow, `ask`/`todo` invocation plumbing, and real `webfetch` request handling with focused parity tests.
- 2026-04-03: Switched README, contributor workflow, and release automation to Go-first while keeping Python pytest checks as parity verification during migration.
- 2026-04-03: Ported the Go agent execution loop, tool-call handling, and self-consistency message selection with focused Go agent parity tests.
- 2026-04-03: Ported working Go provider clients for OpenAI Responses, Chat Completions, and Bedrock Mantle model/chat flows, including reasoning fallback and provider parity tests.
- 2026-04-03: Ported real Go one-shot CLI execution for `run`, `ralph`, and `audit`, wired through the Go agent/providers stack with focused CLI parity tests.
- 2026-04-03: Ported the Go interactive chat/session loop with `/tokens`, `/model`, `/ask`, `/audit`, `/save`, `/load`, `/undo`, `/clear`, and `/quit` handling, plus model/session listing helpers and focused CLI parity tests.
- 2026-04-03: Ported the remaining Go provider/auth parity slice: Codex model-cache/session refresh handling, Codex ChatGPT fallback client, Copilot model classification/routing, OpenCode endpoint alignment, and focused provider parity tests.
- 2026-04-03: Added Go debug logging parity (`OY_DEBUG`, `/debug`, JSONL request/response/tool-result events), and fixed agent tool-state propagation so todo/approval state persists across turns.
- 2026-04-03: Closed Go `list` parity gap for `.` and nested glob patterns, traversal denial, and exclude handling; also aligned the tracker tool checklist with already-ported/tested tool slices.
- 2026-04-03: Added Go CLI parity for `oy chat --yolo` plus interactive/non-interactive model selection behavior, and reconciled tracker checklist items for already-ported prompt/embed and workspace-confinement slices.
- 2026-04-03: Closed a major remaining Go CLI UX parity slice: shared session intro/title rendering, chat git-diff prompt summary, Ralph schedule/progress notes, audit Renovate/focus notes, and focused CLI test coverage; also hardened shim-dependent Go tests against ambient `OY_SHIM` leakage.
- 2026-04-03: Matched Go top-level `--help`, `chat --help`, and `--version` behavior to the in-repo Python baseline, added focused CLI parity tests, and updated the release workflow to embed/verify the binary version via Go ldflags.
- 2026-04-03: Closed another Go parity/documentation slice: added `search` fuzzy-argument plumbing with focused approximate-match coverage, made `sloc` language output generic instead of Python-only, and removed stale Python-specific runtime wording from embedded tool docs.
- 2026-04-03: Retired the in-tree Python baseline (`oy_cli/`, `tests/`, `pyproject.toml`, `uv.lock`, and packaging metadata), rewrote repo docs/trackers for a Go-only tree, and tightened embedded tool descriptions to match the current Go implementation. Marked M10 complete.
- 2026-04-03: Split the former monolithic `internal/oy/tools/tools.go` into focused Go files (`registry.go`, `prompt.go`, `exec.go`, `web.go`, `fs.go`, `schema.go`, `helpers.go`) without changing the public tool surface, kept focused tools/agent/cli tests green, and updated the audit tracker for the new layout.
- 2026-04-03: Split the former monolithic `internal/oy/providers/shims.go` into focused Go files (`registry.go`, `openai.go`, `codex.go`, `copilot.go`, `bedrock.go`, `protocol.go`) while keeping the provider package API flat, preserved existing provider behavior, kept focused provider tests green, and refreshed the audit tracker so M1 now tracks the smaller remaining `protocol.go` hotspot.
- 2026-04-03: Finished the remaining tool-package hotspot split by replacing `internal/oy/tools/fs.go` with focused files (`list_read.go`, `search.go`, `replace.go`, `sloc.go`), preserved the flat public tool API, kept `go test ./...` green, and refreshed the audit tracker so M2 is now resolved.
