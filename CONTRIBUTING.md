# Contributing

Keep `oy` small. OpenCode owns AI behavior; `oy` should remain a setup wrapper plus deterministic MCP helpers.

## Quick Start

```bash
mise install
just check
just run -- --help
just run -- mcp
```

If you do not use [`mise`](https://mise.jdx.dev/), install Rust 1.96+ and [`just`](https://github.com/casey/just) yourself.

## Local Checks

Run these before opening a PR:

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
cargo test --doc --locked
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked
cargo run --locked -- --help
```

`just check` runs the standard local suite. `just ci` adds nextest and Miri parity checks when those tools are installed.

Keep `Cargo.lock` in sync with `Cargo.toml` after dependency changes.

## Development Flow

1. Inspect relevant code and docs first.
2. Make the smallest targeted change.
3. Add focused tests for behavior changes.
4. Run `just check`.
5. Update user-facing docs and `CHANGELOG.md` for behavior changes.

## Design Rules

- Do not add a native LLM client, provider router, transcript store, or chat UI back to `oy`.
- Prefer OpenCode config, agents, skills, commands, and permissions for orchestration.
- Keep MCP tools deterministic and narrow.
- Do not duplicate OpenCode built-in tools such as edit, bash, webfetch, repo clone, todo, task, grep, or glob.
- Validate workspace paths near every read/write boundary.
- Keep generated global and workspace OpenCode files schema-valid against `https://opencode.ai/config.json`.
- Refuse to overwrite non-generated user files during setup.

## Important Paths

| Path | Role |
|---|---|
| `src/opencode.rs` | OpenCode setup, generated config/agents/skills, launch wrappers |
| `src/mcp.rs` | Minimal stdio MCP JSON-RPC server |
| `src/audit/input.rs` | Repo file collection, manifest, security index, chunking, git diff input |
| `src/audit/findings.rs` | Finding extraction and structured findings blocks |
| `src/audit/sarif.rs` | SARIF rendering |
| `src/tools/outline.rs` | Tree-sitter outline helper |
| `src/tools/workspace/sloc.rs` | SLOC helper |
| `src/cli/config/paths.rs` | Workspace output path safety |
| `.github/workflows/ci.yml` | CI checks |
| `justfile` | Local dev task runner |

See also:

- `docs/architecture.md` for runtime flow and ownership boundaries
- `docs/tool-safety.md` for MCP tool boundaries
- `SECURITY.md` for user-facing security guidance
- `ISSUES.md` and `REVIEW.md` for current generated reports

## Release Notes

Update `CHANGELOG.md` for user-visible behavior changes. Keep historical release notes factual, but current docs should describe the OpenCode/MCP architecture.
