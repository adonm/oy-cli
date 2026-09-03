# Contributing

Keep `oy` focused. Its product is the deterministic audit → review → remediate workflow shipped as portable agent skills. The user's agent owns models, permissions, and general tools; `oy` owns deterministic collection/report boundaries and skill installation.

Native development and builds are supported on Linux. Use WSL2 rather than native Windows or macOS.

## Quick Start

```bash
mise install
just check
just run -- --help
```

If you do not use [`mise`](https://mise.jdx.dev/), install Rust 1.98+ and [`just`](https://github.com/casey/just) yourself.

To try the skills in an agent, run `oy setup` (writes them under
`~/.agents/skills/`) or `oy setup --workspace` for the checkout's
`.agents/skills/`, then ask your agent to run the `oy-setup` skill.

## Local Checks

Run these before opening a PR:

```bash
just check
# Optional extended suite; requires cargo-nextest and nightly Miri.
just ci
```

`just check` covers formatting, clippy, Rust tests/docs, CLI help, the installer, shellcheck, the mdBook site, and release-version alignment. `just ci` uses CI's nextest and Miri runners.

Keep `Cargo.lock` in sync with `Cargo.toml` after dependency changes.

## Development Flow

1. Inspect relevant code and docs first.
2. Make the smallest targeted change.
3. Add focused tests for behavior changes.
4. Run `just check`.
5. Update user-facing docs and `CHANGELOG.md` for behavior changes.

## Prompt And Skill Changes

Prompt quality is live-model behavior, not a deterministic unit-test problem.
Keep raw model outputs under `.tmp/eval/` when manually checking prompt changes
against a few pinned public repositories; do not commit generated `ISSUES.md`,
`REVIEW.md`, or SARIF files from local runs.

The canonical skill files live in `assets/skills/` and are embedded into the
binary at compile time; `oy setup` writes them and `oy doctor --check`
verifies byte-exact content. Keep the bodies host-neutral: they run in
OpenCode, Cursor, Codex, Copilot, and Gemini CLI alike.

## Design Rules

- Do not add a native LLM client, provider router, transcript store, or chat UI back to `oy`.
- Keep the three workflow skills aligned on the canonical audit, review, and enhance protocols, and keep the `oy-setup` skill aligned with `oy setup` behavior.
- Do not add oy-owned plan/edit/auto permission modes. The user's agent policy is authoritative.
- Put immutable workflow-input, ordering, limit, and render enforcement in typed Rust boundaries rather than relying on prompt text.
- Describe model-backed outcomes as nondeterministic even when their inputs and report rendering are deterministic.
- Do not duplicate built-in tools such as edit, bash, webfetch, repo clone, todo, task, grep, or glob.
- Validate workspace paths near every read/write boundary.
- Keep host coupling narrow: `src/skills/opencode_host.rs` and `opencode_api.rs` exist only for the optional post-setup OpenCode location refresh.
- Refuse to overwrite non-generated user files during setup.

## Important Paths

| Path | Role |
|---|---|
| `src/skills.rs` | Canonical skill assets and asset contract tests |
| `src/skills/setup.rs` | Skill installation/removal, legacy migration, locking, plugin-cache cleanup |
| `src/skills/setup/backup.rs` | Persistent setup backups and move/restore mechanics |
| `src/skills/setup/legacy_config.rs` | OpenCode JSON/JSONC parsing and stripping of legacy oy entries |
| `src/skills/opencode_host.rs`, `src/skills/opencode_api.rs` | Optional post-setup OpenCode location refresh |
| `src/workflow.rs` | Run-ID generation for prepared artifacts |
| `src/artifacts.rs` | Canonical file-backed prepare/finalize protocol and private run state |
| `src/audit/input.rs` | Repo file collection, manifest, security index, chunking, git diff input |
| `src/audit/findings.rs` | Finding extraction and structured findings blocks |
| `src/audit/sarif.rs` | SARIF rendering |
| `src/tools/external.rs` | Shared bounded-process boundary |
| `src/cli/config/paths.rs` | Workspace output path safety |
| `src/cli/config/atomic_write.rs` | Staged file batches and live rollback |
| `assets/skills/` | Canonical skill files embedded into the binary |
| `docs/install.sh`, `scripts/test_install.sh` | Curl installer and its shell smoke test |
| `.github/workflows/ci.yml` | CI checks |
| `justfile` | Local dev task runner |

See also:

- `docs/architecture.md` for runtime flow and ownership boundaries
- `SECURITY.md` for user-facing security guidance
- `ROADMAP.md` for current project priorities

## Release Notes

Update `CHANGELOG.md` for user-visible behavior changes. Before tagging, update release pins and run `python3 scripts/check_versions.py`; it checks Cargo and the installer/example version pins. Keep historical release notes factual and current docs focused on the file-backed architecture.
