# Tool Safety

`oy` no longer exposes a native model-callable tool registry. The host owns the general tool surface: file reads/edits, bash, web fetches, task/subagent tools, questions, todos, repository cloning, and permissions.

This document covers only the deterministic tools served by `oy mcp`.

## MCP Capability Matrix

| MCP tool | Capability | Mutation | Notes |
|---|---|---:|---|
| `repo_manifest` | Build gitignore-aware file/directory inventory, token estimates, language summary, optional security index | No | Skips dependencies, build outputs, lockfiles, hidden/likely-secret files |
| `repo_chunks` | Build deterministic file/directory chunks and optionally return one chunk's text | No | Used by audit/review agents |
| `git_diff_input` | Build deterministic chunks from `git diff <target>` | No workspace mutation | Runs read-only `git` commands in the workspace |
| `sloc` | Count source lines with external `tokei` | No | Exposed only when `tokei` is on `PATH`; reads paths inside workspace |
| `outline` | Extract source definitions with external Universal Ctags | No | Exposed only when Universal Ctags is on `PATH`; reads one exact source file inside workspace |
| `render_audit_report` | Render markdown or SARIF audit report | Yes | Writes only to a validated workspace output path |
| `render_review_report` | Render markdown review report | Yes | Writes only to a validated workspace output path |

Install optional local helper CLIs with:

```bash
mise use cargo:tokei
mise use aqua:universal-ctags/ctags
# or: brew install tokei universal-ctags
```

Run `oy doctor` to check whether the optional MCP tools are currently exposed.

## What oy MCP Does Not Provide

`oy mcp` intentionally does not provide:

- shell execution
- source edits
- arbitrary file reads beyond deterministic inputs
- web fetches
- repository cloning
- todo management
- model calls
- session persistence

Use built-in tools and permissions for those capabilities. When launched through `oy`, the generated `oy`, `oy-plan`, `oy-edit`, and `oy-auto` agents map old oy safety modes onto host permissions.

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

The collector skips common dependency/build directories and likely-secret file names by default. Keep this conservative.

## Permission Boundary

The host handles user approval for its own tools. `oy mcp` report-writing tools are exposed as MCP tools, so permission behavior follows the host MCP/tool configuration. Keep generated agents explicit about when they call report renderers.

## Adding A New MCP Tool

Only add a tool if it is deterministic repo analysis or deterministic report rendering that the host does not already provide.

Checklist:

- no hidden LLM calls,
- no shell/process side effects unless read-only and documented,
- no network access,
- workspace path validation near entry,
- clear JSON schema in `src/mcp.rs`,
- generated agents/skills updated if the tool should be used by workflows,
- tests or smoke coverage for `tools/list` and `tools/call` behavior.
