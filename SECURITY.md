# Security Policy

## Threat Model

`oy` is not a sandbox. It launches opencode with a local MCP server for deterministic repository analysis helpers.

opencode owns model traffic, chat UI, sessions, permissions, edits, shell commands, web fetches, and other high-risk tools. Configure those surfaces there and review its security guidance for provider credentials and tool permissions.

Native `oy` can:

- write global integration files during explicit setup/upgrade, or `.opencode` files with `oy setup --workspace`,
- launch OpenCode runner, TUI, `mini`, and managed-API processes with `OY_ROOT` as their working directory,
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

`oy setup` writes generated files under `OPENCODE_CONFIG_DIR` when set, otherwise `~/.config/opencode/`; `oy setup --workspace` writes under `.opencode/`. An existing `opencode.jsonc` is selected before `opencode.json`. Launch/model/workflow commands validate setup but never rewrite it. Use `oy setup --dry-run` before first setup and `oy setup --remove` to remove the current oy integration.

Generated agent and skill files refuse to overwrite non-generated files at generated paths. The config merge owns and replaces `mcp.servers.oy`, `commands.oy-audit`, `commands.oy-review`, `commands.oy-enhance`, and `tool_output.max_bytes`/`max_lines`; unknown sibling object keys are retained. Legacy command/MCP entries are migrated where behavior is exact; ambiguous legacy fields fail closed.

Setup/removal is a staged multi-file batch that restores already-mutated files if a later commit fails. It has no persistent journal and cannot promise recovery across a process or machine crash. JSONC comments and formatting are still lost during reserialization. Removal deletes oy's generated files and currently owned config values; it does not remember or restore historical pre-setup values. Back up hand-edited config before setup.

opencode owns its own local state. Treat sessions, logs, and config as sensitive because they may contain prompts, source snippets, command output, or provider metadata.

## Reporting A Vulnerability

If you believe you have found a security vulnerability in this project, do not report it in a public GitHub issue or discussion.

Please follow the Government of Western Australia Vulnerability Disclosure Policy:

https://www.wa.gov.au/government/publications/vulnerability-disclosure-policy
