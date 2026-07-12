# Project direction

Oy gives OpenCode a concise autonomous agent and deterministic repository evidence/report workflows. It does not replace OpenCode's model runtime, tools, or permission system.

## Mission

Make audit, review, and remediation in OpenCode autonomous, bounded, and reviewable:

```text
prepare deterministic evidence
  → OpenCode reasons and edits under the user's permissions
  → oy validates a durable report
  → rerun to confirm
```

## Product boundary

| Layer | Ownership |
|---|---|
| Core | Gitignore-aware evidence, ordered chunks, diff preparation, stable findings, Markdown/SARIF normalization, safe report writes |
| OpenCode integration | One concise `oy` agent and three canonical skills: audit, review, enhance |
| User/OpenCode | Models, providers, permissions, approvals, edits, shell, web, sessions, UI, project instructions |
| OpenCode adapters | Setup plus narrow launch/session/API wrappers |

## Principles

1. **OpenCode permissions are authoritative.** Oy does not maintain separate plan/edit/auto policies or override user rules.
2. **One agent is enough.** Keep `oy` as a short autonomous engineering prompt; use OpenCode's built-in Plan agent when planning is wanted.
3. **Skills carry workflow knowledge.** Audit, review, and remediation execute locally in the selected agent rather than delegating to permission adapters.
4. **Determinism stops at inference.** Oy can make evidence and report normalization repeatable; model findings remain model-dependent.
5. **Prefer file artifacts.** Large evidence should be prepared once into workspace-local immutable files with a small structured index.
6. **Fail closed.** Do not silently sample around changed input, skipped chunks, output escapes, malformed findings, or explicit limits.
7. **Minimize host coupling.** Every OpenCode-specific launcher, API, config, and version dependency must justify itself against a CLI-and-skills alternative.

## The oy agent

The actual [`oy` system prompt](https://github.com/adonm/oy-cli/blob/main/packages/opencode/assets/agents/oy.md) replaces the provider-specific base prompt, so `oy` must carry the important general coding behaviors itself while staying concise.

It aligns with OpenCode 2 Build on:

- inspecting before editing;
- following established dependencies and conventions;
- making the smallest correct change;
- completing implementation and verification rather than stopping at a proposal;
- preserving unrelated dirty-worktree changes;
- allowing focused verified checkpoint commits for long unattended work while protecting unrelated changes, history, pushes, and tags;
- parallelizing independent inspection;
- concise progress and completion reporting.

It intentionally differs by emphasizing local reasoning, narrow deterministic boundaries, evidence-first summaries, and unattended completion. It does not define permissions; the user's effective OpenCode policy applies.

Prompt changes require comparison against a tagged OpenCode 2 default and live evaluation. Do not copy provider-specific UI taste, temporary tool names, or large formatting manuals into the prompt.

## Current state

The current release exposes deterministic collection and report finalization through file-backed CLI commands.

Current setup pins `@oy-cli/opencode` to the matching binary version. The package installs:

- one [`oy` primary agent](https://github.com/adonm/oy-cli/blob/main/packages/opencode/assets/agents/oy.md) without permission overrides;
- [`oy-audit`](https://github.com/adonm/oy-cli/blob/main/packages/opencode/assets/skills/oy-audit/SKILL.md), [`oy-review`](https://github.com/adonm/oy-cli/blob/main/packages/opencode/assets/skills/oy-review/SKILL.md), and [`oy-enhance`](https://github.com/adonm/oy-cli/blob/main/packages/opencode/assets/skills/oy-enhance/SKILL.md) skills;
- three thin commands selecting `oy`.

OpenCode resolves the package into its isolated cache and registers its agent, skills, and commands through the OpenCode V2 plugin API.

## File-backed contract

The deterministic CLI exposes:

```text
oy audit prepare --json
oy audit finalize --run <id> --json
oy review prepare [target] --json
oy review finalize --run <id> --json
```

Preparation writes an index, manifest, prior report, and bounded evidence chunks under a workspace-local run directory. OpenCode reads the index, prior report when present, and every indexed chunk, then writes separate candidate Markdown and findings JSON with native tools. Finalization verifies the bound evidence and canonicalizes the report. Authoritative state uses the platform state location, falling back to the local-data directory when needed.

Default setup does not rewrite the global tool-output budget or install direct command/agent/skill files.

## Keep

- Repository manifests and explicit coverage/exclusions.
- Deterministic repository and diff chunking.
- Stable evidence identity and changed-input rejection.
- Existing-report carry-forward.
- Stable finding IDs and statuses.
- Markdown and SARIF rendering.
- Safe workspace output handling.
- Optional direct `tokei` and Universal Ctags orientation for large scopes.
- One-finding remediation and rerun confirmation.
- The concise autonomous `oy` agent.

## Remove or demote

- Oy-owned plan/edit/auto permission modes.
- Dedicated auditor/reviewer/enhancer permission agents.
- `oy model`, `oy open`, `oy chat`, and unknown-argument passthrough; bare `oy` remains the integration-aware TUI launcher and native host operations use `opencode2`.
- Coupled oy/OpenCode installation and upgrades.
- Exact host API/version coupling not needed by skills.
- Global output-budget mutation after evidence moves to files.

## Non-goals

- Becoming a second coding-agent runtime.
- Owning permission policy or presenting oy as a sandbox.
- Adding general shell, edit, web, clone, credential, or provider capabilities.
- Claiming deterministic security or quality conclusions.
- Persisting OpenCode credentials or transcripts.

Changes should improve autonomous completion, deterministic evidence, report usefulness, or integration simplicity without broadening oy into another host.
