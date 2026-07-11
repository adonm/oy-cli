# CLI and MCP reference

## Commands

| Command | Behavior |
|---|---|
| `oy audit [FOCUS...]` | Run a restricted security audit; Markdown or SARIF; default `ISSUES.md`. |
| `oy review [TARGET]` | Review the collected workspace or `git diff TARGET`; default `REVIEW.md`. |
| `oy enhance [FOCUS...]` | Fix one finding; use a finding ID as positional focus. |
| `oy recover` | Resume the retained OpenCode session/context for an interrupted bound workflow. |
| `oy setup` | Write global opencode integration. |
| `oy setup --workspace` | Write integration under the current workspace's `.opencode/`. |
| `oy setup --dry-run` | Preview generated integration actions without writing. |
| `oy setup --remove` | Remove generated files and current oy-owned global config values; combine with `--workspace` for local setup. |
| `oy doctor` | Show OpenCode contract, config-path, helper, and selected-mode status. |
| `oy doctor --check` | Validate effective service version, required agents/commands, MCP connection, and model/provider/plugin availability. |
| `oy` / `oy open ...` / `oy chat` | Validate integration and launch/pass arguments to the OpenCode 2 TUI. |
| `oy run` | Run one task through OpenCode 2; supports `--continue-session`, `--resume`, and mode-selected agents. |
| `oy model [FILTER]` | List/filter models through the managed OpenCode API. |
| `oy modes` | Print safety-mode aliases and API agent behavior. |
| `oy enhance --interactive [FOCUS...]` | Run remediation through OpenCode `mini` for native permissions, questions, and forms. |
| `oy upgrade` | Upgrade mise-managed `cargo:oy-cli` and OpenCode, then explicitly rerun setup. |
| `oy mcp` | Serve MCP over stdio; normally launched by OpenCode. |

Run `oy <command> --help` for all options and defaults. Unknown top-level actions and flags pass through to the selected OpenCode executable unless they begin with a known oy command.

oy defaults to `opencode2` and supports exactly beta `0.0.0-next-15323` plus tagged OpenCode 2.x. Other prereleases and major versions fail closed until tested. `OY_OPENCODE` remains an executable override. OpenCode 1 is unsupported.

`oy run`, `audit`, `review`, and `enhance` invoke OpenCode 2's noninteractive runner. `oy model` uses the managed API because the pinned beta has no model-list command. With `--json`, noninteractive workflows forward OpenCode's JSON event stream.

## Safety modes

| Mode aliases | `oy run` agent | Host behavior |
|---|---|---|
| `default`, `ask` | `oy` | Edits ask; bash asks. |
| `plan`, `read` | `oy-plan` | Edits and bash denied. |
| `edit`, `accept-edits` | `oy-edit` | Edits allowed; bash asks. |
| `auto`, `auto-approve`, `yolo` | `oy-auto` | Edits and shell allowed in trusted workspaces. |

The OpenCode 2 noninteractive runner cannot pause for an unresolved `ask`; such requests may be rejected rather than prompted. Use `plan` for read-only work, `edit` when file edits are intentionally pre-approved, or `auto` only in a trusted workspace when edits and shell are both pre-approved. The default mode remains conservative and is best for tasks expected not to need mutations.

Restricted audit/review/enhance workflows select their dedicated agents. OpenCode 2's TUI supports session continuation/resume but not per-launch agent or mode flags. Select the desired agent inside the TUI, or use `oy run` when mode selection is required.

## Setup Ownership

Global setup writes `OPENCODE_CONFIG_DIR` when set, otherwise `~/.config/opencode/`; workspace setup writes `OY_ROOT/.opencode/`. An existing `opencode.jsonc` is selected before `opencode.json`. Setup creates seven agents and three canonical workflow skills, then merges config.

oy owns and replaces:

- `mcp.servers.oy`;
- `commands.oy-audit`, `commands.oy-review`, and `commands.oy-enhance`;
- `tool_output.max_bytes` (262,144) and `tool_output.max_lines` (20,000);
- generated agent/skill files containing the oy marker.

Unknown sibling object keys are retained. A non-object `mcp`, `commands`, or `tool_output` value is replaced with an object. The JSON/JSONC config is pretty-serialized, which removes comments and original formatting. Non-generated files at generated Markdown paths are not overwritten.

Setup writes native OpenCode 2 `commands`, `mcp.servers`, timeout objects, and ordered agent permissions. It migrates existing legacy command/MCP entries where behavior is exact, and fails closed on ambiguous legacy permission/provider/plugin and related fields that need a complete manual migration.

Setup and removal use one staged batch and roll back already-committed mutations if a later mutation fails. There is no crash journal or durable persisted recovery. `--remove` removes current owned entries, not historical values that setup previously replaced.

Launch, model, and workflow commands validate that either global or workspace setup is complete, then stop if it is not. They never auto-refresh. `oy upgrade` explicitly runs setup after a successful upgrade. Direct OpenCode use keeps its normal TUI agent selection.

## Bound Workflows

CLI `audit`, `review`, and `enhance` create an inherited typed context that binds `run_id`, session ID, model, scope, focus, output, format, and `max_chunks`. Review refs are resolved to a commit OID before launch, and noninteractive session titles carry `oy:<run-id>`. The OpenCode runner and every managed-API subprocess run with `OY_ROOT` as cwd.

For bound audit/review calls, MCP fixes chunk sizing at the transport-safe default, enforces the maximum chunk count, records the summary input digest, requires 1-based reads in order, rejects changed input, and permits rendering only after all chunks are read. Render calls are rebound to the selected output/format/model/focus/target/chunk metadata. The generated skills remain the canonical protocol, while commands and agents only load them.

## MCP tools

| Tool | Capability |
|---|---|
| `workflow_status` | Return immutable launcher-bound workflow context and current chunk progress; available only in bound runs. |
| `repo_manifest` | Gitignore-aware inventory, approximate token estimates, language summary, optional security index. |
| `repo_chunks` | Ordered repository chunks; summary first, then one-based full chunk retrieval. |
| `git_diff_input` | Ordered `git diff <target>` chunks. |
| `existing_report` | Read an existing generated audit/review report for carry-forward comparison. |
| `sloc` | Source-line counts through optional `tokei`. |
| `outline` | One-file structural definitions through optional Universal Ctags. |
| `sighthound` | Bounded SAST candidates through optional Sighthound embedded rules. |
| `render_audit_report` | Write normalized Markdown or SARIF inside the workspace. |
| `render_review_report` | Write normalized review Markdown inside the workspace. |

Tool names are prefixed by the host's MCP server name, for example `oy_repo_manifest`. Optional tools are advertised only after their executable passes a capability probe. Approximate token estimates currently use oy's byte heuristic; the model value is workflow metadata, not tokenizer selection.

MCP negotiates protocol `2025-06-18` when requested, with `2024-11-05` fallback. Successful metadata calls return text plus `structuredContent`; chunk calls return source text directly; tool failures return `isError: true`. Bound workflows ignore caller attempts to raise `target_tokens`; evidence is sliced so raw chunk text stays below 40 KiB and 3,000 lines, leaving worst-case JSON escaping headroom under the host limit.

## Optional helpers

```bash
mise use --global cargo:tokei
mise use --global github:universal-ctags/ctags
mise use --global rust@1.96 'cargo:https://github.com/Corgea/Sighthound[bin=sighthound,locked=true]@rev:c4608eb2b6ca256daf4dbd1e74aadc3570343685'
# Homebrew alternative for two helpers:
brew install tokei universal-ctags
```

Helpers resolve to canonical absolute paths. Relative `PATH` entries are ignored. Calls use fixed arguments, closed stdin, timeouts, and output limits. Sighthound uses embedded rules, one worker, stable sorting, and finding/byte caps.

Sighthound is optional and source-built at immutable commit `c4608eb2b6ca256daf4dbd1e74aadc3570343685` with Rust 1.96, Cargo `--locked`, and only `bin=sighthound`. Prefer `oy doctor --install-sighthound`; routine `oy doctor --install-missing` deliberately omits it.

## Environment

| Variable | Purpose |
|---|---|
| `OY_ROOT` | Override the workspace root used by oy path boundaries. |
| `OPENCODE_CONFIG_DIR` | Override the global OpenCode config directory used by setup and validation. Relative values resolve from cwd. |
| `OY_OPENCODE` | Override the OpenCode executable. Defaults to `opencode2`; the selected executable must report a supported OpenCode 2 version. |
| `OY_OPENCODE_MODEL` | Override noninteractive workflow model as `provider/model#variant`; the `#variant` suffix is optional. |
| `OY_TOKEI` | Absolute `tokei` executable path. |
| `OY_CTAGS` | Absolute Universal Ctags executable path. |
| `OY_SIGHTHOUND` | Absolute Sighthound executable path. |
| `OY_COLOR` | `auto`, `always`, or `never` color behavior. |
| `NO_COLOR` | Disable color output. |
| `OY_SKIP_SETUP` | Skip `oy setup` in `install.sh`. |
| `OY_MISE_MINIMUM_RELEASE_AGE` | Override the installer's mise release-age filter. |
| `OY_INSTALL_SIGHTHOUND` | Set to `1`/`true` to include the pinned Sighthound source build in `install.sh`. |

## Filesystem and disclosure boundaries

Input paths must resolve under `OY_ROOT` or the current working directory. Output paths must be workspace-relative and may not escape through parent traversal or symlinks. The repository collector skips documented path/file categories and files over 512 KiB; eligible collected files and diff evidence are sliced under the MCP transport bounds.

Repository text returned by MCP may become model input. Fixed external processes include read-only git and optional evidence helpers; oy MCP does not expose arbitrary shell, edit, web, network, or clone capabilities.

For implementation-level detail, read [Tool safety](tool-safety.md) and [Architecture](architecture.md).
