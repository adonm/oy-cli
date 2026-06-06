# Architecture

`oy` is now a small OpenCode integration layer. It does not own model execution, chat UI, sessions, provider routing, shell execution, file editing, web fetching, or a native LLM tool loop. OpenCode owns those surfaces.

`oy` owns three things:

- setup and launch convenience for OpenCode
- a local stdio MCP server with deterministic repository helpers
- compatibility wrappers for old `oy` command names

## Runtime Flow

```text
user argv/stdin
  -> src/main.rs
  -> oy::run in src/lib.rs
  -> cli::app in src/cli/app.rs
  -> opencode wrappers in src/opencode.rs
       -> oy setup writes ~/.config/opencode/* by default
       -> opencode --agent oy...

OpenCode
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
| `src/opencode.rs` | `oy setup`, OpenCode launch, generated agents/skills/commands, legacy wrapper commands |
| `src/mcp.rs` | Minimal MCP server over newline-delimited stdio JSON-RPC |
| `src/audit/input.rs` | Gitignore-aware repo collection, manifest/security index, chunking, git diff input |
| `src/audit/findings.rs` | Markdown/structured finding extraction and machine-readable findings block |
| `src/audit/sarif.rs` | SARIF rendering from findings |
| `src/tools/workspace/sloc.rs` | Source line counting via `tokei` |
| `src/tools/outline.rs` | Tree-sitter structural outline extraction |
| `src/cli/config/paths.rs` | Workspace root and safe output-path handling |
| `src/cli/config/mode.rs` | Legacy safety-mode parsing for wrapper compatibility |

Deleted legacy modules include `src/agent/`, `src/llm/`, native chat/session handling, native provider routing, and the old model-callable tool registry.

## OpenCode Setup

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

The config registers `oy mcp` as a local MCP server and adds OpenCode commands for audit, review, and enhance workflows. Generated primary agents (`oy`, `oy-plan`, `oy-edit`, `oy-auto`) closely match old v0.10 run/chat mode prompts and permissions. `oy` passes `--agent` when launching OpenCode instead of setting `default_agent`, so direct `opencode` usage keeps the user's normal default. Running OpenCode sessions need to be restarted after setup changes because OpenCode loads config at startup.

## MCP Boundary

`src/mcp.rs` implements the small MCP subset needed by OpenCode:

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
| `sloc` | Reads file metadata/content through `tokei` |
| `outline` | Reads one source file and parses it when the default `outline` feature is enabled |
| `render_audit_report` | Writes requested audit report path inside workspace |
| `render_review_report` | Writes requested review report path inside workspace |

## Trust Boundaries

| Boundary | Owner | Posture |
|---|---|---|
| Model prompts/provider traffic | OpenCode | Configure providers and credentials in OpenCode |
| UI/sessions/history | OpenCode | OpenCode owns storage and session lifecycle |
| File edits/shell/web/repo clone | OpenCode | Use OpenCode permissions and agents |
| Workspace reads for chunks/outlines/SLOC | oy MCP | Stay inside `OY_ROOT`/cwd; skip likely secrets and oversized files |
| Report writes | oy MCP | Resolve output path inside workspace and reject symlink destinations |
| Setup writes | oy CLI | Merge generated config into global OpenCode config by default; generated agent/skill files refuse to overwrite non-generated files |

## Design Rules

- Do not reintroduce an LLM client or model tool loop in `oy`.
- Prefer OpenCode agents, commands, skills, and permissions for orchestration.
- Keep MCP tools deterministic and narrow.
- Keep workspace path validation close to reads/writes.
- If OpenCode already has a tool, do not duplicate it in `oy mcp`.
- Update generated setup docs and schemas when changing MCP tool names or command templates.
