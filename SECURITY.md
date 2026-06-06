# Security Policy

## Threat Model

`oy` is not a sandbox. It launches opencode with a local MCP server for deterministic repository analysis helpers.

opencode owns model traffic, chat UI, sessions, permissions, edits, shell commands, web fetches, and other high-risk tools. Configure those surfaces there and review its security guidance for provider credentials and tool permissions.

Native `oy` can:

- write global integration files during `oy setup`, or `.opencode` files with `oy setup --workspace`,
- launch the `opencode` process,
- read workspace files for MCP manifests/chunks/SLOC/outlines,
- run read-only `git` commands for diff input,
- write generated audit/review reports inside the workspace.

Repository text returned by `oy mcp` can be sent to the configured model provider. Treat selected workspace content as disclosed to that provider.

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

`oy setup` writes generated files under `~/.config/opencode/` by default. `oy setup --workspace` writes generated files under `.opencode/`. Generated agent and skill files refuse to overwrite non-generated files at generated paths. `opencode.json` is merged so existing user config is preserved except for the generated `mcp.oy` and `command.oy-*` entries that `oy` owns.

opencode owns its own local state. Treat sessions, logs, and config as sensitive because they may contain prompts, source snippets, command output, or provider metadata.

## Reporting A Vulnerability

If you believe you have found a security vulnerability in this project, do not report it in a public GitHub issue or discussion.

Please follow the Government of Western Australia Vulnerability Disclosure Policy:

https://www.wa.gov.au/government/publications/vulnerability-disclosure-policy
