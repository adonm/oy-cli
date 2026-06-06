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

- Tighten `oy-auditor`, `oy-reviewer`, and `oy-enhancer` prompts based on real runs.
- Keep agents explicit about using edit/bash tools for changes and oy MCP only for deterministic inputs/reports.
- Add examples for `/oy-audit`, `/oy-review`, and `/oy-enhance` workflows.

### 4. Keep The Native Surface Small

- Do not reintroduce provider routing, native chat/session state, or a model tool loop.
- Remove dependencies when deterministic helpers no longer need them.
- Prefer host built-ins over new oy MCP tools.

## Non-Goals

- Rebuilding host features inside `oy`.
- Adding shell, edit, webfetch, or repo clone to `oy mcp`.
- Persisting chat/session/model state in `oy`.
