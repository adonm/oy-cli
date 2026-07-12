# Contributing

Keep `oy` focused. Its product is one concise autonomous OpenCode agent plus the audit → review → remediate loop. OpenCode owns models, permissions, and general tools; `oy` owns deterministic collection/report boundaries. Setup and launcher/API wrappers stay narrow.

Native development and builds are supported on Linux and macOS. Use WSL2 rather than native Windows.

## Quick Start

```bash
mise install
just check
just run -- --help
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
Before changing packaged agents or skills, read `docs/evaluation.md` and use a
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
- Keep the three packaged skills canonical for audit, review, and enhance protocols. They execute under the user's OpenCode permissions through the single `oy` agent.
- Keep `oy` concise but compare it with tagged OpenCode 2 Build behavior: inspect first, preserve unrelated changes, implement end-to-end, verify, and keep checkpoint commits focused without rewriting or publishing history.
- Do not add oy-owned plan/edit/auto permission modes. OpenCode policy is authoritative.
- Put immutable workflow-input, ordering, limit, and render enforcement in typed Rust boundaries rather than relying on prompt text.
- Describe model-backed outcomes as nondeterministic even when their inputs and report rendering are deterministic.
- Do not duplicate built-in tools such as edit, bash, webfetch, repo clone, todo, task, grep, or glob.
- Validate workspace paths near every read/write boundary.
- Keep generated global and workspace config files schema-valid against `https://opencode.ai/config.json`.
- Refuse to overwrite non-generated user files during setup.

## Important Paths

| Path | Role |
|---|---|
| `src/opencode.rs` | Thin OpenCode integration facade and package-asset contract tests |
| `src/opencode/setup.rs` | Setup, namespace migration, backup/rollback, config ownership, and prompting |
| `src/opencode/runner.rs` | Bare launch, task/workflow execution, and recovery |
| `src/opencode/host.rs`, `src/opencode/api.rs` | Root-bound OpenCode contract and managed-API adapters |
| `src/workflow.rs` | Typed workflow context, resolved scope, and recovery lease |
| `src/artifacts.rs` | Canonical file-backed prepare/finalize protocol and private run state |
| `src/audit/input.rs` | Repo file collection, manifest, security index, chunking, git diff input |
| `src/audit/findings.rs` | Finding extraction and structured findings blocks |
| `src/audit/sarif.rs` | SARIF rendering |
| `src/tools/external.rs` | Shared bounded-process boundary |
| `src/cli/config/paths.rs` | Workspace output path safety |
| `src/cli/config/atomic_write.rs` | Staged file batches and live rollback |
| `.github/workflows/ci.yml` | CI checks |
| `justfile` | Local dev task runner |

See also:

- `docs/architecture.md` for runtime flow and ownership boundaries
- `docs/evaluation.md` for prompt/agent evaluation on public OSS corpora
- `SECURITY.md` for user-facing security guidance
- `ROADMAP.md` for current project priorities

## Release Notes

Update `CHANGELOG.md` for user-visible behavior changes. Keep historical release notes factual and current docs focused on the file-backed architecture.
