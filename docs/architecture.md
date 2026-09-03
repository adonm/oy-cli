# Architecture

`oy` is a small evidence and workflow integration for agent skills. The
user's agent (OpenCode, Cursor, Codex, Copilot, or Gemini CLI) owns model
execution, providers, permissions, and tools. `oy` owns deterministic
evidence preparation, report finalization, skill installation, and setup
glue.

## Workflow flow

```text
the agent loads an oy skill (oy-audit, oy-review, or oy-enhance)
  → Rust writes bounded evidence under .oy/runs/<run-id>/
  → the agent reads the index, previous report when present, and every indexed chunk
  → the agent writes candidate report + findings JSON
  → Rust verifies the exact prepared run
  → Rust writes ISSUES.md, REVIEW.md, or SARIF
```

The skill bodies carry the full protocol; the CLI commands prepare and
finalize deterministically.

## Main modules

| Path | Responsibility |
|---|---|
| `src/cli/app.rs` | CLI parsing and dispatch |
| `src/skills.rs` | Canonical skill assets (embedded) and asset contract tests |
| `src/skills/setup.rs` | Skill installation/removal, legacy migration, locking, plugin-cache cleanup |
| `src/skills/setup/backup.rs` | Persistent setup backups and move/restore mechanics |
| `src/skills/setup/legacy_config.rs` | OpenCode JSON/JSONC parsing and stripping of legacy oy entries |
| `src/skills/opencode_host.rs` | OpenCode executable selection used for the post-setup location refresh |
| `src/skills/opencode_api.rs` | Bounded call to `opencode2 api` for location eviction |
| `src/workflow.rs` | Run-ID generation for prepared artifacts |
| `src/artifacts.rs` | File-backed preparation/finalization and private run-state verification |
| `src/audit/input.rs` | Repository collection, manifests, chunking, and git diff evidence |
| `src/audit/findings.rs` | Finding extraction, normalization, IDs, and statuses |
| `src/audit/sarif.rs` | SARIF rendering |
| `src/tools/external.rs` | Bounded subprocess execution used by upgrade and doctor |
| `src/cli/config/paths.rs` | Workspace and safe output-path handling |
| `src/cli/config/atomic_write.rs` | Staged file batches with rollback |

The canonical skill assets are plain Markdown embedded in the binary and
written by setup:

| Path | Responsibility |
|---|---|
| `assets/skills/oy-audit/SKILL.md` | Deterministic security-audit protocol |
| `assets/skills/oy-review/SKILL.md` | Deterministic code-quality review protocol |
| `assets/skills/oy-enhance/SKILL.md` | One-finding remediation protocol |
| `assets/skills/oy-setup/SKILL.md` | Agent-driven setup: verify, install, doctor |

## Setup

Global setup writes the skills under `~/.agents/skills/` (or `OY_SKILLS_DIR`),
the cross-agent location read natively by OpenCode, Cursor, Codex, Copilot,
and Gemini CLI. Workspace setup writes them under `OY_ROOT/.agents/skills/`.
Skill files carry a generated marker and are refreshed in place; user files
without the marker are preserved.

Setup also migrates older oy releases: it strips the version-matched
`@oy-cli/opencode` plugin entry and other legacy oy entries from
`opencode.json(c)`, moves direct oy-namespaced files out of the OpenCode
config directory, and deletes the downloaded OpenCode plugin package cache
(a regenerable cache).

When existing config or oy-owned files will change, setup first creates a
persistent mode-`0700` backup in the platform state location, falling back
to the local-data directory when no dedicated state directory exists.
Unmodified files remain byte-for-byte untouched. Config writes are a staged
rollback-capable batch; if the batch fails, moved files are restored.

After a change, setup asks a running OpenCode 2 service to evict its cached
location so the next session discovers the new files; other agents pick the
files up on their own discovery cycle.

## Artifact verification

Preparation writes model-readable artifacts inside `.oy/runs/<run-id>/` and
authoritative state in the platform state location, falling back to the
local-data directory when needed. The index contains relative artifact
paths, coverage metadata, counts, and evidence digest; it does not contain
the private authoritative state.

Finalization rejects a mismatched workspace, changed repository evidence,
modified immutable artifacts, concurrent output changes, or malformed
candidate findings. It can verify artifact integrity and report shape, but
it cannot prove that the model read the index, previous report, and every
indexed chunk; complete ordered reading is enforced by the skill protocol.

## Trust boundaries

| Boundary | Owner | Posture |
|---|---|---|
| Models, provider traffic, credentials | the user's agent and configured providers | oy never stores provider credentials |
| Permissions, edits, shell, web, questions | the agent/user | skills run under the agent's own permission model |
| Repository and diff collection | oy CLI | read inside the workspace, apply documented exclusions, and fail closed on limits |
| Workflow artifacts and reports | oy CLI + the agent | paths remain inside the workspace; evidence is hash-checked and model-written candidates are validated before final output |
| Setup/removal | oy CLI | namespace-bounded backup-first changes with rollback on config failure |

## Design rules

- Do not add permission overrides; the skills run under whatever permissions the user's agent has.
- Keep oy to three workflow skills plus the setup skill.
- Put evidence identity, ordering, limits, and report validation in Rust.
- Prefer file artifacts and native agent reads over large tool responses.
- Validate workspace paths at read/write boundaries.
- Do not reintroduce a model client, provider router, chat UI, or general tool registry.
