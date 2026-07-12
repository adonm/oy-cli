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

## OpenCode host compatibility

oy 0.13.0 integrates with OpenCode 2 through its noninteractive runner, managed model API, native JSON/Markdown configuration, and local stdio MCP. It defaults to the `opencode2` executable; `OY_OPENCODE` can override the executable but not the required contract.

| Host line | Status |
|---|---|
| OpenCode beta `0.0.0-next-15353` | Supported pinned beta; installed as `@opencode-ai/cli@0.0.0-next-15353` and exposed as `opencode2`. |
| Other OpenCode beta builds | Unsupported until pinned and exercised by the compatibility smoke. |
| Tagged OpenCode 2.x | Accepted by the tagged-major contract; test coverage will be added when a tagged release exists. |
| OpenCode major >2 | Unsupported until its contract is reviewed. |
| OpenCode 1 | Unsupported; removed in oy 0.12.0-beta.1. |
| Unknown contract | Unsupported. |

The installer uses mise's npm backend for the exact pinned beta package. There is not yet an automated pinned cross-version API smoke matrix or provider-backed integration suite; the [roadmap](https://github.com/adonm/oy-cli/blob/main/ROADMAP.md) keeps those limits explicit.

Setup writes one permission-neutral `oy` agent, native `commands`, `mcp.servers`, timeout objects, and three canonical workflow skills. It removes older generated mode/workflow agents. Existing legacy command and MCP entries are migrated automatically; ambiguous legacy fields fail closed with manual-migration guidance. Global setup honors `OPENCODE_CONFIG_DIR` and selects an existing `opencode.jsonc` before `opencode.json`.

`oy doctor --check` validates the effective selected service version, `oy` agent, commands, skills, connected MCP entry, and model/provider/plugin availability. It deliberately does not validate or prescribe the user's permission policy.

## Optional evidence helpers

| Helper | Discovery requirement | Failure behavior |
|---|---|---|
| `tokei` | Successful capability probe at a canonical absolute path | `sloc` is omitted from MCP tools. |
| Universal Ctags | Successful JSON-capability probe at a canonical absolute path | `outline` is omitted from MCP tools. |
| Sighthound at commit `c4608eb2b6ca256daf4dbd1e74aadc3570343685` | Successful capability probe at a canonical absolute path; source-built with Rust 1.96, `--locked`, and only `bin=sighthound` | `sighthound` is omitted; complete chunk audit still runs. |

Relative `PATH` entries are ignored. Use `OY_TOKEI`, `OY_CTAGS`, or `OY_SIGHTHOUND` with an absolute path to override discovery.

## Reporting compatibility problems

Include:

- `oy --version`;
- the selected OpenCode executable and `--version` output;
- operating system and architecture;
- install method;
- `oy doctor --json` output with sensitive paths reviewed;
- whether setup is global or workspace-local.
