# Contributing

Keep `oy` focused. Its product is one concise OpenCode coding-agent behavior plus the audit → review → remediate loop. OpenCode owns models, permissions, and general tools; `oy` owns deterministic collection/report boundaries. Cursor models are a provider inside OpenCode, not a second host integration.

Native development and builds are supported on Linux and macOS. Use WSL2 rather than native Windows.

## Quick Start

```bash
mise install
just opencode-dev
just check
just run -- --help
```

If you do not use [`mise`](https://mise.jdx.dev/), install Rust 1.96+ and [`just`](https://github.com/casey/just) yourself.

`just opencode-dev` launches a private OpenCode instance using the checkout's
`packages/opencode/index.js`. It keeps config, cache, state, and service data
under an ignored `.tmp/opencode-dev.*` directory. Pass OpenCode arguments
directly, for example `just opencode-dev models`. If `opencode2` is missing
from `node@latest`, the recipe installs `@opencode-ai/cli@next` there.

## Local Checks

Run these before opening a PR:

```bash
just check
# Optional extended suite; requires cargo-nextest and nightly Miri.
just ci
```

`just check` covers formatting, clippy, Rust tests/docs, CLI help, the installer, the mdBook site, release-version alignment, and the OpenCode npm package. `just ci` uses CI's nextest and Miri runners.

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
- Keep the three OpenCode skills aligned on the canonical audit, review, and enhance protocols. Preserve and document the separate Cursor tool boundary when changing `cursor/*` support.
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
| `src/opencode/setup.rs` | Setup orchestration, namespace migration, locking, and prompting |
| `src/opencode/setup/backup.rs` | Persistent setup backups and move/restore mechanics |
| `src/opencode/setup/config_file.rs` | OpenCode JSON/JSONC parsing and oy-owned config transformations |
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

Update `CHANGELOG.md` for user-visible behavior changes. Before tagging, update release pins and run `python3 scripts/check_versions.py`; it checks Cargo, npm metadata, the installer, and versioned examples. Keep historical release notes factual and current docs focused on the file-backed architecture.
