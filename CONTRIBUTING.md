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

- `src/main.rs` — process entrypoint plus terminal highlighting print macros
- `src/cli.rs` — command parsing/orchestration for run, chat, model, audit, Ralph, and Renovate-local flows
- `src/agent.rs` — session state, transcript serialization, token estimates, and the `genai` tool loop
- `src/config.rs` — config paths, env flags, agent profiles, prompt loading, model/shim persistence, and saved sessions
- `src/model.rs` — model-id normalization, routing shim resolution, `genai` client setup, and endpoint model introspection
- `src/tools.rs` — model-exposed tools for list/read/search/replace/sloc/bash/webfetch/ask/todo with workspace and approval guardrails
- `src/ui.rs` — reedline chat loop, slash commands, prompts, and interactive model picker
- `src/highlight.rs` — terminal syntax highlighting via `syntect`
- `assets/session_text.toml` — system prompts, agent text, audit text, and tool descriptions
- `legacy-python/` — archived Python implementation, packaging, lockfile, and reference tests

## Crate map

- CLI/runtime: `clap`, `tokio`, `anyhow`
- model/tool loop: `genai`, `toon-format`, `tiktoken-rs`
- config/persistence/prompts: `serde`, `serde_json`, `toml`, `dirs`, `chrono`
- search/file tools: `ignore`, `glob`, `globset`, `grep-regex`, `grep-searcher`, `regex`, `tokei`
- network/tools: `reqwest` with rustls/http2, `url`, `html2md`
- archives/compression: `flate2`, `tar`, `zip`
- terminal UX: `reedline-repl-rs`, `syntect`

Prefer adding notes here when a new crate establishes a new subsystem boundary. Keep README crate notes user-facing and this map contributor-facing.

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
