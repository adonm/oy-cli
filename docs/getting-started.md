# Getting started

Set up oy as a focused audit/review extension for OpenCode 2.

## Requirements

- OpenCode beta `0.0.0-next-15323`, or a tagged OpenCode 2.x release
- `git` for target-diff reviews
- Rust 1.96+ only when building from source

oy does not store provider credentials. Follow OpenCode's [provider setup](https://opencode.ai/docs/providers/), then use `opencode2` once to verify the model works. Noninteractive workflow model overrides use `OY_OPENCODE_MODEL=provider/model#variant`.

## Install

### Full mise installer

```bash
curl -fsSL https://oy.adonm.dev/install.sh | sh
```

The POSIX shell installer installs or updates mise, oy, `@opencode-ai/cli@0.0.0-next-15323` through mise's npm backend, `tokei`, and Universal Ctags, then runs `oy setup`. It configures bash, zsh, or fish activation through mise's bootstrap support; restart your shell when it finishes.

Review [`install.sh`](install.sh) before piping it to a shell. Set `OY_SKIP_SETUP=1` to skip integration writes or `OY_MISE_MINIMUM_RELEASE_AGE` to change mise's release-age filter. Set `OY_INSTALL_SIGHTHOUND=1` for the optional pinned source build; the installer provisions Rust 1.96, uses `--locked`, and builds only `bin=sighthound`.

### Minimal manual install

```bash
mise use --global node@24 cargo-binstall cargo:oy-cli npm:@opencode-ai/cli@0.0.0-next-15323
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

> **Native v2 config migration:** setup writes `commands`, `mcp.servers`, timeout objects, ordered agent permissions, and three workflow skills. It safely migrates legacy command/MCP entries, but fails closed on ambiguous legacy permission/provider/plugin and related fields. JSON/JSONC is still pretty-reserialized, and `--remove` removes owned current values rather than restoring pre-setup values. Back up hand-edited config and preview first.

Normal launch, model, and workflow commands validate setup and never auto-refresh it. Rerun `oy setup` explicitly after changing versions or generated integration files.

## Create a first report

```bash
cd your-repository
oy doctor
oy doctor --check
oy audit
```

`oy doctor --check` validates the effective OpenCode service version, required agents/commands, connected oy MCP server, and available models/providers/plugins. It uses the selected runtime as configured; it does not claim an isolated server check.

oy starts OpenCode 2's noninteractive runner with the restricted auditor, which loads the canonical audit skill and writes `ISSUES.md`. Start with a small or medium repository so you can inspect the protocol and report before increasing scope.

For a code-quality review:

```bash
oy review             # collected workspace
oy review main        # git diff main
```

Continue with the [workflow guide](workflows.md) to understand focus text, path scope, SARIF, finding IDs, and reruns.

## Compatibility

Prebuilt releases cover Linux x86_64/aarch64 with glibc and Apple Silicon macOS. Other Rust targets may build from source but are not release-tested. The curl installer assumes a Unix-like shell.

See the [compatibility matrix](compatibility.md) for the distinction between CI-tested, release-built, and best-effort environments.

oy defaults to `opencode2`. `OY_OPENCODE` can point to another executable that reports a supported OpenCode 2 version. `oy run`, `audit`, `review`, and `enhance` use the noninteractive runner; `model` uses the managed model API; `oy`, `open`, and `chat` launch the TUI. TUI session continuation/resume is supported, but agent and mode selection remain in the TUI; use `oy run` for mode selection.

Optional Sighthound can be added later with `oy doctor --install-sighthound`. Routine `oy doctor --install-missing` installs OpenCode, `tokei`, and Ctags but intentionally does not start the source build.

## Next steps

- [Run audit, review, and remediation loops](workflows.md)
- [Understand explicit setup and owned config](reference.md#setup-ownership)
- [Inspect the deterministic MCP boundary](reference.md#mcp-tools)
- [Review report and CI examples](examples.md)
- [Read the security guidance](https://github.com/adonm/oy-cli/blob/main/SECURITY.md)
