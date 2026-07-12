# CLI and OpenCode reference

`oy` has two user interfaces:

- shell commands such as `oy audit` and `oy review`;
- OpenCode slash commands `/oy-audit`, `/oy-review`, and `/oy-enhance` installed by the package.

The slash commands load the same packaged skills. They are not `oy` shell subcommands.

## Workflow commands

| Command | Behavior |
|---|---|
| `oy audit [FOCUS...]` | Audit collected repository evidence and write `ISSUES.md`; use `--format sarif` for `oy.sarif`. A single focus argument naming an existing workspace path becomes the collection scope. |
| `oy review [TARGET]` | Review the collected workspace, or deterministic `git diff TARGET` evidence when a branch, commit, tag, or ref is supplied; writes `REVIEW.md`. |
| `oy enhance [FOCUS...]` | Confirm and fix one actionable finding from `ISSUES.md` or `REVIEW.md`, then run focused verification. A finding ID is the clearest focus. |
| `oy enhance --interactive [FOCUS...]` | Run the same remediation through OpenCode `mini` so native permission prompts, questions, and forms are available. |
| `oy run [--continue-session | --resume SESSION_ID] [--auto] [PROMPT...]` | Run one task with the `oy` agent. The prompt can also come from stdin. With no prompt in a terminal, launch OpenCode. |
| `oy recover` | Resume the retained OpenCode session for an interrupted outer `oy audit`, `oy review`, or `oy enhance` run; it does not recover a standalone `prepare`. |
| `oy` | Validate setup and launch the OpenCode 2 TUI. Select `oy` or another agent in the TUI. |

Useful workflow options:

- `--out PATH` selects a workspace-relative report path;
- `--max-chunks N` changes the default fail-closed limit of 80;
- `oy review --focus TEXT` is repeatable;
- `oy audit --format markdown|sarif` selects the renderer;
- `OY_OPENCODE_MODEL=provider/model#variant` selects the noninteractive model;
- `--json` requests machine-readable output where supported.

`oy run --json` forwards OpenCode's JSON event stream. Managed audit/review and noninteractive enhance runs instead print one final oy summary object. The global `--quiet`, `--verbose`, and `--json` flags are mutually exclusive.

`oy run --auto` asks OpenCode to approve pending permission requests once; configured explicit denies still apply. Unknown `oy` commands and flags are errors. Use `opencode2` directly for native OpenCode commands and options.

## Setup and maintenance

| Command | Behavior |
|---|---|
| `oy setup` | Back up the previous oy integration and register the version-matched npm package globally. |
| `oy setup --workspace` | Register it in `OY_ROOT/.opencode/` instead. |
| `oy setup --dry-run` | Preview setup or removal without writing. |
| `oy setup --remove` | Back up and remove oy-owned package, command, and direct-file entries. |
| `oy doctor` | Show the selected OpenCode host, setup paths, mise, and optional context-helper availability. In a terminal it may offer to install missing tools. |
| `oy doctor --check` | Validate the effective service, API, location, `oy` agent, three commands, three skills, models, providers, and plugin. |
| `oy doctor --install-missing` | Use mise to install the pinned OpenCode beta, `tokei`, and Universal Ctags when missing. |
| `oy upgrade [--check|--dry-run]` | Check or upgrade oy and OpenCode when both are active mise tools, refresh global setup, and report the backup path. |

oy defaults to `opencode2`. Version 0.13.4 accepts the pinned beta `0.0.0-next-15353` and tagged OpenCode 2.x releases. Other prereleases, other major versions, and OpenCode 1 fail the host contract check. `OY_OPENCODE` can select another executable but cannot bypass that check.

## File-backed protocol

Normal `oy audit` and `oy review` runs orchestrate these lower-level commands:

```text
oy audit prepare [options]
oy audit finalize --run <run-id>
oy review prepare [target] [options]
oy review finalize --run <run-id>
```

Preparation writes an index, manifest, previous report when present, and bounded chunks under `.oy/runs/<run-id>/`. The skill reads every page of every indexed chunk with OpenCode's native `read` tool and writes separate candidate Markdown and findings JSON files. Finalization verifies the workspace binding, immutable artifact hashes, current evidence, previous output, and candidate shape before writing the normalized report.

These commands are public for custom automation, but most users should call `oy audit` or `oy review`. Run `oy audit prepare --help` or `oy review prepare --help` for all protocol options.

## OpenCode package

`oy setup` pins `@oy-cli/opencode` to the binary version in OpenCode's `plugins` array. The package registers:

- one primary agent named `oy`;
- skills `oy-audit`, `oy-review`, and `oy-enhance`;
- slash commands `/oy-audit`, `/oy-review`, and `/oy-enhance`.

The commands select the `oy` agent and tell it to load the corresponding skill. The package defines no permission rules. OpenCode's effective global and project permissions remain authoritative.

The versioned source is available directly:

- [`oy` agent](https://github.com/adonm/oy-cli/blob/main/packages/opencode/assets/agents/oy.md)
- [`oy-audit` skill](https://github.com/adonm/oy-cli/blob/main/packages/opencode/assets/skills/oy-audit/SKILL.md)
- [`oy-review` skill](https://github.com/adonm/oy-cli/blob/main/packages/opencode/assets/skills/oy-review/SKILL.md)
- [`oy-enhance` skill](https://github.com/adonm/oy-cli/blob/main/packages/opencode/assets/skills/oy-enhance/SKILL.md)

Use OpenCode's built-in Plan agent when read-only planning is wanted. The `oy` agent is autonomous and permission-neutral.

## Optional context helpers

The agent may run these directly through OpenCode's normal shell tool when they reduce context use:

```bash
tokei --compact --sort code -- <scope>
ctags --options=NONE --output-format=json --fields=+nK --extras=-F -f - ./<file>
```

`tokei` provides a compact aggregate language and code-size inventory without per-file records. Universal Ctags provides a symbol outline for an exact file without reading the whole file first. Scope both commands narrowly, treat their output as orientation, and confirm conclusions with source reads. `oy doctor --install-missing` installs both through mise. Search tools such as ripgrep are intentionally not added because OpenCode already provides native search.

## Setup ownership and backups

Global setup uses `OPENCODE_CONFIG_DIR` when set, otherwise the platform config directory plus `opencode` (normally `~/.config/opencode/` on Linux). A relative override resolves from `OY_ROOT`. Workspace setup uses `OY_ROOT/.opencode/`. In either directory, an existing `opencode.jsonc` is selected before `opencode.json`.

When existing config or oy-namespaced files will change, setup first creates a mode-`0700` backup under `oy/backups/` in the platform state location, falling back to the local-data directory when needed. It snapshots changed config files and moves direct entries named `oy`, `oy-*`, or `oy.*` from `agents`, `commands`, and `skills`. It then replaces current oy package and command entries and removes obsolete oy integration entries from older releases. Unrelated entries and generic `tool_output` settings are retained. A fresh setup with no existing config or legacy files creates no backup.

JSON and JSONC are pretty-reserialized, so comments and formatting are preserved only in the backup snapshot. Setup restores moved files if the config update fails. `--remove` uses the same backup-first behavior.

## Environment

| Variable | Purpose |
|---|---|
| `OY_ROOT` | Override the workspace root and path boundary. |
| `OPENCODE_CONFIG_DIR` | Override the global OpenCode config directory used by setup and validation. |
| `OY_OPENCODE` | Select the OpenCode executable; defaults to `opencode2`. |
| `OY_OPENCODE_MODEL` | Select a noninteractive workflow model as `provider/model#variant`; the variant is optional. |
| `OY_COLOR` | Select `auto`, `always`, or `never` color behavior. |
| `NO_COLOR` | Disable color output. |
| `OY_SKIP_SETUP` | Skip `oy setup` in `install.sh`. |
| `OY_MISE_MINIMUM_RELEASE_AGE` | Override the installer's mise release-age filter. |

## Path and disclosure boundaries

Input scopes must resolve inside the workspace. Output paths must be workspace-relative and may not escape through parent traversal or symlinks. The collector skips the categories listed in [Workflow coverage](workflows.md#coverage-and-failure-limits), including files over 512 KiB. Eligible source and diff evidence is sliced so every chunk stays within 240 KiB, 19,000 lines, and the fixed 64,000-token estimate.

Prepared source text may be sent to the model provider selected in OpenCode. `oy` does not upload reports or store provider credentials. See [SECURITY.md](https://github.com/adonm/oy-cli/blob/main/SECURITY.md) for the complete trust boundaries.
