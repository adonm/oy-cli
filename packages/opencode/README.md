# @oy-cli/opencode

OpenCode 2 package for the [oy](https://github.com/adonm/oy-cli) deterministic evidence CLI.

Install the `oy` binary, then add the package to OpenCode:

```jsonc
{
  "$schema": "https://opencode.ai/config.json",
    "plugins": ["@oy-cli/opencode@0.13.2"]
}
```

The package registers:

- one concise `oy` primary agent without permission overrides;
- `oy-audit`, `oy-review`, and `oy-enhance` skills;
- `/oy-audit`, `/oy-review`, and `/oy-enhance` commands.

Audit and review use `oy audit|review prepare` to write bounded workspace-local evidence files, OpenCode's native `read` and edit tools, and `oy audit|review finalize` to recheck evidence and normalize the final report. OpenCode and the user own permissions.

Maintainer publishing instructions are documented in [`docs/npm-publishing.md`](../../docs/npm-publishing.md).
