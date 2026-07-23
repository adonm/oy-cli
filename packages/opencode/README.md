# @oy-cli/opencode

OpenCode plugin for [oy](https://github.com/adonm/oy-cli): a focused coding agent with repeatable audits, code reviews, and one-finding fixes.

> This package is the OpenCode integration. Install the `oy` CLI as well; the audit/review skills call its local `prepare` and `finalize` commands.

## Install

The recommended path installs matching CLI and plugin versions:

```bash
cargo install oy-cli --locked
oy setup
```

To configure the package manually, add it to an OpenCode JSON/JSONC file:

```jsonc
{
  "$schema": "https://opencode.ai/config.json",
  "plugins": ["@oy-cli/opencode@0.14.0"]
}
```

Restart OpenCode after changing the package version. OpenCode installs package dependencies into its isolated plugin cache.

## What it registers

- one primary agent: `oy`;
- skills: `oy-audit`, `oy-review`, and `oy-enhance`;
- slash commands: `/oy-audit`, `/oy-review`, and `/oy-enhance`.

The plugin defines no permission rules. Models, credentials, permissions, and tools remain controlled by OpenCode and the user.

Audit and review prepare ordered workspace-local evidence, require the agent to read every prepared chunk, and verify the final report. Model conclusions remain nondeterministic.

## Requirements

- OpenCode 2 compatible with this package version;
- the matching `oy` CLI on `PATH`;
- Linux or macOS (use WSL2 on Windows).

See the [oy documentation](https://oy.adonm.dev/) for setup, workflows, compatibility, and security guidance. Maintainer publishing instructions are in [`docs/npm-publishing.md`](../../docs/npm-publishing.md).
