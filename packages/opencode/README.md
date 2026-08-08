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
  "plugins": ["@oy-cli/opencode@0.14.5"]
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

OpenCode 2 accepts Cursor through an authenticated loopback OpenAI Responses bridge. The bridge binds only to `127.0.0.1`, uses a random per-process token, and forwards each request into the official Cursor local-agent runtime. It preserves one Cursor agent per OpenCode session, carries Cursor token/cache usage back to OpenCode, mirrors the three bundled oy skills into the active workspace, and exposes Cursor-native `explore`, `general`, and `reviewer` subagents. Select the `plan` model variant to use Cursor plan mode.

For a live smoke test against this checkout, run `CURSOR_API_KEY=... just opencode-dev`, connect Cursor if prompted, select a `cursor/*` model, and ask it to read a file without editing. Set `OPENCODE_CURSOR_DEBUG=1` to inspect provider retry/session diagnostics; never paste the key into a config file or report output.

Cursor-specific settings under `providers.cursor.request.body` are forwarded to the local agent. Supported settings include `sandbox`, `autoReview`, `settingSources`, `agents`, `mcpServers`, `session`, `systemPrompt`, `toolDisplay`, and `transport`. Static MCP launch specifications are supported; OpenCode's live MCP connection state and permission prompts cannot cross the Cursor boundary in the current V2 plugin API.

The plugin defines no permission rules. `cursor/*` uses Cursor tools outside OpenCode permissions; read the [security policy](https://github.com/adonm/oy-cli/blob/main/SECURITY.md) before selecting one.

Audit and review prepare ordered workspace-local evidence, require the agent to read every prepared chunk, and verify the final report. Model conclusions remain nondeterministic.

## Requirements

- OpenCode 2 compatible with this package version;
- the matching `oy` CLI on `PATH`;
- Linux or macOS (use WSL2 on Windows).

See the [oy documentation](https://oy.adonm.dev/) for setup, workflows, compatibility, and security guidance. Maintainer publishing instructions are in [`docs/npm-publishing.md`](../../docs/npm-publishing.md).
