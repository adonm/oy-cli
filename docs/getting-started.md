# Getting started

Set up oy as a focused audit/review extension for OpenCode 2.

## Requirements

- OpenCode beta `0.0.0-next-15353`, or a tagged OpenCode 2.x release
- `git` for target-diff reviews
- Rust 1.96+ only when building from source

oy does not store provider credentials. Follow OpenCode's [provider setup](https://v2.opencode.ai/providers), then use `opencode2` once to verify the model works. Noninteractive workflow model overrides use `OY_OPENCODE_MODEL=provider/model#variant`.

## Install

### Full mise installer

```bash
curl -fsSL https://oy.adonm.dev/install.sh | sh
```

The POSIX shell installer installs pinned oy 0.13.0, `@opencode-ai/cli@0.0.0-next-15353`, `tokei`, and Universal Ctags. It verifies versions, stops stale OpenCode services, safely prunes unreferenced old mise versions, removes generated integration from older oy releases, and runs a fresh `oy setup`. Set `OY_RESET_SETUP=0` to update generated setup in place or `OY_SKIP_SETUP=1` to skip setup. It configures bash, zsh, or fish activation through mise's bootstrap support; restart your shell when it finishes.

Review [`install.sh`](install.sh) before piping it to a shell. Set `OY_SKIP_SETUP=1` to skip integration writes or `OY_MISE_MINIMUM_RELEASE_AGE` to change mise's release-age filter. Set `OY_INSTALL_SIGHTHOUND=1` for the optional pinned source build; the installer provisions Rust 1.96, uses `--locked`, and builds only `bin=sighthound`.

### Minimal manual install

```bash
mise use --global node@24 cargo-binstall cargo:oy-cli@0.13.0 npm:@opencode-ai/cli@0.0.0-next-15353
oy setup
oy doctor
```

You can also install from crates.io:

```bash
cargo install oy-cli --locked
```

Optional evidence helpers can be added later from the [reference](reference.md#optional-helpers).

## Choose setup scope

```bash
oy setup --dry-run        # preview global changes
oy setup                  # ~/.config/opencode/
oy setup --workspace      # .opencode/ in this repository
oy setup --remove         # remove global oy integration
```

Global setup is convenient for personal use and honors `OPENCODE_CONFIG_DIR`. Workspace setup is useful when one repository needs local overrides. Existing `opencode.jsonc` wins over `opencode.json`. Setup/removal commits one rollback-capable batch, but does not maintain a persistent crash-recovery journal. Restart running OpenCode sessions after setup changes.

As a package-first alternative, add `@oy-cli/opencode@0.13.0` to OpenCode's `plugins` array after installing the `oy` binary. The package registers the same agent, skills, and commands through the OpenCode V2 plugin API and does not define permissions.

> **Native v2 setup:** setup writes one permission-neutral `oy` agent, three skills, and thin `commands`. It removes exact old oy MCP/output-budget entries but does not register MCP. JSON/JSONC is still pretty-reserialized, and `--remove` removes owned current values rather than restoring pre-setup values. Back up hand-edited config and preview first.

Normal launch, model, and workflow commands validate setup and never auto-refresh it. Rerun `oy setup` explicitly after changing versions or generated integration files.

## Create a first report

```bash
cd your-repository
oy doctor
oy doctor --check
oy audit
```

`oy doctor --check` validates the effective OpenCode service version, `oy` agent, commands, skills, and available models/providers/plugins. It validates discovery, not the user's permission choices.

oy starts OpenCode 2's noninteractive runner with the single `oy` agent, which loads the canonical audit skill under your effective OpenCode permissions and writes `ISSUES.md`. Start with a small or medium repository so you can inspect the protocol and report before increasing scope.

For a code-quality review:

```bash
oy review             # collected workspace
oy review main        # git diff main
```

Continue with the [workflow guide](workflows.md) to understand focus text, path scope, SARIF, finding IDs, and reruns.

## Compatibility

Prebuilt releases cover Linux x86_64/aarch64 with glibc and Apple Silicon macOS. Other Rust targets may build from source but are not release-tested. The curl installer assumes a Unix-like shell.

See the [compatibility matrix](compatibility.md) for the distinction between CI-tested, release-built, and best-effort environments.

oy defaults to `opencode2`. `OY_OPENCODE` can point to another executable that reports a supported OpenCode 2 version. `oy run`, `audit`, `review`, and `enhance` use the single `oy` agent; `oy run --auto` enables OpenCode's one-time automatic approvals while explicit denies remain effective. `oy`, `open`, and `chat` launch the TUI, where you can select `oy` or OpenCode's built-in agents directly.

Optional Sighthound can be added later with `oy doctor --install-sighthound`. Routine `oy doctor --install-missing` installs OpenCode, `tokei`, and Ctags but intentionally does not start the source build.

## Next steps

- [Run audit, review, and remediation loops](workflows.md)
- [Understand explicit setup and owned config](reference.md#setup-ownership)
- [Inspect the deterministic MCP boundary](reference.md#mcp-tools)
- [Review report and CI examples](examples.md)
- [Read the security guidance](https://github.com/adonm/oy-cli/blob/main/SECURITY.md)
