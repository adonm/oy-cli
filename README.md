# oy

[![Crates.io](https://img.shields.io/crates/v/oy-cli.svg)](https://crates.io/crates/oy-cli)
[![docs.rs](https://docs.rs/oy-cli/badge.svg)](https://docs.rs/oy-cli)

**A concise autonomous OpenCode agent with repeatable repository audits and reviews.**

`oy` is for maintainers who already use [opencode](https://opencode.ai/) and want a concise autonomous coding agent plus bounded, reviewable security audit, code-quality review, and finding-remediation workflows.

opencode still owns the model, provider, UI, sessions, permissions, edits, shell, and web tools. `oy` adds:

- deterministic, gitignore-aware repository and diff collection,
- one autonomous `oy` agent aligned with OpenCode 2's inspect/implement/verify behavior,
- three canonical audit/review/enhance skills that run under the user's OpenCode permissions,
- file-backed deterministic evidence with private SHA-256-bound workflow state,
- an `@oy-cli/opencode` OpenCode V2 package for the agent, skills, and commands,
- Markdown and SARIF rendering with stable finding IDs and statuses,
- a one-finding-at-a-time remediation handoff.

The **inputs, ordering, limits, and report rendering** are deterministic. Model conclusions are not; model choice, prompt quality, and the user's OpenCode tool policy still affect outcomes. Oy does not maintain a second permission system.

## Quick start

Requirements: Linux or macOS, OpenCode 2 with a configured provider, plus `git` for diff reviews. Windows users should run oy inside WSL2. oy 0.13.4 does not support OpenCode 1.

```bash
curl -fsSL https://oy.adonm.dev/install.sh | sh
# Restart or activate your shell as the installer prints, then:
oy doctor
oy audit
```

The installer uses [`mise`](https://mise.jdx.dev/) to install pinned oy 0.13.4, `@opencode-ai/cli@0.0.0-next-15353`, `tokei`, and Universal Ctags. It verifies the primary versions, stops stale OpenCode services, prunes unreferenced old mise versions, and runs `oy setup`, which backs up the previous integration before registering `@oy-cli/opencode@0.13.4`. OpenCode installs the package into its isolated cache and the installer waits up to 120 seconds to verify that plugin ID `oy` loaded. Set `OY_SKIP_SETUP=1` to skip setup.

For a minimal manual install:

```bash
mise use --global node@24 cargo-binstall cargo:oy-cli@0.13.4 npm:@opencode-ai/cli@0.0.0-next-15353
oy setup
oy doctor
```

Configure authentication and models with OpenCode's [provider guide](https://v2.opencode.ai/providers). See the [getting-started guide](https://oy.adonm.dev/getting-started.html) for install behavior, supported host versions, and workspace-local setup.

## Core workflow

### 1. Audit a repository

```bash
oy audit
oy audit "authentication and authorization"
oy audit src/auth
oy audit --format sarif --out oy.sarif
```

The default report is `ISSUES.md`. A single argument that exactly names a workspace-relative path narrows collection; other text is an audit lens. SARIF output can be consumed by code-scanning tools.

### 2. Review a workspace or target diff

```bash
oy review
oy review main
oy review main --focus "types and trust boundaries"
```

With no target, `oy` reviews the collected workspace. With a branch, commit, or ref, it reviews deterministic `git diff <target>` input. The default report is `REVIEW.md`.

### 3. Fix one finding

```bash
oy enhance <finding-id>
```

Reports include stable IDs and statuses. `oy enhance` selects one actionable finding, makes a focused change under the user's effective OpenCode permissions, and verifies it when possible. Rerunning an audit or review reads the previous generated report once and carries forward only findings that remain current.

See the [workflow guide](https://oy.adonm.dev/workflows.html) for report semantics, scope behavior, failure limits, and practical examples.

## Why not just prompt opencode?

A free-form agent can choose what to inspect and may silently sample a large repository. The oy audit/review protocol instead:

1. inventories the requested scope,
2. creates ordered chunks or target-diff chunks,
3. fails closed when the configured chunk budget is exceeded,
4. exposes every bounded chunk in one versioned index for native OpenCode reads,
5. rejects changed input, artifact tampering, concurrent output changes, and malformed findings,
6. normalizes the final report for reruns and remediation.

This makes coverage decisions visible and repeatable without rebuilding opencode's general coding agent.

## Coverage boundary

“Every chunk” means every chunk produced by oy's collector, not every byte in the repository. Collection skips gitignored and hidden paths, common dependency/build directories, lockfiles, likely-secret files, generated reports, binary/non-UTF-8/empty files, unreadable files, and files larger than 512 KiB. Eligible source and diff evidence is sliced at or below 240 KiB, 19,000 lines, and the fixed 64,000-token estimate. Review these exclusions before treating an audit as complete for a high-assurance use case.

For large unfamiliar scopes, the `oy` agent can use `tokei` for a compact language/size inventory and Universal Ctags for JSON symbol outlines of specific files. These are optional orientation tools, not evidence substitutes; conclusions still require source reads. Run `oy doctor --install-missing` to install either one when absent.

## Agent, package, and setup

The OpenCode V2 package lives at `packages/opencode` and publishes as `@oy-cli/opencode`. It registers one `oy` primary agent, the three canonical skills, and `/oy-audit`, `/oy-review`, and `/oy-enhance` without permission overrides. Read the actual [agent prompt](https://github.com/adonm/oy-cli/blob/main/packages/opencode/assets/agents/oy.md), [audit skill](https://github.com/adonm/oy-cli/blob/main/packages/opencode/assets/skills/oy-audit/SKILL.md), [review skill](https://github.com/adonm/oy-cli/blob/main/packages/opencode/assets/skills/oy-review/SKILL.md), and [enhance skill](https://github.com/adonm/oy-cli/blob/main/packages/opencode/assets/skills/oy-enhance/SKILL.md). `oy setup` pins the package version matching the binary in OpenCode's `plugins` array.

`oy setup` backs up the previous oy integration, removes superseded oy entries, and pins the matching package without changing unrelated config. Global setup honors `OPENCODE_CONFIG_DIR`; workspace setup writes `.opencode/`. See the [setup reference](https://oy.adonm.dev/reference.html#setup-ownership-and-backups) for exact ownership and rollback behavior.

The `oy` agent has no permission overrides. Its short [system prompt](https://github.com/adonm/oy-cli/blob/main/packages/opencode/assets/agents/oy.md) carries the useful OpenCode 2 defaults that a custom prompt would otherwise replace: inspect before editing, follow repository conventions, make the smallest correct change, persist through verification, preserve unrelated worktree changes, avoid destructive Git operations, and report concisely. OpenCode and the user remain authoritative for permissions and approvals.

When an integration-dependent command is run interactively without setup, oy asks whether to set up the global integration (or refresh a detected workspace integration). Noninteractive and JSON invocations still fail with an explicit `oy setup` instruction. `oy upgrade` quietly upgrades the mise-managed tools, applies setup, and reports only completion plus the backup path. OpenCode `run`, TUI, `mini`, and managed-API processes use `OY_ROOT` as their working directory.

oy defaults to the `opencode2` executable. This release supports exactly beta `0.0.0-next-15353` and tagged OpenCode 2.x; other prereleases and major versions fail closed until tested. `OY_OPENCODE` remains an executable override. `oy run`, `audit`, `review`, and `enhance` use the single `oy` agent. `oy run --auto` asks OpenCode to approve pending requests once while preserving explicit denies; without it, the user's normal policy applies. Set `OY_OPENCODE_MODEL=provider/model#variant` to override the noninteractive workflow model.

Bare `oy` validates the integration and launches the OpenCode 2 TUI. Select `oy` in the TUI when its concise autonomous prompt is useful; use OpenCode's built-in Plan agent when planning is wanted. Invoke `opencode2` directly for native host commands and options.

Audit and review use `prepare → native OpenCode reads/edits → finalize`. Preparation writes a small index, manifest, prior report, and bounded chunks under `.oy/runs/<run-id>/`. Private metadata in the platform state location, or local-data fallback, binds their hashes, canonical workspace, scope or target OID, output, and format. Finalization verifies those bindings and separate candidate Markdown/findings JSON before writing the report.

## Commands

| Command | Purpose |
|---|---|
| `oy audit [focus]` | Write `ISSUES.md` or SARIF from deterministic-input audit coverage |
| `oy audit prepare` / `oy audit finalize --run ID` | Internal file-backed audit protocol; also available for custom automation |
| `oy review [target]` | Write `REVIEW.md` for a workspace or target diff |
| `oy review prepare [target]` / `oy review finalize --run ID` | Internal file-backed review protocol; also available for custom automation |
| `oy enhance [--interactive] [focus]` | Fix one finding; `--interactive` delegates to OpenCode `mini` |
| `oy setup [--workspace] [--dry-run] [--remove]` | Install, preview, or back up/remove the OpenCode integration |
| `oy doctor [--check]` | Show local status; `--check` validates effective OpenCode runtime integration |
| `oy` | Validate the integration and launch the OpenCode 2 TUI |
| `oy run [--auto]` | Run a noninteractive task with the `oy` agent |
| `oy upgrade` | Quietly upgrade mise-managed `oy` and OpenCode, then report the integration backup |
| `oy recover` | Resume the retained OpenCode session for an interrupted workflow |

Inside OpenCode, use `/oy-audit`, `/oy-review`, and `/oy-enhance`. These slash commands load the same packaged skills; they are not `oy` shell subcommands. Run `oy <command> --help` for flags, and invoke `opencode2` directly for native OpenCode commands and options. The full inventory is in the [reference](https://oy.adonm.dev/reference.html).

## Safety

`oy` is not a sandbox. Repository text read from prepared artifacts may be sent to the model provider selected in OpenCode. Native oy can read collected workspace text, write workflow artifacts and requested reports inside the workspace, update integration config, launch OpenCode, and use its managed API through the CLI. oy does not store provider credentials; general edits, shell, web, authentication, and provider traffic remain governed by OpenCode.

Configure OpenCode [permissions](https://v2.opencode.ai/permissions) for your trust boundary and use a disposable environment for untrusted repositories. Read [SECURITY.md](SECURITY.md) before high-risk use.

## Project direction and development

The product contract is intentionally narrow: improve the audit → review → remediate loop without becoming another model client or general agent framework. See [ROADMAP.md](ROADMAP.md) for current outcomes and explicit non-goals.

```bash
just dev
just check
just eval
just docs
```

Contributor references: [architecture](docs/architecture.md), [evaluation](docs/evaluation.md), [contributing](CONTRIBUTING.md), and [docs.rs API](https://docs.rs/oy-cli).
