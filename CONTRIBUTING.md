# Contributing

Keep `oy` small, boring, and useful.

## Local checks

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run -- --help
```

## Design rules

- Prefer fewer files, fewer concepts, and clear names.
- Default to safe interactive behavior; make higher-risk modes explicit.
- Inspect before edits; verify after edits.
- Keep public docs aligned with implemented behavior.
- Do not add providers, persistence, process, file, network, or credential handling without a concrete user need and focused tests.

## Important paths

- `src/app.rs` — CLI commands and user-facing flow.
- `src/chat.rs` — interactive chat commands/history.
- `src/config.rs` — config, prompts, saved session/model metadata, modes.
- `src/model.rs` — model selection and OpenAI-compatible routing.
- `src/session.rs` — transcript/session state and context compaction.
- `src/tools.rs` — workspace tools and tool schemas.
- `src/ui.rs` — terminal output helpers.

## Release notes

Update `CHANGELOG.md` for user-visible behavior changes.
