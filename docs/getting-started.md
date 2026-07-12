# Getting started

Set up oy as a focused audit/review extension for OpenCode 2.

## Requirements

- Linux or macOS; on Windows, use WSL2
- OpenCode beta `0.0.0-next-15353`, or a tagged OpenCode 2.x release
- `git` for target-diff reviews
- Rust 1.96+ only when building from source

oy does not store provider credentials. Follow OpenCode's [provider setup](https://v2.opencode.ai/providers), then use `opencode2` once to verify the model works. Noninteractive workflow model overrides use `OY_OPENCODE_MODEL=provider/model#variant`.

## Install

### Full mise installer

```bash
curl -fsSL https://oy.adonm.dev/install.sh | sh
```

The POSIX shell installer installs pinned oy 0.13.4, `@opencode-ai/cli@0.0.0-next-15353`, `tokei`, and Universal Ctags. It verifies versions, stops stale OpenCode services, safely prunes unreferenced old mise versions, and runs `oy setup`. Setup backs up any previous oy-namespaced integration, registers the matching `@oy-cli/opencode@0.13.4` package, and reports a backup path when one is created. OpenCode installs the package into its isolated cache, and the installer waits up to 120 seconds to verify plugin ID `oy` loaded. Set `OY_SKIP_SETUP=1` to skip setup. It configures bash, zsh, or fish activation through mise's bootstrap support; restart your shell when it finishes.

Review [`install.sh`](install.sh) before piping it to a shell. Set `OY_SKIP_SETUP=1` to skip integration writes or `OY_MISE_MINIMUM_RELEASE_AGE` to change mise's release-age filter.

### Minimal manual install

```bash
mise use --global node@24 cargo-binstall cargo:oy-cli@0.13.4 npm:@opencode-ai/cli@0.0.0-next-15353
oy setup
oy doctor
```

You can also install from crates.io:

```bash
cargo install oy-cli --locked
```

## Choose setup scope

```bash
oy setup --dry-run        # preview global changes
oy setup                  # platform OpenCode config directory
oy setup --workspace      # .opencode/ in this repository
oy setup --remove         # remove global oy integration
```

Global setup is convenient for personal use and honors `OPENCODE_CONFIG_DIR`; on Linux its default is normally `~/.config/opencode/`. Workspace setup is useful when one repository needs local overrides. Existing `opencode.jsonc` wins over `opencode.json`. Setup and removal move old oy-namespaced files and snapshot changed configs under `oy/backups/` in the platform state location, falling back to the local-data directory when needed. Restart running OpenCode sessions after setup changes.

`oy setup` adds `@oy-cli/opencode@0.13.4` to OpenCode's `plugins` array after installing the `oy` binary. The package registers the agent, skills, and commands through the OpenCode V2 plugin API and does not define permissions.

> **Package-first setup:** the plugin supplies the permission-neutral `oy` agent, three skills, and `/oy-audit`, `/oy-review`, and `/oy-enhance`. Setup removes obsolete oy integration entries while preserving unrelated settings. JSON/JSONC is pretty-reserialized; its exact previous bytes remain in the reported backup.

Bare launch and workflow commands validate setup. In a terminal they offer to set it up when missing; automation exits with an explicit setup instruction. Rerun `oy setup` after changing versions so the package pin is refreshed.

## Create a first report

```bash
cd your-repository
oy doctor
oy doctor --check
oy audit
```

`oy doctor --check` validates the effective OpenCode service version, `oy` agent, commands, skills, and available models/providers/plugins. It validates discovery, not the user's permission choices.

oy creates a managed OpenCode session with the single `oy` agent, which loads the canonical audit skill under your effective OpenCode permissions and writes `ISSUES.md`. Start with a small or medium repository so you can inspect the protocol and report before increasing scope.

For a code-quality review:

```bash
oy review             # collected workspace
oy review main        # git diff main
```

Continue with the [workflow guide](workflows.md) to understand focus text, path scope, SARIF, finding IDs, and reruns.

## Compatibility

Prebuilt releases cover Linux x86_64/aarch64 with glibc and Apple Silicon macOS. Other Linux and macOS architectures may build from source but are not release-tested. Native Windows and other operating systems are rejected at build time; use WSL2 on Windows. The curl installer assumes a POSIX shell.

See the [compatibility matrix](compatibility.md) for the distinction between CI-tested, release-built, and best-effort environments.

oy defaults to `opencode2`. `OY_OPENCODE` can point to another executable that reports a supported OpenCode 2 version. `oy run`, `audit`, `review`, and `enhance` use the single `oy` agent; `oy run --auto` enables OpenCode's one-time automatic approvals while explicit denies remain effective. Bare `oy` launches the TUI, where you can select `oy` or OpenCode's built-in agents directly. Use `opencode2` for native host commands and options.

`oy doctor --install-missing` installs the pinned OpenCode beta, `tokei`, and Universal Ctags through mise when any are missing. The agent uses the two helpers only for compact orientation on large scopes; native OpenCode reads remain authoritative.

## Next steps

- [Run audit, review, and remediation loops](workflows.md)
- [Understand explicit setup and owned config](reference.md#setup-ownership-and-backups)
- [Look up all commands and environment variables](reference.md)
- [Review report and CI examples](examples.md)
- [Read the security guidance](https://github.com/adonm/oy-cli/blob/main/SECURITY.md)
