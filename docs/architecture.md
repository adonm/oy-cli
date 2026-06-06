# Architecture

`oy` is now a small opencode integration layer. It does not own model execution, chat UI, sessions, provider routing, shell execution, file editing, web fetching, or a native LLM tool loop. The host owns those surfaces.

`oy` owns three things:

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

The config registers `oy mcp` as a local MCP server and adds commands for audit, review, and enhance workflows. Generated primary agents (`oy`, `oy-plan`, `oy-edit`, `oy-auto`) closely match old v0.10 run/chat mode prompts and permissions. `oy` passes `--agent` when launching instead of setting `default_agent`, so direct `opencode` usage keeps the user's normal default. Running sessions need to be restarted after setup changes because config loads at startup.

## MCP Boundary

`src/mcp.rs` implements the small MCP subset needed by the host:

- `initialize`
- `ping`
- `tools/list`
- `tools/call`

The server is intentionally deterministic. It returns manifests, chunks, diffs, SLOC, outlines, and report-rendering results. It does not call an LLM.

Tools exposed by `oy mcp`:

| Tool | Side effects |
|---|---|
| `repo_manifest` | Reads workspace files |
| `repo_chunks` | Reads workspace files |
| `git_diff_input` | Runs `git diff`/`git rev-parse` read-only commands |
| `sloc` | Reads file metadata/content through external `tokei`; exposed only when `tokei` is on `PATH` |
| `outline` | Reads one source file through external Universal Ctags; exposed only when Universal Ctags is on `PATH` |
| `render_audit_report` | Writes requested audit report path inside workspace |
| `render_review_report` | Writes requested review report path inside workspace |

## Trust Boundaries

| Boundary | Owner | Posture |
|---|---|---|
| Model prompts/provider traffic | host | Configure providers and credentials there |
| UI/sessions/history | host | Host owns storage and session lifecycle |
| File edits/shell/web/repo clone | host | Use host permissions and agents |
| Workspace reads for chunks/SLOC/outlines | oy MCP | Stay inside `OY_ROOT`/cwd; skip likely secrets and oversized files |
| Report writes | oy MCP | Resolve output path inside workspace and reject symlink destinations |
| Setup writes | oy CLI | Merge generated config into global config by default; generated agent/skill files refuse to overwrite non-generated files |

## Design Rules

- Do not reintroduce an LLM client or model tool loop in `oy`.
- Prefer host agents, commands, skills, and permissions for orchestration.
- Keep MCP tools deterministic and narrow.
- Keep workspace path validation close to reads/writes.
- If the host already has a tool, do not duplicate it in `oy mcp`.
- Update generated setup docs and schemas when changing MCP tool names or command templates.
