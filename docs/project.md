# Project direction

`oy` gives coding agents repeatable audit → review → fix workflows through portable Agent Skills. Any agent that reads `.agents/skills` can run them; oy works through your agent's own tools and permissions.

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
| oy skills | Three workflow skills and one setup skill |
| the agent and user | Models, providers, credentials, tools, sessions, UI, and project instructions |

## Principles

1. **Run inside the user's agent.** The skills use the permissions the user's agent already has; oy contributes workflow structure — frozen input, full-coverage reading, verified reports — not new capabilities.
2. **Inputs can be repeatable; conclusions cannot.** Evidence and report normalization are deterministic, model reasoning is not.
3. **Fail instead of silently sampling.** Changed evidence, malformed reports, and explicit limits are visible errors.
4. **Reports are handoff artifacts.** Stable IDs and reruns matter more than chat-only output.
5. **Keep host coupling narrow.** The only agent-specific code is the optional OpenCode location refresh after setup.

## Current product

Setup writes:

- `oy-audit`, `oy-review`, and `oy-enhance` skills;
- the `oy-setup` skill, which verifies skill discovery in the agent's environment.

The Rust CLI prepares evidence, verifies model-written candidates, normalizes finding metadata, writes Markdown/SARIF, installs the skills, and migrates legacy OpenCode plugin state.

## Boundaries

oy stays focused on evidence collection, report validation, and skill installation. Model choice, permissions, provider credentials, editing tools, and final judgment stay with your agent — oy structures the review workflow without taking it over. Paid model evaluations stay out of default CI.

See [Architecture](architecture.md) for implementation boundaries and [`ROADMAP.md`](https://github.com/adonm/oy-cli/blob/main/ROADMAP.md) for current work.
