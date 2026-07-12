# Security Policy

## Threat Model

`oy` is not a sandbox. It provides one autonomous OpenCode agent and file-backed deterministic repository evidence/report helpers.

opencode owns model traffic, chat UI, sessions, permissions, edits, shell commands, web fetches, and other high-risk tools. The packaged `oy` agent intentionally has no permission overrides. Configure those surfaces in OpenCode and review its security guidance for provider credentials and tool permissions.

Native `oy` can:

- write global integration files during explicit setup/upgrade or after an interactive setup confirmation, and write `.opencode` files with `oy setup --workspace`,
- launch OpenCode `run`, TUI, `mini`, and managed-API processes with `OY_ROOT` as their working directory,
- read workspace files for prepared manifests and chunks,
- run read-only `git` commands for diff input,
- write `.oy/runs` evidence/candidate paths and generated audit/review reports inside the workspace,
- write private prepared-run metadata with artifact hashes in the platform state location, falling back to the local-data directory when needed.
- install OpenCode, `tokei`, and Universal Ctags into the user's global mise configuration when `oy doctor --install-missing` is requested.

Repository text read from prepared artifacts can be sent to the configured model provider. Treat selected workspace content as disclosed to that provider.

## Safer Use For Untrusted Repositories

Prefer a disposable container or VM. Start with the workspace read-only, then opt into writes only when you trust the workspace and proposed changes. oy does not publish a container image, so any container must be user-built and supplied with the required OpenCode configuration and credentials.

Avoid mounting the host Docker socket into AI-assisted containers. Docker socket access is usually host-root-equivalent.

## Local Files

`oy setup` writes a versioned `@oy-cli/opencode` plugin entry under `OPENCODE_CONFIG_DIR` when set, otherwise the platform OpenCode config directory; `oy setup --workspace` writes under `.opencode/`. An existing `opencode.jsonc` is selected before `opencode.json`. Interactive launch and workflow commands may offer to run setup when it is missing; noninteractive and JSON calls never do so. Use `oy setup --dry-run` before first setup and `oy setup --remove` to remove the current oy integration.

Setup owns the `oy`, `oy-*`, and `oy.*` direct-file namespace under OpenCode's `agents`, `commands`, and `skills` directories, plus oy package and command config entries. It also removes obsolete oy integration entries from older releases. Those paths are moved without reading or classifying their contents, so locally modified files are not lost. Unrelated files and generic settings such as `tool_output` are retained.

Before changing an existing config, oy copies its exact bytes and permissions into `oy/backups/<unique-id>/` under the platform state location, falling back to the local-data directory when needed; namespaced files are moved there. Backup directories are restricted to mode `0700` because configs can contain credentials or provider metadata. If the config batch fails, moved files are restored. A machine crash can still interrupt the operation, so inspect the reported backup before deleting it. JSONC comments and formatting remain available in the snapshot even though the active config is reserialized.

opencode owns its own local state. Treat sessions, logs, and config as sensitive because they may contain prompts, source snippets, command output, or provider metadata.

## Reporting A Vulnerability

If you believe you have found a security vulnerability in this project, do not report it in a public GitHub issue or discussion.

Please follow the Government of Western Australia Vulnerability Disclosure Policy:

https://www.wa.gov.au/government/publications/vulnerability-disclosure-policy
