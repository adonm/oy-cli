# Getting started

Set up oy as a focused audit/review extension for an existing opencode installation.

## Requirements

- opencode and a configured model provider
- `git` for target-diff reviews
- Rust 1.96+ only when building from source

oy does not store provider credentials or select a provider. Follow opencode's [provider setup](https://opencode.ai/docs/providers/), then use `opencode` once to verify the model works.

## Install

### Full mise installer

```bash
curl -fsSL https://adonm.github.io/oy-cli/install.sh | sh
```

The POSIX shell installer installs or updates mise, oy, opencode, `tokei`, and Universal Ctags, then runs `oy setup`. Restart your shell or use the activation command it prints.

Review [`install.sh`](install.sh) before piping it to a shell. Set `OY_SKIP_SETUP=1` to skip integration writes or `OY_MISE_MINIMUM_RELEASE_AGE` to change mise's release-age filter. Optional Sighthound has no release binary; set `OY_INSTALL_SIGHTHOUND=1` only when Rust 1.85+ is already installed and you want mise to build it from source.

### Minimal manual install

```bash
mise use --global cargo-binstall cargo:oy-cli opencode
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
```

Global setup is convenient for personal use. Workspace setup is useful when one repository needs local overrides. Restart running opencode sessions after setup changes.

> **Config rewrite:** oy replaces its documented config entries and pretty-serializes `opencode.json`. Other object keys remain, but JSONC comments and formatting do not. Back up a hand-edited config and preview first.

## Create a first report

```bash
cd your-repository
oy doctor
oy audit
```

opencode runs the restricted auditor and writes `ISSUES.md`. Start with a small or medium repository so you can inspect the protocol and report before increasing scope.

For a code-quality review:

```bash
oy review             # collected workspace
oy review main        # git diff main
```

Continue with the [workflow guide](workflows.md) to understand focus text, path scope, SARIF, finding IDs, and reruns.

## Compatibility

Prebuilt releases cover Linux x86_64/aarch64 with glibc and Apple Silicon macOS. Other Rust targets may build from source but are not release-tested. The curl installer assumes a Unix-like shell.

See the [compatibility matrix](compatibility.md) for the distinction between CI-tested, release-built, and best-effort environments.

## Next steps

- [Run audit, review, and remediation loops](workflows.md)
- [Understand automatic refresh and owned config](reference.md#setup-ownership-and-refresh)
- [Inspect the deterministic MCP boundary](reference.md#mcp-tools)
- [Review report and CI examples](examples.md)
- [Read the security guidance](https://github.com/adonm/oy-cli/blob/main/SECURITY.md)
