# CLI and MCP reference

## Commands

| Command | Behavior |
|---|---|
| `oy audit [FOCUS...]` | Run a restricted security audit; Markdown or SARIF; default `ISSUES.md`. |
| `oy review [TARGET]` | Review the collected workspace or `git diff TARGET`; default `REVIEW.md`. |
| `oy enhance [FOCUS...]` | Fix one finding; use a finding ID as positional focus. |
| `oy setup` | Write global opencode integration. |
| `oy setup --workspace` | Write integration under the current workspace's `.opencode/`. |
| `oy setup --dry-run` | Preview generated integration actions without writing. |
| `oy doctor` | Show executable, config-path, helper, and selected-mode status. |
| `oy` / `oy open ...` | Refresh integration and launch/pass through to opencode. |
| `oy run`, `oy chat`, `oy model` | Compatibility wrappers around opencode. |
| `oy modes` | Print safety-mode aliases and host permission behavior. |
| `oy upgrade` | Upgrade mise-managed `cargo:oy-cli` and `opencode`, then refresh setup. |
| `oy mcp` | Serve MCP over stdio; normally launched by opencode. |

Run `oy <command> --help` for all options and defaults. Unknown top-level actions and flags pass through to opencode unless they begin with a known oy command.

## Safety modes

| Mode aliases | Agent | Host behavior |
|---|---|---|
| `default`, `ask` | `oy` | Edits ask; bash asks. |
| `plan`, `read` | `oy-plan` | Edits and bash denied. |
| `edit`, `accept-edits` | `oy-edit` | Edits allowed; bash asks. |
| `auto`, `auto-approve`, `yolo` | `oy-auto` + `--auto` | Host prompts auto-approved unless explicitly denied. |

These modes apply to general launcher/remediation behavior. Restricted audit and review subagents define their own narrow tool permissions.

## Setup ownership and refresh

Global setup writes `~/.config/opencode/`; workspace setup writes `.opencode/`. It creates seven agents and two skills, then merges config.

oy owns and replaces:

- `mcp.oy`;
- `command.oy-audit`, `command.oy-review`, and `command.oy-enhance`;
- `tool_output.max_bytes` (262,144) and `tool_output.max_lines` (20,000);
- generated agent/skill files containing the oy marker.

Unknown sibling object keys are retained. A non-object `mcp`, `command`, or `tool_output` value is replaced with an object. The JSON/JSONC config is pretty-serialized, which removes comments and original formatting. Non-generated files at generated Markdown paths are not overwritten.

Launch-oriented commands refresh global integration and any detected workspace integration before opencode starts. Direct `opencode` use is not changed to the oy default agent; oy passes `--agent` only for its own launches.

## MCP tools

| Tool | Capability |
|---|---|
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

## Optional helpers

```bash
mise use --global cargo:tokei
mise use --global github:universal-ctags/ctags
mise use --global cargo:https://github.com/Corgea/Sighthound@tag:1.0
# Homebrew alternative for two helpers:
brew install tokei universal-ctags
```

Helpers resolve to canonical absolute paths. Relative `PATH` entries are ignored. Calls use fixed arguments, closed stdin, timeouts, and output limits. Sighthound uses embedded rules, one worker, stable sorting, and finding/byte caps.

Sighthound has no release binary at the pinned 1.0 tag; its mise install builds from source and requires Rust 1.85+.

## Environment

| Variable | Purpose |
|---|---|
| `OY_ROOT` | Override the workspace root used by oy path boundaries. |
| `OY_TOKEI` | Absolute `tokei` executable path. |
| `OY_CTAGS` | Absolute Universal Ctags executable path. |
| `OY_SIGHTHOUND` | Absolute Sighthound executable path. |
| `OY_COLOR` | `auto`, `always`, or `never` color behavior. |
| `NO_COLOR` | Disable color output. |
| `OY_SKIP_SETUP` | Skip `oy setup` in `install.sh`. |
| `OY_MISE_MINIMUM_RELEASE_AGE` | Override the installer's mise release-age filter. |

## Filesystem and disclosure boundaries

Input paths must resolve under `OY_ROOT` or the current working directory. Output paths must be workspace-relative and may not escape through parent traversal or symlinks. The collector skips documented path/file categories and files over 512 KiB.

Repository text returned by MCP may become model input. Fixed external processes include read-only git and optional evidence helpers; oy MCP does not expose arbitrary shell, edit, web, network, or clone capabilities.

For implementation-level detail, read [Tool safety](tool-safety.md) and [Architecture](architecture.md).
