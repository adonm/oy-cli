# Architecture

`oy` is a small integration layer around OpenCode 2. OpenCode owns model execution, providers, permissions, sessions, the TUI, and general tools. `oy` owns deterministic evidence preparation, report finalization, and package/setup glue.

## Normal runtime flow

```text
oy audit / oy review
  → validate the OpenCode host and installed package
  → create a bound workflow session
  → run the oy agent with the matching packaged skill
  → skill calls oy audit|review prepare
  → Rust writes bounded evidence under .oy/runs/<run-id>/
  → OpenCode reads the index, previous report when present, and every indexed chunk
  → OpenCode writes candidate report + findings JSON
  → skill calls oy audit|review finalize
  → Rust verifies bindings and writes ISSUES.md, REVIEW.md, or SARIF
```

`oy enhance` runs the packaged enhancement skill against one report finding. Bare `oy` launches the TUI, and `oy run` runs a general task with the `oy` agent.

## Main modules

| Path | Responsibility |
|---|---|
| `src/cli/app.rs` | CLI parsing and dispatch |
| `src/opencode.rs` | Thin facade for OpenCode integration modules and package-asset contract tests |
| `src/opencode/host.rs` | Executable selection, version probing, and OpenCode 2 contract gate |
| `src/opencode/setup.rs` | Package setup/removal orchestration, namespace migration, locking, and prompting |
| `src/opencode/setup/backup.rs` | Persistent setup backups and move/restore mechanics |
| `src/opencode/setup/config_file.rs` | OpenCode JSON/JSONC parsing and oy-owned config transformations |
| `src/opencode/runner.rs` | TUI/task launch, audit/review/enhance orchestration, and recovery |
| `src/opencode/api.rs` | Bounded calls to the authenticated OpenCode API through `opencode2 api` |
| `src/workflow.rs` | Typed workflow context and retained recovery lease |
| `src/artifacts.rs` | File-backed preparation/finalization and private run-state verification |
| `src/audit/input.rs` | Repository collection, manifests, chunking, and git diff evidence |
| `src/audit/findings.rs` | Finding extraction, normalization, IDs, and statuses |
| `src/audit/sarif.rs` | SARIF rendering |
| `src/tools/external.rs` | Bounded subprocess execution used by upgrade |
| `src/cli/config/paths.rs` | Workspace and safe output-path handling |
| `src/cli/config/atomic_write.rs` | Staged file batches with rollback |

The published package is separate from the Rust binary:

| Path | Responsibility |
|---|---|
| `packages/opencode/src/index.js` | Registers the agent, skills directory, and three slash commands through the V2 plugin API |
| `packages/opencode/assets/agents/oy.md` | Primary agent definition and system prompt |
| `packages/opencode/assets/skills/*/SKILL.md` | Canonical audit, review, and enhancement protocols |

## Setup

Global setup updates an existing `opencode.jsonc` or `opencode.json` under `OPENCODE_CONFIG_DIR` or the platform config directory's `opencode` child. Workspace setup does the same under `OY_ROOT/.opencode/`. It adds the version-matched `@oy-cli/opencode` package to `plugins`.

When existing config or oy-namespaced files will change, setup first creates a persistent mode-`0700` backup in the platform state location, falling back to the local-data directory when no dedicated state directory exists. It snapshots changed configs and moves direct `oy`, `oy-*`, and `oy.*` agent/command/skill entries out of OpenCode's discovery paths. It also removes superseded oy config entries. Unrelated config remains in place.

Config writes are a staged rollback-capable batch. If the batch fails, moved files are restored. On success, the backup remains the recovery copy, including original JSONC comments and formatting.

## Workflow binding

The launcher binds:

- workflow kind and random run ID;
- canonical workspace and scope, or resolved review target OID;
- focus, output path, report format, and maximum chunks;
- selected model and OpenCode session;
- digest of any previous output.

Preparation writes model-readable artifacts inside `.oy/runs/<run-id>/` and authoritative state in the platform state location, falling back to the local-data directory when needed. The index contains relative artifact paths, coverage metadata, counts, and evidence digest; it does not contain the private authoritative state.

Finalization rejects a mismatched workspace, changed repository evidence, modified immutable artifacts, concurrent output changes, or malformed candidate findings. It can verify artifact integrity and report shape, but it cannot prove that the model read the index, previous report, and every indexed chunk; complete ordered reading is enforced by the skill protocol.

Interrupted orchestrated workflows retain their run/session context for `oy recover`. Completed workflows remove that recovery lease.

## OpenCode boundary

The selected host defaults to `opencode2` and always runs with `OY_ROOT` as its working directory. The runner uses OpenCode's noninteractive session API for bound workflows, `run` for general tasks, `mini` for interactive enhancement, and the TUI for bare launch. OpenCode stores credentials and sessions; oy passes only transient IDs and workflow metadata.

The package registers one permission-neutral primary agent, three skills, and three slash commands. It does not add tools or permission rules.

## Trust boundaries

| Boundary | Owner | Posture |
|---|---|---|
| Models, provider traffic, credentials | OpenCode | oy uses the selected host and never stores provider credentials |
| Permissions, edits, shell, web, questions | OpenCode/user | the package defines no permission overrides |
| Repository and diff collection | oy CLI | read inside the workspace, apply documented exclusions, and fail closed on limits |
| Workflow artifacts and reports | oy CLI + OpenCode | paths remain inside the workspace; evidence is hash-checked and model-written candidates are validated before final output |
| Setup/removal | oy CLI | namespace-bounded backup-first changes with rollback on config failure |

## Design rules

- Keep OpenCode permissions authoritative.
- Keep the package to one agent, three skills, and three commands.
- Put evidence identity, ordering, limits, and report validation in Rust.
- Prefer file artifacts and native OpenCode reads over large tool responses.
- Validate workspace paths at read/write boundaries.
- Do not reintroduce a model client, provider router, chat UI, or general tool registry.
