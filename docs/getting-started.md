# Getting started

This guide installs oy, configures OpenCode, and creates a first report.

## Before you begin

You need:

- Linux or macOS; use WSL2 on Windows;
- a supported OpenCode 2 release;
- a model provider configured in OpenCode;
- `git` only for target-diff reviews such as `oy review main`.

See [Compatibility](compatibility.md) for exact tested versions and platforms.

## 1. Install

### Recommended: mise installer

```bash
curl -fsSL https://oy.adonm.dev/install.sh | sh

# Or choose a scope without prompting.
curl -fsSL https://oy.adonm.dev/install.sh | sh -s -- --global
curl -fsSL https://oy.adonm.dev/install.sh | sh -s -- --workspace
```

The installer:

1. installs and activates [mise](https://mise.jdx.dev/) with its official bootstrap for bash, zsh, or fish when mise is missing;
2. installs a prebuilt oy release with mise;
3. installs prebuilt `tokei` and Universal Ctags context helpers in the selected mise scope;
4. installs OpenCode 2, registers the version-matched oy plugin, and checks that it loaded.

The default interactive installer asks whether to write mise's global config or `mise.toml` in the current workspace. Noninteractive use defaults to global unless `--workspace` or `OY_INSTALL_SCOPE=workspace` is supplied.

Review [`install.sh`](install.sh) before running it. Set `OY_INSTALL_SCOPE=global|workspace` as an alternative to flags. Set `OY_SKIP_SETUP=1` to install binaries without changing OpenCode integration files.

### Manual install

With mise:

```bash
mise use --global --yes --minimum-release-age 0 github:adonm/oy-cli@0.14.4 node@latest
mise exec node@latest -- npm install -g @opencode-ai/cli@next
mise exec github:adonm/oy-cli@0.14.4 node@latest -- oy setup
```

Or install only the Rust CLI from crates.io, then provide a compatible OpenCode installation yourself:

```bash
cargo install oy-cli --locked
oy setup
```

Rust 1.96+ is required only when building from source.

The installer and `oy doctor --install-missing` use `aqua:XAMPPRocky/tokei@12.1.2`, the newest stable official tokei release that provides binaries, and the release-only archives from `github:universal-ctags/ctags-nightly-build`. They do not install a Rust build toolchain.

## 2. Check the host

Configure a provider using the [OpenCode provider guide](https://v2.opencode.ai/providers), then verify both OpenCode and oy:

```bash
opencode2
oy doctor --check
```

To use Cursor models through OpenCode, run `/connect` in OpenCode, choose **Cursor**, and paste an API key from the Cursor dashboard. Models available to that key appear as `cursor/<id>` in the model picker. `CURSOR_API_KEY` is also recognized.

> `cursor/*` uses Cursor tools outside OpenCode permissions; read the [security policy](https://github.com/adonm/oy-cli/blob/main/SECURITY.md) before using it in an untrusted repository.

`oy doctor --check` validates the OpenCode service, plugin, agent, skills, commands, and model/provider discovery. It does not test or change your permission policy.

If optional context helpers are missing:

```bash
oy doctor --install-missing
```

## 3. Choose setup scope

OpenCode setup runs globally by default. You can preview or change the OpenCode config scope later:

```bash
oy setup --dry-run        # preview global setup
oy setup                  # global OpenCode config
oy setup --workspace      # this repository's .opencode config
oy setup --remove         # back up and remove global oy entries
```

Use global setup for your own workstation. Use `--workspace` when only one repository should load oy. The installer’s `--workspace` flag is separate: it controls where mise writes its tool versions.

Setup defaults Cursor CLI commit and PR attribution to disabled in its global `cli-config.json` when those preferences are absent. Explicit values and unrelated Cursor settings are preserved. This global preference update also applies with `oy setup --workspace`.

Before changing existing oy entries, setup creates a private backup and reports its path. Unchanged config stays byte-for-byte intact. When an oy-owned entry changes, setup preserves unrelated settings but reserializes the active file; the backup retains its original formatting and comments. See [Setup ownership and backups](reference.md#setup-ownership-and-backups).

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
