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

The OpenCode plugin exposes:

| Command | Action |
|---|---|
| `/oy-audit` | Load the audit skill and review all prepared evidence. |
| `/oy-review` | Load the code-review skill and review all prepared evidence. |
| `/oy-enhance` | Fix one finding from a generated report. |

These are OpenCode slash commands, not shell subcommands. They use OpenCode tools and your effective permissions.

## Setup and maintenance

| Command | Purpose |
|---|---|
| `oy setup` | Register the version-matched oy plugin package in global OpenCode config and default Cursor commit/PR attribution off. |
| `oy setup --workspace` | Register the package under `OY_ROOT/.opencode/opencode.json(c)` and apply the same global Cursor attribution defaults. |
| `oy setup --dry-run` | Preview setup or removal. |
| `oy setup --remove` | Back up and remove the oy package entry and legacy oy-owned files/config entries. |
| `oy doctor` | Show selected paths, host version, setup state, and optional tools. |
| `oy doctor --check` | Validate the effective service, API, plugin, agent, skills, commands, and models. |
| `oy doctor --install-missing` | Install missing OpenCode/context helpers with mise. |
| `oy upgrade [--check|--dry-run]` | Upgrade a mise-installed oy, latest Node.js, and the OpenCode 2 npm package. |

See [Compatibility](compatibility.md) for the OpenCode versions accepted by this release.

## Setup ownership and backups

OpenCode global setup uses `OPENCODE_CONFIG_DIR` when set; otherwise it uses the platform OpenCode config directory (normally `~/.config/opencode/` on Linux). OpenCode workspace setup uses `OY_ROOT/.opencode/`. An existing `opencode.jsonc` is selected before `opencode.json`.

Setup registers `@oy-cli/opencode@<matching-oy-version>` in `opencode.json(c)`. OpenCode installs the package and its production dependencies, including the Cursor provider and official Cursor SDK. Setup also sets missing `attribution.attributeCommitsToAgent` and `attribution.attributePRsToAgent` preferences to `false` in Cursor's global `cli-config.json`. Explicit attribution values and unrelated Cursor settings are preserved; `--remove` leaves these user preferences in place.

Setup owns:

- the version-matched `@oy-cli/opencode` entry in `plugins`;
- superseded `plugins/oy.js` and `plugins/assets/` files from direct-file releases;
- old direct files named `oy`, `oy-*`, or `oy.*` under `agents`, `commands`, and `skills`;
- obsolete oy plugin, command, and MCP config entries from earlier releases.

Before changing existing owned entries or adding Cursor attribution defaults, setup creates a mode-`0700` backup under the platform state directory (or local-data fallback). It moves namespaced files, snapshots changed config bytes, and leaves unmodified configs byte-for-byte untouched. Unrelated settings remain in place. JSON/JSONC comments and formatting are preserved in the backup, while changed configs are pretty-reserialized.

## Curl installer

```bash
curl -fsSL https://oy.adonm.dev/install.sh | sh                    # prompt for mise scope
curl -fsSL https://oy.adonm.dev/install.sh | sh -s -- --global    # global mise config
curl -fsSL https://oy.adonm.dev/install.sh | sh -s -- --workspace # current mise.toml
```

The installer uses mise for oy, Node.js, OpenCode, and optional context helpers. The default is interactive: choose global config or the current workspace’s `mise.toml`. Noninteractive installs default to global.

## Environment variables

| Variable | Purpose |
|---|---|
| `OY_ROOT` | Select the workspace root and path boundary. |
| `OPENCODE_CONFIG_DIR` | Override the global OpenCode config directory. |
| `OY_OPENCODE` | Select the OpenCode executable; default `opencode2`. |
| `OY_OPENCODE_MODEL` | Select a workflow model as `provider/model#variant`. |
| `OY_COLOR` | Set `auto`, `always`, or `never`. |
| `NO_COLOR` | Disable color output. |
| `OY_INSTALL_SCOPE` | Select `global` or `workspace` in `install.sh`; an explicit installer flag wins. |
| `OY_SKIP_SETUP` | Skip integration setup and runtime load checks in `install.sh`. |
| `CURSOR_API_KEY` | Supply the Cursor provider key without using OpenCode `/connect`. |
| `OPENCODE_CURSOR_STALL_MS` | Override oy's 1,200,000 ms Cursor idle-stream watchdog default; set `0` to disable it. |

## Cursor models in OpenCode

The default OpenCode plugin registers a **Cursor** provider. Use `/connect` and paste a Cursor API key; the account's live catalog appears as `cursor/*`. Configuration under `providers.cursor.settings` is passed to `@stablekernel/opencode-cursor`, for example `sandbox`, `mode`, `settingSources`, or `systemPrompt`. Oy raises the provider's idle-stream watchdog default from 120,000 ms to 1,200,000 ms; an explicit `OPENCODE_CURSOR_STALL_MS` value wins.

`cursor/*` uses Cursor tools outside OpenCode permissions. Oy preserves the provider's sandbox default; see the [security policy](https://github.com/adonm/oy-cli/blob/main/SECURITY.md).

## Files written by oy

| Path | Purpose |
|---|---|
| `ISSUES.md` | Default Markdown audit report. |
| `REVIEW.md` | Default code-quality report. |
| `oy.sarif` | Default SARIF audit output. |
| `.oy/runs/<run-id>/` | Prepared evidence and model-written candidates. |
| Cursor global `cli-config.json` | Missing commit/PR attribution preferences, defaulted to `false` by setup. |
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
