# Go Port Tracker

## Goals

- [ ] Full feature parity with the current Python CLI
- [ ] Smaller single-binary distribution
- [ ] Modern, low-bloat dependency set
- [ ] Preserve security boundaries and workspace confinement
- [ ] Preserve provider/tool behavior and operator UX

## Baseline

- Source baseline: Python implementation in `oy_cli/`
- Test baseline: `67` passing pytest tests via `uv run pytest -q`
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
- [ ] M10: retire Python implementation after parity verification

## Parity checklist

### Commands

- [ ] `oy "prompt"`
- [ ] `oy run`
- [ ] `oy chat`
- [ ] `oy ralph`
- [ ] `oy audit`
- [ ] `oy model`
- [ ] chat commands: `/help`, `/tokens`, `/model`, `/debug`, `/yolo`, `/ask`, `/audit`, `/save`, `/load`, `/undo`, `/clear`, `/quit`

### Providers and auth

- [ ] OpenAI-compatible Responses API
- [ ] OpenAI-compatible Chat Completions API
- [ ] Codex auth and model handling
- [ ] Bedrock Mantle SigV4 and model loading
- [ ] Copilot auth/model detection
- [ ] OpenCode Zen
- [ ] OpenCode Go
- [ ] retry and reasoning fallback behavior

### Tools

- [ ] `list`
- [ ] `read`
- [ ] `search`
- [ ] `replace`
- [ ] `sloc`
- [ ] `bash`
- [ ] `webfetch`
- [ ] `ask`
- [ ] `todo`
- [ ] mutating tool approval flow
- [ ] read-only tool mode for `/ask`

### Runtime and UX

- [ ] embedded system prompt + tool descriptions
- [ ] config persistence
- [ ] model discovery/selection
- [ ] best-of defaults
- [ ] context budgeting and truncation
- [ ] saved sessions
- [ ] debug logging
- [ ] workspace path confinement

### Verification

- [ ] `go test ./...`
- [ ] `go build ./cmd/oy`
- [ ] compare major CLI flows against Python behavior

## Progress log

- 2026-04-03: Baseline inspected. Python tests pass via `uv run pytest -q`. Started Go port scaffold and tracker.
- 2026-04-03: Added initial Go module scaffold, shared types, runtime helpers, SigV4 signing, and first Go parity tests.
- 2026-04-03: Ported embedded prompt loading, model config helpers, shim registry plumbing, JSON/file helpers, HTTP response utilities, and added Go parity tests for runtime/providers.
- 2026-04-03: Ported Go transcript primitives and local tool foundations (`todo`, `bash`, `list`, `read`, `search`, `replace`, `sloc`, web payload helpers) with focused Go tests.
- 2026-04-03: Ported Go CLI/session foundations for command normalization, session resolution, Renovate config bootstrap, session save/load, and chat command handling with focused Go CLI tests.
- 2026-04-03: Added Go tool registry/spec selection, mutating approval flow, `ask`/`todo` invocation plumbing, and real `webfetch` request handling with focused parity tests.
- 2026-04-03: Switched README, contributor workflow, and release automation to Go-first while keeping Python pytest checks as parity verification during migration.
