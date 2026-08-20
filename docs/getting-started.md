# Getting started

This guide installs oy, installs the skills, and creates a first report.

## Before you begin

You need:

- Linux or macOS; use WSL2 on Windows;
- any agent that reads Agent Skills (OpenCode, Cursor, Codex, Copilot, or Gemini CLI);
- a model provider configured in that agent;
- `git` only for target-diff reviews such as reviewing against `main`.

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
4. runs `oy setup`, which writes the oy skills under `~/.agents/skills/` and removes any legacy OpenCode plugin state.

The default interactive installer asks whether to write mise's global config or `mise.toml` in the current workspace. Noninteractive use defaults to global unless `--workspace` or `OY_INSTALL_SCOPE=workspace` is supplied.

Review [`install.sh`](install.sh) before running it. Set `OY_INSTALL_SCOPE=global|workspace` as an alternative to flags. Set `OY_SKIP_SETUP=1` to install binaries without changing the skills installation.

### Manual install

With mise:

```bash
mise use --global --yes --minimum-release-age 0 github:adonm/oy-cli@0.14.13
mise exec github:adonm/oy-cli@0.14.13 -- oy setup
```

Or install only the Rust CLI from crates.io:

```bash
cargo install oy-cli --locked
oy setup
```

Rust 1.96+ is required only when building from source.

The installer and `oy doctor --install-missing` use `aqua:XAMPPRocky/tokei@12.1.2`, the newest stable official tokei release that provides binaries, and the release-only archives from `github:universal-ctags/ctags-nightly-build`. They do not install a Rust build toolchain.

## 2. Finish agent setup

Run `oy doctor --check` to verify the skills:

```bash
oy doctor --check
```

Then ask your agent to finish host-specific setup:

```text
run the oy-setup skill
```

The skill verifies discovery, offers to copy the skills to any host-specific
directory this agent prefers (for example `.claude/skills`), installs the oy
persona (improving the default agent or creating an `oy` agent), and reruns
`oy doctor --check`.

## 3. Choose setup scope

Skills install globally by default. You can preview or change the scope later:

```bash
oy setup --dry-run        # preview global setup
oy setup                  # skills under ~/.agents/skills
oy setup --workspace      # skills under this repository's .agents/skills
oy setup --remove         # back up and remove oy-owned skills and legacy config
```

Use global setup for your own workstation. Use `--workspace` when only one repository should load oy. The installer's `--workspace` flag is separate: it controls where mise writes its tool versions.

Before changing existing oy-owned files, setup creates a private backup and reports its path. Unchanged files stay byte-for-byte intact. User-modified skill files without the setup marker are preserved. Setup also removes the obsolete OpenCode plugin package cache. See [Setup ownership and backups](reference.md#setup-ownership-and-backups).

## 4. Create a first report

Start in a small or medium repository and ask your agent:

```text
audit this repository with the oy-audit skill
```

The command writes `ISSUES.md`. Read the findings alongside the documented collection exclusions before acting on them.

For a code-quality review:

```text
review this repository with the oy-review skill     # whole workspace
review the diff against main with the oy-review skill   # current work vs main
```

To fix one finding:

```text
use the oy-enhance skill to fix audit-0123456789abcdef
```

Rerun the originating audit or review to confirm the finding against current code.

## If something fails

- Run `oy doctor` for paths, versions, and missing tools.
- Run `oy doctor --check` to validate the skills installation.
- Confirm the skills are visible to your agent; see [Compatibility](compatibility.md) for the directory each agent reads.
- Rerun `oy setup`, then ask your agent to run the `oy-setup` skill again.

## Next

- [Choose scopes and understand reports](workflows.md)
- [See report and CI examples](examples.md)
- [Look up every command and environment variable](reference.md)
