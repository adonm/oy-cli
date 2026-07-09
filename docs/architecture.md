# Architecture

`oy` is a focused audit/review integration layer for opencode. It does not own model execution, chat UI, sessions, provider routing, shell execution, file editing, web fetching, or a native LLM tool loop. The host owns those surfaces.

`oy` owns three things in support of the audit → review → remediate loop:

- setup and launch convenience
- a local stdio MCP server with deterministic repository helpers
- focused convenience wrappers plus passthrough to opencode for unknown top-level actions/flags

## Runtime Flow

```text
user argv/stdin
  -> src/main.rs
  -> oy::run in src/lib.rs
  -> cli::app in src/cli/app.rs
  -> opencode wrappers in src/opencode.rs
       -> oy setup writes ~/.config/opencode/* by default
       -> opencode --agent oy...

opencode
  -> starts local MCP command: oy mcp
  -> src/mcp.rs JSON-RPC stdio loop
  -> deterministic helpers in audit/tools modules
```

## Main Modules

| Path | Responsibility |
|---|---|
| `src/main.rs` | Tokio process entrypoint and exit code handling |
| `src/lib.rs` | Small public facade exposing `run` and diagnostics |
| `src/cli/app.rs` | CLI parsing and dispatch |
| `src/opencode.rs` | `oy setup`, launch, generated agents/skills/commands, opencode convenience wrappers |
| `src/mcp.rs` | Minimal MCP server over newline-delimited stdio JSON-RPC |
| `src/audit/input.rs` | Gitignore-aware repo collection, manifest/security index, chunking, git diff input |
| `src/audit/findings.rs` | Markdown/structured finding extraction and machine-readable findings block |
| `src/audit/sarif.rs` | SARIF rendering from findings |
| `src/tools/workspace/outline.rs` | Optional structural outline extraction via external Universal Ctags |
| `src/tools/workspace/sloc.rs` | Optional source line counting via external `tokei` |
| `src/tools/workspace/sighthound.rs` | Optional source vulnerability scanning via external Sighthound |
| `src/tools/external.rs` | Shared absolute-path resolution, capability probes, relative-PATH rejection, and bounded process execution |
| `src/cli/config/paths.rs` | Workspace root and safe output-path handling |
| `src/cli/config/mode.rs` | Safety-mode parsing for launcher convenience modes |

Deleted legacy modules include `src/agent/`, `src/llm/`, native chat/session handling, native provider routing, and the old model-callable tool registry.

## Setup

`oy setup` writes global files under `~/.config/opencode/` by default:

```text
~/.config/opencode/opencode.json
~/.config/opencode/agents/oy.md
~/.config/opencode/agents/oy-plan.md
~/.config/opencode/agents/oy-edit.md
~/.config/opencode/agents/oy-auto.md
~/.config/opencode/agents/oy-auditor.md
~/.config/opencode/agents/oy-reviewer.md
~/.config/opencode/agents/oy-enhancer.md
~/.config/opencode/skills/oy-audit/SKILL.md
~/.config/opencode/skills/oy-review/SKILL.md
```

`oy setup --workspace` writes the same generated files under `.opencode/` for project-local overrides:

```text
.opencode/opencode.json
.opencode/agents/oy.md
.opencode/agents/oy-plan.md
.opencode/agents/oy-edit.md
.opencode/agents/oy-auto.md
.opencode/agents/oy-auditor.md
.opencode/agents/oy-reviewer.md
.opencode/agents/oy-enhancer.md
.opencode/skills/oy-audit/SKILL.md
.opencode/skills/oy-review/SKILL.md
```

The config registers `oy mcp` as a local MCP server and adds commands for audit, review, and enhance workflows. Generated primary agents (`oy`, `oy-plan`, `oy-edit`, `oy-auto`) support launcher compatibility, but restricted audit/review agents are the product's primary direction. `oy` passes `--agent` when launching instead of setting `default_agent`, so direct `opencode` usage keeps the user's normal default. Running sessions need to be restarted after setup changes because config loads at startup.

Launch-oriented commands refresh the global integration before starting opencode and refresh workspace integration files when an existing oy workspace integration is detected. The config merge replaces oy-owned `mcp.oy`, `command.oy-*`, and `tool_output.max_bytes`/`max_lines` values. Unknown sibling object keys survive, but parsing and pretty-serialization remove JSONC comments and original formatting. Generated Markdown files refuse to replace files without the generated marker.

## MCP Boundary

`src/mcp.rs` implements the small MCP subset needed by the host:

- `initialize`
- `ping`
- `tools/list`
- `tools/call`

The server is intentionally deterministic at the collection/rendering boundary. It returns manifests, chunks, diffs, existing reports, SLOC, outlines, optional Sighthound findings, and report-rendering results. It does not call an LLM. Audit/review conclusions remain model-dependent.

Tools exposed by `oy mcp`:

| Tool | Side effects |
|---|---|
| `repo_manifest` | Reads workspace files |
| `repo_chunks` | Reads workspace files |
| `git_diff_input` | Runs `git diff`/`git rev-parse` read-only commands |
| `existing_report` | Reads a generated `ISSUES.md` or `REVIEW.md` for carry-forward comparison |
| `sloc` | Reads file metadata/content through external `tokei`; exposed only when `tokei` is on `PATH` |
| `outline` | Reads one source file through external Universal Ctags; exposed only when Universal Ctags is on `PATH` |
| `sighthound` | Reads a workspace directory through external Sighthound embedded rules; exposed only when Sighthound is on `PATH` |
| `render_audit_report` | Writes requested audit report path inside workspace |
| `render_review_report` | Writes requested review report path inside workspace |

Sighthound uses its own gitignore-aware discovery and file-size limit rather than the manifest/chunk collector's exact exclusions. The generated auditor invokes it only for explicit Sighthound/SAST focus, and its returned snippets form an additional disclosure boundary.

## Trust Boundaries

| Boundary | Owner | Posture |
|---|---|---|
| Model prompts/provider traffic | host | Configure providers and credentials there |
| UI/sessions/history | host | Host owns storage and session lifecycle |
| File edits/shell/web/repo clone | host | Use host permissions and agents |
| Workspace reads for chunks/SLOC/outlines/SAST | oy MCP | Stay inside `OY_ROOT`/cwd; use fixed helper arguments and bounded processes |
| Report writes | oy MCP | Resolve output path inside workspace and reject symlink destinations |
| Setup/launch refresh writes | oy CLI | Replace documented oy-owned config values; generated agent/skill files refuse to overwrite non-generated files |

## Design Rules

- Do not reintroduce an LLM client or model tool loop in `oy`.
- Prefer host agents, commands, skills, and permissions for orchestration.
- Keep MCP tools deterministic and narrow.
- Keep workspace path validation close to reads/writes.
- If the host already has a tool, do not duplicate it in `oy mcp`.
- Update generated setup docs and schemas when changing MCP tool names or command templates.
