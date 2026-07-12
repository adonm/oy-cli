# Contributing

Keep `oy` focused. Its product is one concise autonomous OpenCode agent plus the audit → review → remediate loop. OpenCode owns models, permissions, and general tools; `oy` owns deterministic collection/report boundaries. Setup, MCP, and launcher/API compatibility are transitional surfaces.

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
mdbook build
cargo run --locked -- --help
```

`just check` runs the standard local suite, including the mdBook site build. `just ci` adds nextest and Miri parity checks when those tools are installed.

Keep `Cargo.lock` in sync with `Cargo.toml` after dependency changes.

## Development Flow

1. Inspect relevant code and docs first.
2. Make the smallest targeted change.
3. Add focused tests for behavior changes.
4. Run `just check`.
5. For generated prompt/agent changes, run or update the evaluation plan in `docs/evaluation.md`.
6. Update user-facing docs and `CHANGELOG.md` for behavior changes.

## Prompt And Agent Changes

Prompt quality is live-model behavior, not a deterministic unit-test problem.
Before changing generated agents or skills, read `docs/evaluation.md` and use a
pinned public-repository corpus when possible. Keep raw model outputs under
`.tmp/eval/`; do not commit generated `ISSUES.md`, `REVIEW.md`, or SARIF files
from local runs.

Useful commands:

```bash
just eval
python3 scripts/eval_runner.py run --dry-run
```

## Design Rules

- Do not add a native LLM client, provider router, transcript store, or chat UI back to `oy`.
- Keep the three generated skills canonical for audit, review, and enhance protocols. They execute under the user's OpenCode permissions through the single `oy` agent.
- Keep `oy` concise but compare it with tagged OpenCode 2 Build behavior: inspect first, preserve unrelated changes, implement end-to-end, verify, and avoid destructive or unrequested Git operations.
- Do not add oy-owned plan/edit/auto permission modes. OpenCode policy is authoritative.
- Put immutable workflow-input, ordering, limit, and render enforcement in typed Rust boundaries rather than relying on prompt text.
- Keep MCP tools deterministic and narrow while they remain; new core behavior should be reusable from the CLI and a future file-artifact workflow.
- Describe model-backed outcomes as nondeterministic even when their inputs and report rendering are deterministic.
- Do not duplicate built-in tools such as edit, bash, webfetch, repo clone, todo, task, grep, or glob.
- Validate workspace paths near every read/write boundary.
- Keep generated global and workspace config files schema-valid against `https://opencode.ai/config.json`.
- Refuse to overwrite non-generated user files during setup.

## Important Paths

| Path | Role |
|---|---|
| `src/opencode.rs` | Transitional setup/config plus the single agent, skills, and launch wrappers |
| `src/opencode/host.rs`, `src/opencode/api.rs` | Root-bound OpenCode contract and managed-API adapters |
| `src/workflow.rs` | Typed inherited workflow context and resolved scope |
| `src/mcp.rs` | Transitional stdio MCP adapter and current deterministic workflow entrypoint |
| `src/audit/input.rs` | Repo file collection, manifest, security index, chunking, git diff input |
| `src/audit/findings.rs` | Finding extraction and structured findings blocks |
| `src/audit/sarif.rs` | SARIF rendering |
| `src/tools/workspace/outline.rs` | Optional outline helper via Universal Ctags |
| `src/tools/workspace/sloc.rs` | SLOC helper |
| `src/tools/workspace/sighthound.rs` | Optional SAST helper via Sighthound |
| `src/tools/external.rs` | Shared optional-executable boundary |
| `src/cli/config/paths.rs` | Workspace output path safety |
| `src/cli/config/atomic_write.rs` | Staged file batches and live rollback |
| `.github/workflows/ci.yml` | CI checks |
| `justfile` | Local dev task runner |

See also:

- `docs/architecture.md` for runtime flow and ownership boundaries
- `docs/tool-safety.md` for MCP tool boundaries
- `docs/evaluation.md` for prompt/agent evaluation on public OSS corpora
- `SECURITY.md` for user-facing security guidance
- `ROADMAP.md` for current project priorities

## Release Notes

Update `CHANGELOG.md` for user-visible behavior changes. Keep historical release notes factual; current docs should distinguish the supported MCP implementation from the CLI-first target architecture.
