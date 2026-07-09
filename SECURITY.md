# Security Policy

## Threat Model

`oy` is not a sandbox. It launches opencode with a local MCP server for deterministic repository analysis helpers.

opencode owns model traffic, chat UI, sessions, permissions, edits, shell commands, web fetches, and other high-risk tools. Configure those surfaces there and review its security guidance for provider credentials and tool permissions.

Native `oy` can:

- write global integration files during setup and launch-oriented commands, or `.opencode` files with `oy setup --workspace` and when an existing workspace integration is refreshed,
- launch the `opencode` process,
- read workspace files for MCP manifests/chunks/SLOC/outlines and optional Sighthound scans,
- run read-only `git` commands for diff input,
- write generated audit/review reports inside the workspace.

Repository text returned by `oy mcp` can be sent to the configured model provider. Treat selected workspace content as disclosed to that provider.

Sighthound runs only when its MCP tool is called; the generated auditor calls it only for explicit Sighthound/SAST focus. Sighthound uses independent gitignore-aware file discovery and its own size limit rather than oy's manifest/chunk collector exclusions, so it can return snippets from supported hidden or larger source files. Treat a requested Sighthound scope as disclosed to the provider.

## Safer Use For Untrusted Repositories

Prefer a disposable container or VM. Start with restrictive permissions, then opt into writes only when you trust the workspace and proposed changes.

```bash
docker run --rm -it \
  -v "$PWD:/workspace:ro" \
  -w /workspace \
  oy-image oy
```

For audit/review report writing, mount the workspace read-write but keep permissions conservative:

```bash
docker run --rm -it \
  -v "$PWD:/workspace:rw" \
  -w /workspace \
  oy-image oy setup
```

Avoid mounting the host Docker socket into AI-assisted containers. Docker socket access is usually host-root-equivalent.

## Local Files

`oy setup` writes generated files under `~/.config/opencode/` by default. `oy setup --workspace` writes generated files under `.opencode/`. Launch-oriented wrappers refresh the global integration before starting opencode and refresh a detected workspace integration. Use `oy setup --dry-run` before first setup to inspect intended changes.

Generated agent and skill files refuse to overwrite non-generated files at generated paths. The `opencode.json` merge owns and replaces `mcp.oy`, `command.oy-audit`, `command.oy-review`, `command.oy-enhance`, and `tool_output.max_bytes`/`max_lines`; unknown sibling object keys are retained. Non-object values at `mcp`, `command`, or `tool_output` are replaced with objects. Config is parsed and pretty-serialized, so JSONC comments and original formatting are not preserved. Back up a hand-edited config before setup.

opencode owns its own local state. Treat sessions, logs, and config as sensitive because they may contain prompts, source snippets, command output, or provider metadata.

## Reporting A Vulnerability

If you believe you have found a security vulnerability in this project, do not report it in a public GitHub issue or discussion.

Please follow the Government of Western Australia Vulnerability Disclosure Policy:

https://www.wa.gov.au/government/publications/vulnerability-disclosure-policy
