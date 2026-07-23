# Getting started

This guide installs oy, configures an OpenCode or Cursor integration, and creates a first report.

## Before you begin

You need:

- Linux or macOS; use WSL2 on Windows;
- a supported OpenCode 2 release or Cursor installation;
- a model provider configured in the selected host;
- `git` only for target-diff reviews such as `oy review main`.

See [Compatibility](compatibility.md) for exact tested versions and platforms.

## 1. Install

### Recommended: mise installer

```bash
# OpenCode 2 (default)
curl -fsSL https://oy.adonm.dev/install.sh | sh

# Cursor CLI and Cursor integration
curl -fsSL https://oy.adonm.dev/install.sh | sh -s -- --cursor

# Both hosts and integrations
curl -fsSL https://oy.adonm.dev/install.sh | sh -s -- --both
```

Every target:

1. installs and activates [mise](https://mise.jdx.dev/) with its official bootstrap for bash, zsh, or fish when mise is missing;
2. installs a prebuilt oy release with mise;
3. installs prebuilt `tokei` and Universal Ctags context helpers.

The default OpenCode target also provisions the latest Node.js, installs OpenCode 2 with the exact npm package and channel documented upstream, registers the matching `@oy-cli/opencode` plugin, and checks that it loaded. The Cursor target runs Cursor's official CLI installer, verifies `agent --version`, and installs the global Cursor oy assets. `--both` performs both paths.

Review [`install.sh`](install.sh) before running it. Set `OY_INSTALL_TARGET=cursor|both` as an alternative to flags. Set `OY_SKIP_SETUP=1` to install binaries without changing host integration files.

### Manual install

With mise:

```bash
mise use --global --yes --minimum-release-age 0 github:adonm/oy-cli@0.14.0 node@latest
mise exec node@latest -- npm install -g @opencode-ai/cli@next
mise exec github:adonm/oy-cli@0.14.0 node@latest -- oy setup
```

Or install only the Rust CLI from crates.io, then provide a compatible OpenCode installation yourself:

```bash
cargo install oy-cli --locked
oy setup
```

Rust 1.96+ is required only when building from source.

Cursor does not publish an official mise package or stable version-index API. For a manual Cursor-only installation, use its supported installer, install oy separately, then set up the assets:

```bash
curl https://cursor.com/install -fsS | bash
cargo install oy-cli --locked
oy setup --cursor
agent --version
```

The installer and `oy doctor --install-missing` use `aqua:XAMPPRocky/tokei@12.1.2`, the newest stable official tokei release that provides binaries, and the release-only archives from `github:universal-ctags/ctags-nightly-build`. They do not install a Rust build toolchain.

## 2. Check the host

Configure a provider using the [OpenCode provider guide](https://v2.opencode.ai/providers), then verify both OpenCode and oy:

```bash
opencode2
oy doctor --check
```

`oy doctor --check` validates the OpenCode service, plugin, agent, skills, commands, and model/provider discovery. It does not test or change your permission policy.

For Cursor, run `agent --version` and `oy setup --cursor --dry-run` to inspect the selected paths. Cursor discovers the installed rule, subagent, and skills when you start a new Agent chat; `doctor --check` remains specific to the OpenCode runtime integration.

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
oy setup --cursor          # global ~/.cursor assets
oy setup --cursor --workspace
oy setup --cursor --remove
```

Use global setup for your own workstation. Use `--workspace` when only one repository should load oy. Cursor setup installs an always-applied rule, an `oy` subagent, and `oy-audit`, `oy-review`, and `oy-enhance` skills. Cursor exposes installed skills as slash commands.

Before changing existing oy entries, setup creates a private backup and reports its path. It preserves unrelated OpenCode settings, but JSON/JSONC formatting and comments remain only in the backup because the active file is reserialized. See [Setup ownership and backups](reference.md#setup-ownership-and-backups).

Restart OpenCode after changing a plugin version or setup scope. Start a new Cursor Agent chat after changing Cursor assets.

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
