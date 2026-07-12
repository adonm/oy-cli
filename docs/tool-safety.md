# Tool Safety

`oy` no longer exposes a native model-callable tool registry. The host owns the general tool surface: file reads/edits, bash, web fetches, task/subagent tools, questions, todos, repository cloning, and permissions.

This document covers only the deterministic tools served by `oy mcp`.

## MCP Capability Matrix

| MCP tool | Capability | Mutation | Notes |
|---|---|---:|---|
| `workflow_status` | Return inherited run/session/model/scope/output/format/chunk context and progress | No | Always advertised; usable only with bound context |
| `repo_manifest` | Build gitignore-aware file/directory inventory, token estimates, language summary, optional security index | No | Skips dependencies, build outputs, lockfiles, hidden/likely-secret files |
| `repo_chunks` | Build deterministic file/directory chunks and optionally return one chunk's text | No | Used by audit/review agents |
| `git_diff_input` | Build deterministic chunks from `git diff <target>` | No workspace mutation | Runs read-only `git` commands in the workspace |
| `existing_report` | Read a generated audit/review report for carry-forward comparison | No | Defaults to `ISSUES.md` or `REVIEW.md`; path stays inside the workspace |
| `sloc` | Count source lines with external `tokei` | No | Exposed only when `tokei` is on `PATH`; reads paths inside workspace |
| `outline` | Extract source definitions with external Universal Ctags | No | Exposed only when Universal Ctags is on `PATH`; reads one exact source file inside workspace |
| `sighthound` | Scan source with Sighthound embedded SAST rules | No workspace mutation | Explicit-focus audit use only; independent gitignore-aware discovery; fixed JSON output, timeout, finding-count limit, and byte budget |
| `render_audit_report` | Render markdown or SARIF audit report | Yes | Writes only to a validated workspace output path |
| `render_review_report` | Render markdown review report | Yes | Writes only to a validated workspace output path |

Install optional local helper CLIs with:

```bash
mise use --global cargo:tokei
mise use --global github:universal-ctags/ctags
mise use --global rust@1.96 'cargo:https://github.com/Corgea/Sighthound[bin=sighthound,locked=true]@rev:c4608eb2b6ca256daf4dbd1e74aadc3570343685'
# or: brew install tokei universal-ctags
```

Sighthound remains optional and source-built. The mise source pins commit `c4608eb2b6ca256daf4dbd1e74aadc3570343685`, Rust 1.96, Cargo `--locked`, and only `bin=sighthound`. `oy doctor --install-sighthound` performs this build; `oy doctor --install-missing` does not.

Optional helpers are resolved to canonical absolute paths. Relative `PATH` entries are ignored; `OY_TOKEI`, `OY_CTAGS`, and `OY_SIGHTHOUND` can override discovery with an explicit absolute executable path. Probes verify successful version/capability output; calls close stdin and enforce tool-specific time and per-stream output limits. On supported Linux/macOS systems, helpers run in a dedicated process group that is terminated after direct-child exit or timeout so descendants cannot hold captured pipes open. Ctags option-file loading is disabled with `--options=NONE`. Sighthound is restricted to embedded rules and one worker; returned findings are stably sorted, string/array bounded, and size capped. Unsupported-language scopes return an empty status, and all-mode scans fall back to simple analysis when a language pack has no taint rules.

Run `oy doctor` to check whether the optional MCP tools are currently exposed.

## What oy MCP Does Not Provide

`oy mcp` intentionally does not provide:

- arbitrary shell execution (it does run fixed, bounded `git` and optional helper commands)
- source edits
- arbitrary file reads beyond deterministic inputs
- web fetches
- repository cloning
- todo management
- model calls
- session persistence

Use built-in tools and user-managed OpenCode permissions for those capabilities. `oy run` selects the single permission-neutral `oy` agent; `--auto` delegates one-time approvals to OpenCode while explicit denies remain effective. TUI launches require agent selection inside OpenCode 2.

## Filesystem Boundary

Workspace input tools resolve paths under the current workspace root (`OY_ROOT` or cwd). They accept workspace-relative paths and absolute paths that resolve inside the workspace. They reject parent traversal and resolved paths outside the workspace.

Report renderers use `config::resolve_workspace_output_path`, which rejects absolute paths, parent traversal, symlink ancestors that escape the workspace, and symlink final destinations.

When changing this boundary:

- validate before reading or writing,
- canonicalize existing paths,
- keep output writes inside the workspace,
- add tests for traversal and symlink cases,
- avoid broadening file collection without documenting the disclosure impact.

## Disclosure Boundary

The host decides what to send to the selected model. `oy mcp` can return repository text chunks, so returned chunk content may become model input.

The repository collector skips gitignored and hidden paths, common dependency/build directories, lockfiles, generated reports, likely-secret file names, binary/non-UTF-8/empty files, unreadable files, and files larger than 512 KiB. Eligible large collected files and diff evidence are sliced so chunk text stays below 240 KiB and 19,000 lines. “Every chunk” therefore means every collected chunk, not every repository byte. Keep this conservative and make exclusions visible when completeness matters.

Sighthound does not use that collected file list. It has independent gitignore-aware discovery, common directory exclusions, supported-language filtering, and a larger file limit (currently 10 MiB). It can therefore inspect supported hidden source or source files omitted by oy's size limit. Generated auditors invoke it only when focus explicitly asks for Sighthound/SAST. Treat returned snippets as an additional disclosure boundary.

## Permission Boundary

The host handles user approval for its own tools. `oy mcp` report-writing tools are exposed as MCP tools, so permission behavior follows the host MCP/tool configuration. Keep packaged agents explicit about when they call report renderers.

Canonical CLI audit/review workflows use file-backed preparation and finalization rather than MCP chunk calls. Finalization verifies immutable evidence/candidate hashes, workspace/scope/output bindings, and changed prior output, but it cannot prove that the model read every indexed chunk; complete reading is a skill requirement. When explicitly configured for its older bound workflow, MCP still replaces caller-supplied scope/model/chunk sizing, tracks ordered chunk requests, and binds render metadata. OpenCode owns tool permissions in both paths.

MCP negotiates `2025-06-18` when requested and returns successful tool data in both text `content` and `structuredContent` with `isError: false`. Tool execution failures are normal MCP tool results with `isError: true`; malformed protocol methods remain JSON-RPC errors.

## Adding A New MCP Tool

Only add a tool if it is deterministic repo analysis or deterministic report rendering that the host does not already provide.

Checklist:

- no hidden LLM calls,
- no shell/process side effects unless read-only and documented,
- no network access,
- bounded external-process runtime and output,
- workspace path validation near entry,
- clear JSON schema in `src/mcp.rs`,
- packaged agents/skills updated if the tool should be used by workflows,
- tests or smoke coverage for `tools/list` and `tools/call` behavior.
