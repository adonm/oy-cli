# Contributing

Keep `oy` small, boring, and useful.

## Quick start

```bash
just check          # run all CI checks locally (fmt, clippy, test, rustdoc, smoke)
just fix            # auto-format and apply clippy suggestions, then check
just run -- chat    # run oy with arguments during development
```

If you don't have [`just`](https://github.com/casey/just), install it
(`cargo install just`, `brew install just`, etc.) or run the individual
commands listed below.

## Local checks

Run these before opening a PR. `just check` runs them all.

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked
cargo run --locked -- --help
```

Run clippy after formatting so its suggestions apply cleanly. Keep
`--all-targets --locked -- -D warnings` intact: it checks tests/examples as
well as the binary and treats every lint as something to fix before review.

For release-adjacent changes, also run:

```bash
just package          # or: cargo package --locked
```

### Pre-commit hook (optional but recommended)

```bash
git config core.hooksPath .githooks
```

This runs `cargo fmt --check` and `cargo clippy -- -D warnings` before each
commit so style/lint noise never reaches CI.

## Development flow

1. Inspect the relevant code and docs first.
2. Make the smallest targeted change.
3. Add or update focused tests for behavior changes.
4. Run `just check`.
5. Update `README.md`, `SECURITY.md`, docs, help text, and `CHANGELOG.md` for
   user-visible behavior changes.

## Finding something to work on

- **Upstream Rig TODO** — OpenAI-compatible reasoning/tool-call roundtrip:
  preserve empty `reasoning_content`, add an opt-in compatibility flag to emit
  `reasoning_content: ""` on assistant tool-call messages when a provider
  requires it, and cover the behavior with default-off and provider-compat tests.
  This should let Moonshot/Kimi-like models use thinking with tools without
  requiring `oy` to disable thinking by default.
- **`docs/GOOD_FIRST_ISSUES.md`** — limited-scope tasks suitable for new
  contributors.
- **`ISSUES.md`** — the latest audit findings; each finding lists the affected
  files and the trust boundary.
- **GitHub issues** — look for [`good-first-issue`][gfi] and
  [`help-wanted`][hw] labels.

[gfi]: https://github.com/wagov-dtt/oy-cli/labels/good-first-issue
[hw]: https://github.com/wagov-dtt/oy-cli/labels/help-wanted

## Design rules

- Prefer fewer concepts and explicit data flow.
- Default to safe interactive behavior; make higher-risk modes explicit.
- Inspect before edits; verify after edits.
- Keep public docs aligned with implemented CLI help.
- For security-sensitive work, name the trust boundary, validate near it, fail
  closed, and add focused tests.
- For `webfetch` changes, keep model use simple: public docs should work by
  default with redirects and non-credentialed document-friendly headers, while
  every request/redirect remains public-only and sensitive headers stay denied.
- Do not add providers, persistence, process, file, network, or credential
  handling without a concrete user need and focused tests.

## Important paths

| Path | Role |
|---|---|
| `src/agent.rs`, `src/agent/` | Provider integration, model routing, sessions, transcripts, context compaction, tool loop |
| `src/audit.rs` | Deterministic no-tools audit runner, file collection, chunking, prompts, report post-processing |
| `src/cli.rs`, `src/cli/` | Config paths, safety modes, terminal UI, chat shell, command handlers |
| `src/tools.rs` | Tool schemas, tool dispatch, previews, todos, filesystem/network/mutation approval boundaries |
| `src/lib.rs`, `src/main.rs` | Public facade and binary entry point |
| `tests/` | Integration and snapshot tests |
| `.github/workflows/ci.yml` | CI checks (fmt, clippy, test, rustdoc, help smoke, doc drift, package) |
| `.github/workflows/release.yml` | Release builds, artifact attestation, and crate publishing |
| `justfile` | Local dev task runner; `just check` / `just fix` / `just run` |

See also:

- `docs/architecture.md` for runtime flow and module responsibilities.
- `docs/tool-safety.md` for tool capability and boundary guidance.
- `SECURITY.md` for user-facing security guidance.
- `ISSUES.md` for the latest audit findings and remediation history.
- `docs/GOOD_FIRST_ISSUES.md` for newcomer-friendly tasks.

## Release notes

Update `CHANGELOG.md` for user-visible behavior changes. Keep fixed security
findings in `ISSUES.md` with **Status: Fixed** until the next full audit
refresh.
