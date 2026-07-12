# Getting started

This guide installs oy, checks its OpenCode integration, and creates a first report.

## Before you begin

You need:

- Linux or macOS; use WSL2 on Windows;
- a supported OpenCode 2 release;
- an OpenCode model provider configured and working;
- `git` only for target-diff reviews such as `oy review main`.

See [Compatibility](compatibility.md) for exact tested versions and platforms.

## 1. Install

### Recommended: mise installer

```bash
curl -fsSL https://oy.adonm.dev/install.sh | sh
```

The installer:

1. installs compatible versions of oy and OpenCode with [mise](https://mise.jdx.dev/);
2. registers the matching `@oy-cli/opencode` plugin;
3. installs optional `tokei` and Universal Ctags context helpers;
4. checks that OpenCode loaded the plugin.

Review [`install.sh`](install.sh) before running it. Set `OY_SKIP_SETUP=1` to install without changing OpenCode configuration.

### Manual install

With mise:

```bash
mise use --global --yes --minimum-release-age 0 node@24 cargo-binstall cargo:oy-cli@0.13.6 npm:@opencode-ai/cli@next
oy setup
```

Or install only the Rust CLI from crates.io, then provide a compatible OpenCode installation yourself:

```bash
cargo install oy-cli --locked
oy setup
```

Rust 1.96+ is required only when building from source.

## 2. Check OpenCode

Configure a provider using the [OpenCode provider guide](https://v2.opencode.ai/providers), then verify both OpenCode and oy:

```bash
opencode2
oy doctor --check
```

`oy doctor --check` validates the OpenCode service, plugin, agent, skills, commands, and model/provider discovery. It does not test or change your permission policy.

If optional context helpers are missing:

```bash
oy doctor --install-missing
```

## 3. Choose setup scope

The installer runs global setup by default. You can preview or change the scope later:

```bash
oy setup --dry-run        # preview global setup
oy setup                  # global OpenCode config
oy setup --workspace      # this repository's .opencode config
oy setup --remove         # back up and remove global oy entries
```

Use global setup for your own workstation. Use `--workspace` when only one repository should load oy.

Before changing existing oy entries, setup creates a private backup and reports its path. It preserves unrelated OpenCode settings, but JSON/JSONC formatting and comments remain only in the backup because the active file is reserialized. See [Setup ownership and backups](reference.md#setup-ownership-and-backups).

Restart OpenCode after changing a plugin version or setup scope.

## 4. Create a first report

Start in a small or medium repository:

```bash
cd your-repository
oy audit
```

The command writes `ISSUES.md`. Read the findings alongside the documented collection exclusions before acting on them.

For a code-quality review:

```bash
oy review             # whole workspace
oy review main        # current work compared with main
```

To fix one finding:

```bash
oy enhance <finding-id>
```

Rerun the originating audit or review to confirm the finding against current code.

## If something fails

- Run `oy doctor` for paths, versions, and missing tools.
- Run `oy doctor --check` for effective plugin/runtime validation.
- Restart the OpenCode service with `opencode2 service restart`.
- Check [Compatibility](compatibility.md) before overriding `OY_OPENCODE`.
- Use [OpenCode troubleshooting](https://v2.opencode.ai/troubleshooting) for service, provider, and session problems.

## Next

- [Choose scopes and understand reports](workflows.md)
- [See report and CI examples](examples.md)
- [Look up every command and environment variable](reference.md)
