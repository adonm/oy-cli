# Architecture

`oy` is one concise autonomous agent plus a focused audit/review integration layer for OpenCode 2. It does not own model execution, permissions, chat UI, provider routing, shell execution, file editing, web fetching, or a native LLM tool loop. OpenCode and the user own those surfaces.

`oy` owns three things in support of the audit → review → remediate loop:

- explicit setup/removal and launch convenience during migration
- a transitional local stdio MCP server with deterministic repository helpers
- three canonical workflow skills with thin command/agent adapters

## Runtime Flow

```text
user argv/stdin
  -> src/main.rs
  -> oy::run in src/lib.rs
  -> cli::app in src/cli/app.rs
  -> OpenCode integration facade in src/opencode.rs
       -> setup/backup state machine in src/opencode/setup.rs
       -> workflow launcher in src/opencode/runner.rs
       -> selected OpenCode 2 host (`opencode2` or `OY_OPENCODE`), cwd=`OY_ROOT`
            -> TUI, runner, or `mini`, and
            -> authenticated `api v2.*` model/session/health operations

optional compatibility MCP use
  -> an explicitly configured host starts: oy mcp
  -> src/mcp.rs JSON-RPC stdio loop
  -> deterministic helpers in audit/tools modules
```

## Main Modules

| Path | Responsibility |
|---|---|
| `src/main.rs` | Tokio process entrypoint and exit code handling |
| `src/lib.rs` | Small public facade exposing `run` and diagnostics |
| `src/cli/app.rs` | CLI parsing and dispatch |
| `src/opencode.rs` | Thin facade for OpenCode setup, host, API, and workflow modules plus package-asset contract tests |
| `src/opencode/setup.rs` | Package-first setup, namespace migration, persistent backup/rollback, JSONC config ownership, prompting |
| `src/opencode/runner.rs` | Bare TUI launch, task execution, bound audit/review/enhance orchestration, and recovery |
| `src/opencode/host.rs` | OpenCode executable selection, version probing, and v2 contract gate |
| `src/opencode/api.rs` | Bounded adapter for OpenCode 2 model, session, and runtime-health API calls |
| `src/workflow.rs` | Typed inherited run/session/model/scope/output/format/chunk context |
| `src/mcp.rs` | Minimal MCP server over newline-delimited stdio JSON-RPC |
| `src/audit/input.rs` | Gitignore-aware repo collection, manifest/security index, chunking, git diff input |
| `src/audit/findings.rs` | Markdown/structured finding extraction and machine-readable findings block |
| `src/audit/sarif.rs` | SARIF rendering from findings |
| `src/tools/workspace/outline.rs` | Optional structural outline extraction via external Universal Ctags |
| `src/tools/workspace/sloc.rs` | Optional source line counting via external `tokei` |
| `src/tools/workspace/sighthound.rs` | Optional source vulnerability scanning via external Sighthound |
| `src/tools/external.rs` | Shared absolute-path resolution, capability probes, relative-PATH rejection, and bounded process execution |
| `src/cli/config/paths.rs` | Workspace root and safe output-path handling |
| `src/cli/config/atomic_write.rs` | Staged file batches with rollback of committed mutations |

Deleted legacy modules include `src/agent/`, `src/llm/`, native chat/session handling, native provider routing, and the old model-callable tool registry.

## Setup

`oy setup` updates global OpenCode configuration under `~/.config/opencode/` by default:

```text
~/.config/opencode/opencode.json
```

`oy setup --workspace` writes project-local configuration instead:

```text
.opencode/opencode.json
```

Global setup honors `OPENCODE_CONFIG_DIR`; workspace setup remains rooted at `OY_ROOT/.opencode`. In either directory, setup updates an existing `opencode.jsonc` in preference to `opencode.json`. The config pins the `@oy-cli/opencode` npm package to the binary version. OpenCode installs that package into its isolated cache; the package registers the single agent, three skills, and commands without permission overrides.

Setup discovers old direct-file integration by namespace rather than embedded historical contents: direct `agents`, `commands`, and `skills` entries named `oy`, `oy-*`, or `oy.*` are moved to a unique directory under the platform state path at `oy/backups/`. Changed JSON/JSONC files are copied there before oy-namespaced plugin, command, and MCP entries are replaced. Generic settings are preserved rather than interpreted as historical oy state.

Config writes remain a staged rollback-capable batch. If that batch fails, already-moved files are restored; after success, the persistent backup is the recovery path. Backups are created outside the OpenCode config directory so the host cannot discover them as agents or skills, and use mode `0700`. JSONC comments/formatting are lost in the active file but retained in its snapshot.

Bare launch and workflow commands validate that a complete global or workspace integration exists. Missing interactive setup prompts the user before launching; automation receives an error instead. Running sessions must be restarted after an explicit setup change because OpenCode loads package configuration at startup.

## Workflow Binding

For CLI audit/review/enhance runs, oy creates a host session and an inherited typed context containing the run ID, session ID, model, resolved scope, output, format, focus, and `max_chunks`. Diff refs are resolved to commit OIDs before host launch. Noninteractive runner sessions receive the title `oy:<run-id>`.

Canonical audit/review skills call file-backed `prepare`, read the indexed chunks with native OpenCode tools, write candidate files, and call `finalize`. Private state and artifact hashes let finalization reject changed evidence, candidates, prior output, or workspace bindings. Reading every indexed chunk remains a skill requirement; finalization verifies artifact integrity but cannot attest what the model read.

The optional MCP adapter retains its older bound-context enforcement when explicitly used: it overrides caller-supplied scope/model/chunk sizing/render metadata and tracks ordered chunk requests. It is not registered by default and is not the canonical CLI audit/review path.

## Transitional MCP Boundary

`src/mcp.rs` currently implements the small MCP subset needed by the host. New deterministic operations should move toward typed CLI-reusable services and file-backed evidence rather than expanding this transport:

- `initialize`
- `ping`
- `tools/list`
- `tools/call`

The server negotiates MCP `2025-06-18` when requested and retains `2024-11-05` fallback compatibility. Successful tool calls include text `content`, matching `structuredContent`, and `isError: false`; tool failures return normal MCP tool results with `isError: true`. Protocol/method failures remain JSON-RPC errors.

The server is deterministic at the collection/rendering boundary. It returns manifests, chunks, diffs, existing reports, SLOC, outlines, optional Sighthound findings, and report-rendering results. It does not call an LLM. Audit/review conclusions remain model-dependent.

Tools exposed by `oy mcp`:

| Tool | Side effects |
|---|---|
| `workflow_status` | Reads inherited context; always advertised, but returns an error without a bound CLI workflow |
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
| Model/provider traffic | OpenCode host | oy launches the runner or queries the authenticated model API through the selected CLI; OpenCode stores credentials and oy does not |
| UI/sessions/history | OpenCode host | Host owns storage and session lifecycle; oy passes transient session IDs for API continuation/resume |
| File edits/shell/web/repo clone | host | Use host permissions and agents |
| Workspace reads for chunks/SLOC/outlines/SAST | oy MCP | Stay inside `OY_ROOT`/cwd; use fixed helper arguments and bounded processes |
| Report writes | oy MCP | Resolve output path inside workspace and reject symlink destinations |
| Setup/removal writes | oy CLI | User-confirmed or explicit writes; namespace-bounded moves, config snapshots, and rollback on config failure |

## Design Rules

- Do not reintroduce an LLM client or model tool loop in `oy`.
- Keep skills canonical and commands/agents thin.
- Enforce bound workflow invariants in Rust rather than prompt prose alone.
- Keep MCP tools deterministic and narrow while the adapter remains.
- Keep workspace path validation close to reads/writes.
- If the host already has a tool, do not duplicate it in `oy mcp`.
- Update generated setup docs and schemas when changing MCP tool names or command templates.
