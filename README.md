# oy

Small local AI coding CLI for your shell. The active implementation is Rust; the old Python package lives under `legacy-python/` for reference only.

Design summary:

- `src/main.rs` installs syntax-highlighting stdout/stderr wrappers, then enters the async CLI.
- `src/cli.rs` owns command parsing and orchestration for `run`, `chat`, `ralph`, `model`, `audit`, `audit-logic`, and `renovate-local`.
- `src/agent.rs` owns session state, transcript persistence, token estimates, and the `genai` request/tool loop.
- `src/tools.rs` exposes the model tools and enforces workspace bounds, approval gates, public-only fetches, archive reads, and output summarization.
- `src/config.rs` owns config paths, saved model/shim config, agent profiles, prompts loaded from `assets/session_text.toml`, env flags, and saved sessions.
- `src/model.rs` normalizes model ids, resolves routing shims, builds `genai` clients, and introspects OpenAI-compatible model endpoints.
- `src/ui.rs` owns reedline chat, slash commands, prompts, and interactive model selection.
- `src/highlight.rs` syntax-highlights terminal output with `syntect`.

Crate notes:

- `genai` is the model client and tool-call transport.
- `tokio` drives async CLI execution, subprocesses, timeouts, DNS, and network operations.
- `clap` defines the command surface.
- `serde`, `serde_json`, and `toml` load/save config, sessions, tool args/results, and prompt text.
- `reqwest` + `url` implement `webfetch` and endpoint model introspection over rustls/http2.
- `ignore`, `glob`, `globset`, `grep-regex`, `grep-searcher`, and `regex` implement workspace listing/search/replace while respecting gitignore-style filters.
- `tokei` powers the `sloc` tool.
- `reedline-repl-rs` provides reedline input/history for chat and selection prompts.
- `syntect` highlights Markdown/JSON/TOML/shell-ish terminal output.
- `tiktoken-rs` estimates context size for wait status and `/tokens`.
- `toon-format` compacts tool outputs before they go back to the model.
- `html2md` converts fetched HTML pages to Markdown-ish text.
- `dirs` locates the user config directory.
- `chrono` stamps sessions, audit reports, and local Renovate reports.
- `flate2`, `tar`, and `zip` let file tools inspect compressed text and archive members without extracting into the workspace.
- `anyhow` keeps error paths simple at CLI boundaries.

## Quick start

```bash
cargo run -- "inspect this repo and summarize the main risks"
cargo run -- chat
cargo run -- chat --agent plan
cargo run -- run --resume 20260325 "finish the refactor"
cargo run -- audit "focus on authentication"
cargo run -- audit-logic "focus on runtime behavior"
```

## Common tasks

```bash
cargo run -- "inspect the main module and suggest improvements"
OY_ROOT=./my-project cargo run -- "fix the failing tests"
echo "update the changelog" | OY_NON_INTERACTIVE=1 cargo run -- run
cargo run -- chat
cargo run -- chat --agent accept-edits
cargo run -- chat --continue-session
cargo run -- ralph "re-run the maintenance prompt every minute"
cargo run -- model                         # list detected auth/env + available models
cargo run -- model copilot::gpt-4.1-mini
cargo run -- model local-8080::qwen3.5
```

In chat, `/ask <question>` is research-only: no `bash`, no file changes, but public `webfetch` is still allowed.
Chat uses reedline prompts, so terminal scrollback/history work normally. Tab completes commands/models/choices. Use `/help` for commands, `/history [limit]` to print the transcript, and short aliases like `/q`, `/h`, `/m`, `/t`, `/u`.

## Model ids

`oy model`:

- shows the current configured model id and active routing shim
- introspects relevant auth env vars and auto-populates `GITHUB_TOKEN` from `gh auth token` when missing
- sends direct `GET /models` requests to configured OpenAI-compatible endpoints
- covers Python-compatible routing shims: `openai`, `copilot`, `local-<port>`, bearer-token `codex`/`opencode`, and Bedrock-Mantle auth visibility
- hides missing auth and failed/static provider sections to keep output high-signal
- includes built-in model hints as selectable choices even when endpoint introspection is unavailable
- in a TTY, `oy model` can open an interactive fuzzy picker for choosing and saving a model

Use exact `genai` model ids in config. `oy model` may also show endpoint-qualified choices (`shim::model`) so it can infer routing:

- plain provider-native ids when `genai` can infer the adapter:
  - `gpt-5.4-mini`
  - `gemini-2.0-flash`
  - `claude-3-7-sonnet-latest`
- explicit genai adapter ids when needed:
  - `openai_resp::gpt-5.5`
- endpoint-qualified picker/CLI choices when routing should be inferred:
  - `copilot::gpt-4.1-mini`
  - `local-8080::qwen3.5`
  - `local-11434::qwen3.5`

Rust config stores the exact `genai` model id in `model`. When you choose from an autodetected endpoint, Rust may also persist a `shim` such as `copilot`, `local-8080`, `codex`, or `opencode`; the shim is only for endpoint/auth routing and does not rewrite the model id.

## Agent profiles

- `default` — normal tool approvals
- `plan` — read-only exploration and planning
- `accept-edits` — auto-approves file edits, but not shell commands
- `auto-approve` — auto-approves all available tools

Examples:

```bash
cargo run -- chat --agent plan
cargo run -- run --agent accept-edits "rename the helper and update callers"
cargo run -- chat --agent auto-approve
```

## Session continuation

Saved chat sessions can be resumed:

- `oy chat --continue-session`
- `oy run --continue-session "next task"`
- `oy run --resume <name-or-number> "next task"`
- in chat, `/save [name]` and `/load [name]`

Saved sessions keep the transcript and active agent profile under `~/.config/oy-rust/sessions/`.

## Audit commands

`oy audit [focus]`:

- runs a repo audit with the normal audit prompt
- writes the final Markdown report to `ISSUES.md`
- includes a transparency line with the active model id

`oy audit-logic [focus]` is stricter:

- focuses on runtime behavior, security boundaries, auth/authz, state changes, parsing, persistence, and network behavior
- deprioritizes docs/comment quality unless it changes behavior

Compatibility flags are still accepted:

```bash
cargo run -- audit auth --from src/
cargo run -- audit-logic payments --phase phase2
```

## Renovate local

`oy renovate-local` runs Renovate in local lookup mode and writes a report to `.tmp/renovate-YYYY-MM-DD.json`.

Requirements:

- `renovate` installed in your `PATH`
- a GitHub token via `RENOVATE_GITHUB_COM_TOKEN`, `GH_TOKEN`, or `GITHUB_TOKEN`
  - if unset, `oy` also tries `gh auth token`

## Configuration

### Environment variables

| Variable | Purpose |
|---|---|
| `OY_MODEL` | Override the model for this session using a native `genai` model id |
| `OY_SHIM` | Override routing shim (`openai`, `copilot`, `local-<port>`, `codex`, `opencode`, `bedrock-mantle`) |
| `OY_NON_INTERACTIVE` | Set to `1` to disable approval and prompt pauses |
| `OY_UNATTENDED_LIMIT` | Agent deadline window, such as `1h`, `30m`, or `3600s` |
| `OY_RALPH_LIMIT` | Ralph deadline window, such as `3h`, `90m`, or `3600s` |
| `OY_ROOT` | Run against a different workspace |
| `OY_SYSTEM_FILE` | Append extra system instructions |
| `OY_CONFIG` | Override config path (default: `~/.config/oy-rust/config.json`) |
| `OY_YOLO` | Start with all tool approvals enabled |
| `OY_MAX_BASH_CMD_BYTES` | Override max accepted bash command size |
| `OPENAI_API_KEY` | OpenAI-compatible auth, including many local/self-hosted adapters |
| `OPENAI_BASE_URL` | Override the OpenAI-compatible endpoint |
| `COPILOT_GITHUB_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN` | Copilot shim auth, matching Python lookup order |
| `COPILOT_BASE_URL` | Override Copilot shim endpoint (default `https://api.githubcopilot.com`) |
| `LOCAL_API_KEY` | Local `local-<port>` shim key; falls back to `OPENAI_API_KEY` then `oy-local` |
| `OPENCODE_API_KEY` | OpenCode shim key; falls back to `~/.local/share/opencode/auth.json` |
| `AWS_REGION`, `AWS_DEFAULT_REGION` | Bedrock-Mantle region detection |

### Config file

```json
{"model": "openai_resp::gpt-5.5", "shim": "copilot"}
```

`model` is the exact genai id. `shim`, when present, only selects endpoint/auth routing (for example GitHub Copilot token + base URL).

## Installation

For local development:

```bash
cargo build
cargo run -- --help
```

For a local install:

```bash
cargo install --path .
```

## Requirements

- Rust toolchain
- `bash`
- credentials for a `genai`-supported backend, or a local OpenAI-compatible server

## Authentication

OpenAI-compatible endpoint:

```bash
export OPENAI_API_KEY=...
export OPENAI_BASE_URL=https://your-endpoint.example/v1  # optional
```

GitHub Copilot via native `genai` id:

```bash
cargo run -- model copilot::gpt-4.1-mini
```

Local OpenAI-compatible servers:

```bash
cargo run -- model local-8080::qwen3.5
cargo run -- chat
```

## Development

```bash
cargo fmt
cargo check
cargo test
cargo run -- --help
```

Contributor workflow lives in [`CONTRIBUTING.md`](CONTRIBUTING.md).

## Security

`oy` can run shell commands and modify files with your permissions.

Recommended:

- run in a repo or workspace you trust
- avoid exposing long-lived secrets in the environment
- use `/ask` for no-write research mode
- review generated changes before shipping

Protections include workspace-bound file tools, public-only `webfetch`, and explicit approval modes.

## License

Apache License 2.0
