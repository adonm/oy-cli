# Security Policy

## Threat Model

`oy` is not a sandbox. It provides one autonomous OpenCode agent and file-backed deterministic repository evidence/report helpers. A local MCP server remains available only as a compatibility adapter.

opencode owns model traffic, chat UI, sessions, permissions, edits, shell commands, web fetches, and other high-risk tools. The generated `oy` agent intentionally has no permission overrides. Configure those surfaces in OpenCode and review its security guidance for provider credentials and tool permissions.

Native `oy` can:

- write global integration files during explicit setup/upgrade, or `.opencode` files with `oy setup --workspace`,
- launch OpenCode runner, TUI, `mini`, and managed-API processes with `OY_ROOT` as their working directory,
- read workspace files for prepared manifests/chunks, compatibility MCP operations, and optional analysis helpers,
- run read-only `git` commands for diff input,
- write `.oy/runs` evidence/candidate paths and generated audit/review reports inside the workspace,
- write private prepared-run metadata with artifact hashes in the platform user state directory.

Repository text read from prepared artifacts or returned by `oy mcp` can be sent to the configured model provider. Treat selected workspace content as disclosed to that provider.

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

`oy setup` writes a versioned `@oy-cli/opencode` plugin entry under `OPENCODE_CONFIG_DIR` when set, otherwise `~/.config/opencode/`; `oy setup --workspace` writes under `.opencode/`. An existing `opencode.jsonc` is selected before `opencode.json`. Launch/model/workflow commands validate setup but never rewrite it. Use `oy setup --dry-run` before first setup and `oy setup --remove` to remove the current oy integration.

Setup owns string-form `@oy-cli/opencode` package entries. It removes exact legacy generated agent/skill files and command, MCP, and output-budget values while retaining unrelated entries. Modified generated files and object-form oy plugin entries with custom options fail closed rather than being deleted or overwritten.

Setup/removal is a staged file batch that restores already-mutated files if a later commit fails. It has no persistent journal and cannot promise recovery across a process or machine crash. JSONC comments and formatting are still lost during reserialization. Removal deletes the current package entry, exact legacy generated files, and currently owned config values; it does not remember or restore historical pre-setup values. Back up hand-edited config before setup.

opencode owns its own local state. Treat sessions, logs, and config as sensitive because they may contain prompts, source snippets, command output, or provider metadata.

## Reporting A Vulnerability

If you believe you have found a security vulnerability in this project, do not report it in a public GitHub issue or discussion.

Please follow the Government of Western Australia Vulnerability Disclosure Policy:

https://www.wa.gov.au/government/publications/vulnerability-disclosure-policy
