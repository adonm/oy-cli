# Contributing

Keep `oy` small, boring, and useful.

## Quick start

```bash
mise install        # optional: installs the Rust MSRV toolchain and just
just check          # stable local checks (fmt, clippy, tests, docs, help smoke)
just ci             # optional CI-parity checks; requires nextest and nightly Miri
just fix            # auto-format and apply clippy suggestions, then check
just run -- chat    # run oy with arguments during development
```

If you don't have [`mise`](https://mise.jdx.dev/), install Rust and
[`just`](https://github.com/casey/just) yourself (`cargo install just`,
`brew install just`, etc.) or run the individual commands listed below.

Optional CI-parity checks require [`cargo-nextest`](https://nexte.st/) and
nightly Rust with [`miri`](https://github.com/rust-lang/miri/#using-miri):

```bash
cargo install cargo-nextest --locked --version 0.9.135
rustup toolchain install nightly --profile minimal --component miri --component rust-src
```

## Local checks

Run these before opening a PR. `just check` runs them all with stable Cargo.

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
cargo test --doc --locked
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked
cargo run --locked -- --help
```

Run clippy after formatting so its suggestions apply cleanly. Keep
`--all-targets --locked -- -D warnings` intact: it checks tests/examples as
well as the binary and treats every lint as something to fix before review.
CI uses nextest for non-doc tests because it does not fail fast, reports slow
tests, and writes a JUnit report via `.config/nextest.toml`; `just ci` runs
that optional path locally when nextest and nightly Miri are installed. Keep
`cargo test --doc --locked` because nextest does not run rustdoc tests.

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

- **LLM internals roadmap** — help keep `oy` on its smaller owned LLM boundary;
  see the month-by-month plan below.
- **`docs/GOOD_FIRST_ISSUES.md`** — limited-scope tasks suitable for new
  contributors.
- **`ISSUES.md`** — the latest audit findings; each finding lists the affected
  files and the trust boundary.
- **GitHub issues** — look for [`good-first-issue`][gfi] and
  [`help-wanted`][hw] labels.

[gfi]: https://github.com/wagov-dtt/oy-cli/labels/good-first-issue
[hw]: https://github.com/wagov-dtt/oy-cli/labels/help-wanted

## LLM internals roadmap

Goal: reduce code and improve quality by owning the small LLM boundary `oy`
actually needs, while keeping behavior stable. Port OpenCode's shape
(`request -> route -> protocol -> transport -> tool loop`), not its full
provider surface.

| Month | Outcome | Keep it small by |
|---|---|---|
| 1 | **Done:** added `src/llm/` facade with `LlmRequest`, `LlmResponse`, `Message`, `ToolSpec`, `ModelRoute`, and a `ChatBackend` trait. | No wire-protocol rewrite; request/response conversion goldens covered the adapter seam. |
| 2 | **Done:** moved transcript storage and tool definitions to `oy`-owned types. `agent::model` accepts `llm::Message` directly, and `src/tools/registry.rs` remains the single tool schema registry. | One tool schema registry, one message shape, no provider traits. |
| 3 | **Done:** added non-streaming native OpenAI Chat and OpenAI Responses backends, reusing `agent::auth`, OpenCode route metadata, and the existing tool schema registry/tool loop boundary. | No new providers, credential paths, or streaming surface. |
| 4 | **Done:** made the native backend the default, removed the previous external backend and compatibility shims, and kept only OpenAI-compatible Chat/Responses routes. | Keep only OpenAI-compatible protocols unless a concrete user need justifies more. |
| 5 | **Done:** hardened the native tool loop without widening capability: repeated identical failed tool calls are blocked, unknown tools return enabled-tool hints, tool failures use consistent `TOOL_ERROR`/`RECOVERY` markers, model-visible tool output is capped with head/tail preservation, and tool-only churn has a progress guard. | Changes stay local to `src/llm/openai.rs` and `src/tools/output.rs`; Chat and Responses share focused coverage; the default tool-round budget is unchanged. |
| 6 | Make retries side-effect aware and trim duplicated loop code: avoid whole-prompt retries after write/shell/persistent todo side effects, add jitter/fewer provider retry attempts, share Chat/Responses tool-round bookkeeping through small helpers, and tighten per-argument schema descriptions for common bad calls. | Add one explicit side-effect signal near `tools::invoke_inner`; keep retry policy in `agent::retry`; extract only repeated loop mechanics, not a new framework or provider abstraction. |

Every month: delete obsolete compatibility code, add focused request/response
goldens, keep auth lookup in `agent::auth`, and do not add new providers,
process execution, or credential paths just to make the abstraction look
complete.

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
| `.github/workflows/ci.yml` | CI checks (fmt, clippy, nextest, doc tests, miri smoke, rustdoc, help smoke, doc drift, package) |
| `.github/workflows/release.yml` | Release builds, artifact attestation, and crate publishing |
| `justfile` | Local dev task runner; `just check` / `just ci` / `just fix` / `just run` |

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
