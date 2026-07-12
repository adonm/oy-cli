# Compatibility

## Platforms

| Environment | Status |
|---|---|
| Linux x86_64 with glibc | Full CI and release archive |
| Linux aarch64 with glibc | Release archive; full suite is not run on this target |
| macOS Apple Silicon | Release archive; full suite is not run on this target |
| Other Linux/macOS targets | Source build only; not release-tested |
| Windows | Unsupported natively; use WSL2 |
| Other operating systems | Unsupported; rejected at build time |

The curl installer requires a POSIX shell. Rust 1.96 is the minimum source-build toolchain. The npm package declares only Linux and macOS.

## OpenCode 2

oy 0.13.4 defaults to the `opencode2` executable and accepts:

| Host | Status |
|---|---|
| `0.0.0-next-15353` | Supported pinned beta and installer default |
| Other beta/prerelease builds | Unsupported until pinned and tested |
| Tagged OpenCode 2.x | Accepted by the major-version contract |
| OpenCode 1, major >2, or unknown versions | Unsupported |

`OY_OPENCODE` can select another executable, but the selected executable must pass the same contract. The installer uses mise's npm backend for the exact beta. CI includes a pinned OpenCode integration smoke, but there is no provider-backed test suite or broad cross-version matrix.

The runtime integration uses:

- the version-matched `@oy-cli/opencode` package for one agent, three skills, and three slash commands;
- OpenCode's managed API for outer audit/review/enhance sessions;
- OpenCode `run` for `oy run`, `mini` for interactive enhancement, and the TUI for bare `oy`;
- native OpenCode file tools plus `oy audit|review prepare/finalize` for reports.

`oy doctor --check` validates the effective service version, API, location, agent, commands, skills, models, providers, and plugin. It does not validate the user's permission choices.

## Setup compatibility

Global setup uses `OPENCODE_CONFIG_DIR` when set, otherwise the platform config directory's `opencode` child. Workspace setup uses `OY_ROOT/.opencode/`. An existing `opencode.jsonc` is selected before `opencode.json`.

Setup pins the package version matching the binary. It moves old direct `oy`, `oy-*`, and `oy.*` agent/command/skill entries to a reported backup and removes obsolete oy config entries. Unrelated configuration is retained.

## Optional context helpers

The installer and `oy doctor --install-missing` provide `tokei` and Universal Ctags. They are optional direct shell tools used by the agent for compact repository inventory and scoped symbol outlines. Missing helpers do not block setup, audits, reviews, or remediation.

## Reporting a compatibility problem

Include:

- `oy --version`;
- selected OpenCode executable and `--version` output;
- operating system and architecture;
- install method;
- reviewed/redacted `oy doctor --json` output;
- whether setup is global or workspace-local.
