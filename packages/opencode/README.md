# @oy-cli/opencode

OpenCode plugin for [oy](https://github.com/adonm/oy-cli): a focused coding agent with repeatable audits, code reviews, and one-finding fixes.

> This package is the OpenCode integration. Install the `oy` CLI as well; the audit/review skills call its local `prepare` and `finalize` commands.

## Install

The recommended path installs matching CLI and plugin versions:

```bash
cargo install oy-cli --locked
oy setup
```

`oy setup` registers the matching package version in OpenCode config so OpenCode installs the plugin and its Cursor SDK dependencies. To configure the package manually instead, add it to an OpenCode JSON/JSONC file:

```jsonc
{
  "$schema": "https://opencode.ai/config.json",
  "plugins": ["@oy-cli/opencode@0.14.3"]
}
```

Restart OpenCode after changing the package version.

The package pins `undici@6.28.0` through npm overrides because the current
Cursor SDK release still requests the vulnerable 5.x line indirectly through
Connect. Run `npm audit --omit=dev` after dependency changes.

## What it registers

- one primary agent: `oy`;
- skills: `oy-audit`, `oy-review`, and `oy-enhance`;
- slash commands: `/oy-audit`, `/oy-review`, and `/oy-enhance`;
- a Cursor provider and `cursor/*` models backed by `@stablekernel/opencode-cursor` and the official Cursor SDK.

Run `/connect`, choose **Cursor**, and paste an API key from the Cursor dashboard. The live models available to that key then replace the fallback Cursor catalog.

Oy defaults the provider's idle-stream watchdog to 1,200,000 ms (20 minutes). Set `OPENCODE_CURSOR_STALL_MS` explicitly to override it, or to `0` to disable it.

The plugin defines no permission rules. `cursor/*` uses Cursor tools outside OpenCode permissions; read the [security policy](https://github.com/adonm/oy-cli/blob/main/SECURITY.md) before selecting one.

Audit and review prepare ordered workspace-local evidence, require the agent to read every prepared chunk, and verify the final report. Model conclusions remain nondeterministic.

## Requirements

- OpenCode 2 compatible with this package version;
- the matching `oy` CLI on `PATH`;
- Linux or macOS (use WSL2 on Windows).

See the [oy documentation](https://oy.adonm.dev/) for setup, workflows, compatibility, and security guidance. Maintainer publishing instructions are in [`docs/npm-publishing.md`](../../docs/npm-publishing.md).
