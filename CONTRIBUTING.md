# Contributing

Keep `oy` small, boring, and useful.

## Local checks

Run these before opening a PR:

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked
cargo run --locked -- --help
```

For release-adjacent changes, also run:

```bash
cargo package --locked
```

## Development flow

1. Inspect the relevant code and docs first.
2. Make the smallest targeted change.
3. Add or update focused tests for behavior changes.
4. Run the local checks above.
5. Update `README.md`, `SECURITY.md`, docs, help text, and `CHANGELOG.md` for user-visible behavior changes.

## Design rules

- Prefer fewer concepts and explicit data flow.
- Default to safe interactive behavior; make higher-risk modes explicit.
- Inspect before edits; verify after edits.
- Keep public docs aligned with implemented CLI help.
- For security-sensitive work, name the trust boundary, validate near it, fail closed, and add focused tests.
- Do not add providers, persistence, process, file, network, or credential handling without a concrete user need and focused tests.

## Important paths

| Path | Role |
|---|---|
| `src/agent.rs` | Provider integration, model routing, Bedrock support, sessions, transcripts, context compaction, tool loop |
| `src/audit.rs` | Deterministic no-tools audit runner, file collection, chunking, prompts, report post-processing |
| `src/cli.rs` | Config paths, safety modes, terminal UI, chat shell, command handlers |
| `src/tools.rs` | Tool schemas, tool dispatch, previews, todos, filesystem/network/mutation approval boundaries |
| `src/lib.rs`, `src/main.rs` | Public facade and binary entry point |
| `tests/snapshots.rs` | Snapshot coverage for chat help and tool previews |
| `.github/workflows/release.yml` | CI, release, artifact attestation, and crate publishing |

See also:

- `docs/architecture.md` for runtime flow and module responsibilities.
- `docs/tool-safety.md` for tool capability and boundary guidance.
- `SECURITY.md` for user-facing security guidance.
- `ISSUES.md` for the latest audit findings and remediation history.

## Release notes

Update `CHANGELOG.md` for user-visible behavior changes. Keep fixed security findings in `ISSUES.md` with **Status: Fixed** until the next full audit refresh.
