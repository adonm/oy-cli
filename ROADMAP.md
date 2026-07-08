# Roadmap

`oy` has pivoted from a standalone AI coding CLI to an opencode launcher with deterministic MCP helpers. The old roadmap for the native LLM/tool stack is complete and obsolete.

## Current Priorities

### 1. Harden Setup

- Validate generated global and workspace `opencode.json` files against `https://opencode.ai/config.json` in tests.
- Add idempotency coverage for `oy setup`.
- Preserve user-authored config while updating only generated oy entries.
- Document restart requirements clearly after setup changes.

### 2. Test MCP Protocol Behavior

- Add tests for `initialize`, `tools/list`, and representative `tools/call` requests.
- Add fixture tests for `repo_manifest`, `repo_chunks`, and `git_diff_input`.
- Add report-rendering fixture tests for markdown and SARIF.

### 3. Improve Generated Agents

- Use `docs/evaluation.md` to test prompt changes against pinned public OSS projects before merging substantial prompt edits.
- Expand the seed corpus with more recall canaries, regression diffs, and precision baselines.
- Track scorecards by behavior: recall, precision, evidence quality, actionability, cost, and protocol compliance.
- Keep agents explicit about using edit/bash tools for changes and oy MCP only for deterministic inputs/reports.
- Add examples for `/oy-audit`, `/oy-review`, and `/oy-enhance` workflows after the evaluation corpus stabilizes.

### 4. Keep The Native Surface Small

- Do not reintroduce provider routing, native chat/session state, or a model tool loop.
- Remove dependencies when deterministic helpers no longer need them.
- Prefer host built-ins over new oy MCP tools.

### 5. Track opencode 2.0 Integration

- Wait for a tagged/stable opencode 2.0 release before making it the default path.
- Prefer v2 HTTP/SSE APIs over CLI flag compatibility: create sessions with `agent: "oy"`, run audit/review/enhance through `session.command`, and subscribe to events.
- Use `opencode serve --stdio` as an optional sidecar/embed mode; avoid linking opencode into the Rust binary.
- Emit native v2 config when available (`agents`, `commands`, `permissions`, `mcp.servers`) while keeping v1-compatible setup until v2 stabilizes.
- Keep oy MCP as the deterministic helper boundary; consider v2 custom tools/plugins only if MCP becomes a blocker.

## Non-Goals

- Rebuilding host features inside `oy`.
- Adding shell, edit, webfetch, or repo clone to `oy mcp`.
- Persisting chat/session/model state in `oy`.
- Running provider-backed LLM evaluations in default CI.
