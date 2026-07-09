# Compatibility

This matrix distinguishes what CI exercises from what release automation merely builds. It avoids implying support that the project does not currently test.

## oy release targets

| Environment | Status | Evidence |
|---|---|---|
| Linux x86_64, glibc | CI-tested and release-built | Full Rust checks run on `ubuntu-latest`; release archive is built. |
| Linux aarch64, glibc | Release-built | Release archive is built on Ubuntu ARM; full test suite is not run there. |
| macOS Apple Silicon | Release-built | Release archive is built on macOS 14; full test suite is not run there. |
| Windows | Source-level best effort | Windows-specific code compiles only when contributors/automation exercise it; no release archive is published. |
| Other Rust targets | Unsupported/best effort | May build from source; no CI or release guarantee. |

The curl installer requires a Unix-like POSIX shell. Rust 1.96 is the minimum source-build toolchain.

## opencode host compatibility

oy currently integrates through opencode CLI flags, generated Markdown agents/skills, JSON config, and local stdio MCP. The installer selects the current opencode release through mise with a configurable minimum-release-age filter.

| Host line | Status |
|---|---|
| Current stable opencode | Intended integration target; run `oy setup --dry-run`, `oy setup`, and `oy doctor` after upgrades. |
| Older opencode releases | Best effort; generated config and CLI flags may differ. |
| opencode prereleases/major transitions | Not supported until their integration contract is tagged and stable. |

There is not yet an automated cross-version opencode matrix. The [roadmap](https://github.com/adonm/oy-cli/blob/main/ROADMAP.md) keeps that limitation explicit rather than claiming a minimum host version without evidence.

## Optional evidence helpers

| Helper | Discovery requirement | Failure behavior |
|---|---|---|
| `tokei` | Successful capability probe at a canonical absolute path | `sloc` is omitted from MCP tools. |
| Universal Ctags | Successful JSON-capability probe at a canonical absolute path | `outline` is omitted from MCP tools. |
| Sighthound 1.0 | Successful capability probe at a canonical absolute path; source install requires Rust 1.85+ | `sighthound` is omitted; complete chunk audit still runs. |

Relative `PATH` entries are ignored. Use `OY_TOKEI`, `OY_CTAGS`, or `OY_SIGHTHOUND` with an absolute path to override discovery.

## Reporting compatibility problems

Include:

- `oy --version`;
- `opencode --version`;
- operating system and architecture;
- install method;
- `oy doctor --json` output with sensitive paths reviewed;
- whether setup is global or workspace-local.
