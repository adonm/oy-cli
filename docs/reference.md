# CLI reference

Run `oy <command> --help` for the exact flags supported by your installed version.

## Workflow commands

| Command | Purpose |
|---|---|
| `oy audit [FOCUS...]` | Audit a workspace/path and write `ISSUES.md`; add `--format sarif` for SARIF. |
| `oy review [TARGET]` | Review the workspace or `git diff TARGET` and write `REVIEW.md`. |
| `oy enhance [FOCUS...]` | Confirm and fix one finding from `ISSUES.md` or `REVIEW.md`. A finding ID is the clearest focus. |
| `oy enhance --interactive [FOCUS...]` | Run enhancement through OpenCode `mini` for native prompts and forms. |
| `oy run [OPTIONS] [PROMPT...]` | Run a general task with the `oy` agent; prompt may come from stdin. |
| `oy recover` | Resume an interrupted managed audit, review, or enhance session. |
| `oy` | Validate setup and launch the OpenCode TUI. |

Common options:

| Option | Meaning |
|---|---|
| `--out PATH` | Write the report to a workspace-relative path. |
| `--max-chunks N` | Change the fail-closed evidence limit (default `80`). |
| `oy review --focus TEXT` | Add repeatable review guidance. |
| `oy audit --format markdown|sarif` | Select report format. |
| `--json` | Request machine-readable output where supported. |
| `oy run --auto` | Ask OpenCode to approve pending requests once; explicit denies still apply. |

Unknown oy commands are errors. Use `opencode2` directly for native OpenCode commands.

## OpenCode slash commands

The plugin registers:

| Command | Action |
|---|---|
| `/oy-audit` | Load the audit skill and review all prepared evidence. |
| `/oy-review` | Load the code-review skill and review all prepared evidence. |
| `/oy-enhance` | Fix one finding from a generated report. |

These are OpenCode prompt commands, not shell subcommands. They use the same `oy` agent and your effective OpenCode permissions.

## Setup and maintenance

| Command | Purpose |
|---|---|
| `oy setup` | Back up prior oy entries and register the matching npm plugin globally. |
| `oy setup --workspace` | Register the plugin in `OY_ROOT/.opencode/`. |
| `oy setup --dry-run` | Preview setup or removal. |
| `oy setup --remove` | Back up and remove oy-owned entries. |
| `oy doctor` | Show selected paths, host version, setup state, and optional tools. |
| `oy doctor --check` | Validate the effective service, API, plugin, agent, skills, commands, and models. |
| `oy doctor --install-missing` | Install missing OpenCode/context helpers with mise. |
| `oy upgrade [--check|--dry-run]` | Upgrade a mise-installed oy, latest Node.js, and the OpenCode 2 npm package. |

See [Compatibility](compatibility.md) for the OpenCode versions accepted by this release.

## Setup ownership and backups

Global setup uses `OPENCODE_CONFIG_DIR` when set; otherwise it uses the platform OpenCode config directory (normally `~/.config/opencode/` on Linux). Workspace setup uses `OY_ROOT/.opencode/`. An existing `opencode.jsonc` is selected before `opencode.json`.

Setup owns:

- the matching `@oy-cli/opencode` plugin entry;
- old direct files named `oy`, `oy-*`, or `oy.*` under `agents`, `commands`, and `skills`;
- obsolete oy command/MCP config entries from earlier releases.

Before changing existing owned entries, setup creates a mode-`0700` backup under the platform state directory (or local-data fallback). It snapshots changed config bytes and moves namespaced files. Unrelated settings remain in place. JSON/JSONC comments and formatting are preserved in the backup, while the active config is pretty-reserialized.

## Environment variables

| Variable | Purpose |
|---|---|
| `OY_ROOT` | Select the workspace root and path boundary. |
| `OPENCODE_CONFIG_DIR` | Override the global OpenCode config directory. |
| `OY_OPENCODE` | Select the OpenCode executable; default `opencode2`. |
| `OY_OPENCODE_MODEL` | Select a workflow model as `provider/model#variant`. |
| `OY_COLOR` | Set `auto`, `always`, or `never`. |
| `NO_COLOR` | Disable color output. |
| `OY_SKIP_SETUP` | Skip setup in `install.sh`. |

## Files written by oy

| Path | Purpose |
|---|---|
| `ISSUES.md` | Default Markdown audit report. |
| `REVIEW.md` | Default code-quality report. |
| `oy.sarif` | Default SARIF audit output. |
| `.oy/runs/<run-id>/` | Prepared evidence and model-written candidates. |
| platform state/data directory | Private backup and prepared-run metadata. |

Report output paths must be workspace-relative and may not escape through parent traversal or symlinks.

## Advanced prepare/finalize protocol

Normal workflows orchestrate these commands automatically:

```text
oy audit prepare [options]
oy audit finalize --run <run-id>
oy review prepare [target] [options]
oy review finalize --run <run-id>
```

Preparation writes an index, manifest, previous report when present, and ordered chunks under `.oy/runs/<run-id>/`. Finalization verifies the workspace, evidence hashes, current input, previous output, and candidate report/findings before writing the normalized report.

These commands are public for custom automation. Run their `--help` output before integrating them.

## Path and disclosure boundaries

Input scopes must resolve inside the workspace. The collector's exclusions and limits are documented in [Coverage and limits](workflows.md#coverage-and-limits).

Prepared source may be sent to your configured model provider. oy does not upload reports or store provider credentials. See [SECURITY.md](https://github.com/adonm/oy-cli/blob/main/SECURITY.md).
