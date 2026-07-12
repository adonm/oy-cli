# Project direction

`oy` gives OpenCode a focused coding agent and repeatable audit → review → fix workflows. It does not replace OpenCode's runtime, tools, or permission system.

## Mission

Make repository review in OpenCode visible and durable:

```text
prepare known input
  → OpenCode analyzes it under the user's permissions
  → oy validates a report
  → fix one finding and rerun
```

## Product boundary

| Owner | Responsibilities |
|---|---|
| oy CLI | Repository/diff collection, ordering, limits, evidence identity, report validation, Markdown/SARIF output |
| oy plugin | One coding agent, audit/review/enhance skills, and three slash commands |
| OpenCode and user | Models, providers, credentials, permissions, tools, sessions, UI, and project instructions |

## Principles

1. **OpenCode permissions stay authoritative.** oy does not add a parallel permission policy.
2. **Inputs can be repeatable; conclusions cannot.** Evidence and report normalization are deterministic, model reasoning is not.
3. **Fail instead of silently sampling.** Changed evidence, malformed reports, and explicit limits are visible errors.
4. **Reports are handoff artifacts.** Stable IDs and reruns matter more than chat-only output.
5. **Keep host coupling narrow.** OpenCode-specific setup, API, and version code must support the review workflow directly.
6. **Keep one useful agent.** The `oy` prompt emphasizes inspection, small changes, verification, and worktree safety without defining permissions.

## Current product

The matching `@oy-cli/opencode` plugin registers:

- the `oy` primary agent;
- `oy-audit`, `oy-review`, and `oy-enhance` skills;
- `/oy-audit`, `/oy-review`, and `/oy-enhance` commands.

The Rust CLI prepares evidence, verifies model-written candidates, normalizes finding metadata, writes Markdown/SARIF, manages plugin setup, and provides narrow OpenCode launch/session helpers.

## Non-goals

- becoming a second coding-agent or model runtime;
- owning provider credentials or model routing;
- bypassing or broadening OpenCode permissions;
- adding general shell, edit, web, clone, or search tools;
- claiming deterministic security or quality conclusions;
- running paid model evaluations in default CI.

See [Architecture](architecture.md) for implementation boundaries, [LLM evaluation](evaluation.md) for prompt testing, and [`ROADMAP.md`](https://github.com/adonm/oy-cli/blob/main/ROADMAP.md) for current work.
