# Project direction

`oy` gives coding agents repeatable audit → review → fix workflows through portable Agent Skills. Any agent that reads `.agents/skills` can run them; oy itself is not a second agent or model runtime.

## Mission

Make repository review in coding agents visible and durable:

```text
prepare known input
  → the agent analyzes it under the user's permissions
  → oy validates a report
  → fix one finding and rerun
```

## Product boundary

| Owner | Responsibilities |
|---|---|
| oy CLI | Repository/diff collection, ordering, limits, evidence identity, report validation, Markdown/SARIF output, skill installation |
| oy skills | Three workflow skills, one setup skill, and the oy persona |
| the agent and user | Models, providers, credentials, tools, sessions, UI, and project instructions |

## Principles

1. **Do not add a parallel permission policy.** The skills run under whatever permissions the user's agent has; they never broaden them.
2. **Inputs can be repeatable; conclusions cannot.** Evidence and report normalization are deterministic, model reasoning is not.
3. **Fail instead of silently sampling.** Changed evidence, malformed reports, and explicit limits are visible errors.
4. **Reports are handoff artifacts.** Stable IDs and reruns matter more than chat-only output.
5. **Keep host coupling narrow.** The only agent-specific code is the optional OpenCode location refresh after setup.
6. **Keep one useful persona.** The oy persona emphasizes inspection, small changes, verification, and worktree safety without defining permissions.

## Current product

Setup writes:

- `oy-audit`, `oy-review`, and `oy-enhance` skills;
- the `oy-setup` skill and the `oy-persona.md` it installs into the agent's environment (improving the default agent or creating an `oy` agent).

The Rust CLI prepares evidence, verifies model-written candidates, normalizes finding metadata, writes Markdown/SARIF, installs the skills, and migrates legacy OpenCode plugin state.

## Non-goals

- becoming a second coding-agent or model runtime;
- owning provider credentials or model routing;
- adding permission overrides;
- adding general shell, edit, web, clone, or search tools;
- claiming deterministic security or quality conclusions;
- running paid model evaluations in default CI.

See [Architecture](architecture.md) for implementation boundaries and [`ROADMAP.md`](https://github.com/adonm/oy-cli/blob/main/ROADMAP.md) for current work.
