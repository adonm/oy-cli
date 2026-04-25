# Contributing

Thanks for contributing to `oy`.

## Development loop

```bash
cargo fmt
cargo check
cargo test
cargo run -- --help
```

## Repo layout

- `src/main.rs` — process entrypoint
- `src/cli.rs` — command entrypoints and chat UX
- `src/agent.rs` — transcript handling and `genai` tool loop
- `src/config.rs` — config, model persistence, session persistence
- `src/model.rs` — model-id normalization and `genai` client setup
- `src/tools.rs` — model-exposed tools plus local file/search/web helpers
- `tests/` — legacy Python reference tests kept during the migration
- `oy_cli/` — legacy Python implementation kept as porting reference

## Working rules

- package / command: `oy` / `oy`
- prefer native `genai` model ids in docs and examples
- keep the implementation small, direct, and easy to audit
- prefer env-first configuration so common usage stays close to `oy "prompt"`
- when docs, tests, and behavior disagree, fix them together
- prefer simple changes over abstraction-heavy rewrites
- keep security guidance OWASP-minded and performance guidance measurement-first

## Style

- optimize for readability at the call site
- prefer short obvious names in local context
- prefer flat control flow and early returns
- avoid clever tricks and hidden mutation
- keep file edits small and maintainable

## Change checklist

- keep `README.md` user-focused and task-oriented
- keep contributor workflow here in `CONTRIBUTING.md`
- keep `/ask` wording explicit: no-write, but public `webfetch` is still allowed
- add or extend focused Rust tests next to changed behavior
- run targeted checks before broader checks when iterating

## Release process

1. Run pre-flight checks:

   ```bash
   cargo fmt --check
   cargo check
   cargo test
   cargo run -- --help
   ```

2. Bump the crate version in `Cargo.toml`.
3. Commit, tag, push, and create the GitHub release.
4. Install the built binary and verify `oy --help`.
