# Project direction

`oy` gives OpenCode a focused coding agent and repeatable audit → review → fix workflows. Cursor models are an OpenCode provider, not a second oy host integration.

## Mission

Make repository review in coding agents visible and durable:

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
| oy integration | One coding-agent behavior, audit/review/enhance skills, and three slash commands |
| OpenCode and user | Models, providers, credentials, tools, sessions, UI, and project instructions; Cursor's provider path owns its separate tool policy |

## Principles

1. **Do not add a parallel permission policy.** OpenCode permissions remain authoritative for normal models; selecting `cursor/*` explicitly enters Cursor's separate tool boundary.
2. **Inputs can be repeatable; conclusions cannot.** Evidence and report normalization are deterministic, model reasoning is not.
3. **Fail instead of silently sampling.** Changed evidence, malformed reports, and explicit limits are visible errors.
4. **Reports are handoff artifacts.** Stable IDs and reruns matter more than chat-only output.
5. **Keep OpenCode coupling narrow.** Setup, API, and version code must support the review workflow directly.
6. **Keep one useful agent.** The `oy` prompt emphasizes inspection, small changes, verification, and worktree safety without defining permissions.

## Current product

The matching `@oy-cli/opencode` plugin registers:

- the `oy` primary agent;
- `oy-audit`, `oy-review`, and `oy-enhance` skills;
- `/oy-audit`, `/oy-review`, and `/oy-enhance` commands;
- a Cursor provider whose models are backed by the official Cursor SDK.

The Rust CLI prepares evidence, verifies model-written candidates, normalizes finding metadata, writes Markdown/SARIF, and provides narrow OpenCode launch/session helpers.

## Non-goals

- becoming a second coding-agent or model runtime;
- owning provider credentials or model routing;
- adding permission overrides; note that the explicit `cursor/*` provider path executes Cursor tools outside OpenCode permissions;
- adding general shell, edit, web, clone, or search tools;
- claiming deterministic security or quality conclusions;
- running paid model evaluations in default CI.

See [Architecture](architecture.md) for implementation boundaries, [LLM evaluation](evaluation.md) for prompt testing, and [`ROADMAP.md`](https://github.com/adonm/oy-cli/blob/main/ROADMAP.md) for current work.
