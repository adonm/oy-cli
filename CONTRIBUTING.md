# Contributing

Thanks for contributing to `oy`.

## Development loop

```bash
cargo fmt --check
cargo check
cargo test
cargo run -- --help
```

## Requirements

- Rust 1.85+ (`Cargo.toml` is the source of truth)
- `bash`
- provider credentials or a local OpenAI-compatible server for manual end-to-end checks

## Repo layout

- `src/main.rs` — process entrypoint
- `src/cli.rs` — command parsing/orchestration for run, chat, model, audit, and Ralph
- `src/chat.rs` — reedline chat loop, slash commands, prompts, and interactive model picker
- `src/agent.rs` — session state, transcript serialization, token estimates, and the `genai` tool loop
- `src/config.rs` — config paths, env flags, agent profiles, prompt loading, model/shim persistence, and saved sessions
- `src/model.rs` — model-id normalization, routing shim resolution, `genai` client setup, and endpoint model introspection
- `src/tools/mod.rs` — model-exposed tools for list/read/search/replace/sloc/bash/webfetch/ask/todo with workspace and approval guardrails
- `src/ui.rs` — terminal output, markdown, highlighting, diffs, and preview clamping helpers
- `assets/session_text.toml` — system prompts, agent text, audit text, and tool descriptions

Keep implementation architecture here, not in the README. README should stay user-first: install, quick start, commands, config, safety, troubleshooting.

## Crate map

- CLI/runtime: `clap`, `tokio`, `anyhow`
- model/tool loop: `genai`, `toon-format`, `tiktoken-rs`
- config/persistence/prompts: `serde`, `serde_json`, `toml`, `dirs`, `chrono`
- search/file tools: `ignore`, `glob`, `globset`, `grep-regex`, `grep-searcher`, `regex`, `tokei`
- network/tools: `reqwest` with rustls/http2, `url`, `html2md`
- terminal UX: `reedline-repl-rs`, `dialoguer`, `console`, `syntect`, `termimad`, `terminal_size`, `textwrap`, `unicode-width`

Prefer adding notes here when a new crate establishes a new subsystem boundary. Keep README crate notes user-facing and this map contributor-facing.

## Working rules

- package / command: `oy` / `oy`
- prefer native `genai` model ids in docs and examples
- keep the implementation small, direct, and easy to audit
- prefer env-first configuration so common usage stays close to `oy "prompt"`
- keep prompt text and tool descriptions in `assets/session_text.toml`
- when docs, tests, and behavior disagree, fix them together
- prefer simple changes over abstraction-heavy rewrites
- collapse repeated helper code when it makes nearby call sites shorter and clearer
- keep security guidance OWASP-minded and performance guidance measurement-first

## Style

- optimize for readability at the call site
- prefer short obvious names in local context
- prefer flat control flow and early returns
- keep the same concept named the same way across nearby modules
- prefer nouns for data, verbs for functions, and predicate names for booleans
- avoid clever tricks, hidden mutation, and framework-style indirection
- keep file edits small and maintainable

## Change checklist

- keep `README.md` user-focused and task-oriented
- keep contributor workflow here in `CONTRIBUTING.md`
- keep `/ask` wording explicit: no-write, but public `webfetch` is still allowed
- add or extend focused Rust tests next to changed behavior, usually in the same module under `#[cfg(test)]`
- avoid adding crates for tiny helpers; prefer small local functions unless a crate defines a real subsystem boundary
- do not add file, process, network, or credential capability without updating security notes
- run targeted checks before broader checks when iterating
- keep terminal output concise; clamp previews instead of dumping full command/tool output

## Documentation checklist

- New command? Update the README command table and common-task examples.
- New env var? Update the README configuration table.
- New model/auth flow? Update first model setup, authentication/config notes, and troubleshooting.
- New tool? Update `assets/session_text.toml`; if it mutates files, runs processes, touches network, or handles secrets, update README safety notes.
- UX behavior change? Update the relevant README section and slash-command help if applicable.
- New subsystem or crate? Update repo layout / crate map here, not the user-facing README.
- Terminology: use “model id”, “routing shim”, “workspace”, and “preview” in user docs; reserve code-specific names like `model_spec` for implementation docs.

## CI and release builds

`.github/workflows/release.yml` is the source of truth.

- Pushes to branch `rust` run CI on `ubuntu-latest`:
  - `cargo build --locked`
  - `cargo test --locked`
- Pushing a tag matching `v*` builds release assets for:
  - `x86_64-unknown-linux-gnu` on `ubuntu-latest`
  - `aarch64-unknown-linux-gnu` on `ubuntu-24.04-arm`
  - `aarch64-apple-darwin` on `macos-14`
- Each release asset is a `tar.gz` archive named `oy-<tag>-<target>.tar.gz` containing a single `oy` binary.
- Release assets are uploaded as workflow artifacts, provenance-attested, then published to a GitHub release.
- The workflow does not currently build Windows, Intel macOS, musl, or Debian/RPM/Homebrew packages.

## Release process

1. Run pre-flight checks:

   ```bash
   cargo fmt --check
   cargo check
   cargo test
   cargo run -- --help
   ```

2. Bump the crate version in `Cargo.toml`.
3. Commit and push to `rust`; wait for CI.
4. Tag with `v*` and push the tag.
5. Confirm the workflow publishes the GitHub release assets listed above.
6. Install a published asset and verify `oy --help`.
