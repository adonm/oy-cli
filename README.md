# oy

Small local AI coding CLI for your shell, rewritten in Rust.

It uses:

- [`genai`](https://crates.io/crates/genai) for model access and tool calls
- [`rustyline`](https://crates.io/crates/rustyline) for chat UX
- ripgrep ecosystem crates (`ignore`, `globset`, `regex`) for repo search
- [`toon-format`](https://crates.io/crates/toon-format) for compact tool payload encoding

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
cargo run -- model github_copilot::openai/gpt-4.1-mini
cargo run -- model local-8080::qwen3.5
```

In chat, `/ask <question>` is research-only: no `bash`, no file changes, but public `webfetch` is still allowed.

## Model ids

`oy model` now:

- shows the current normalized model id
- introspects relevant auth env vars and auto-populates `GITHUB_TOKEN` from `gh auth token` when missing
- sends direct `GET /models` requests to configured OpenAI-compatible endpoints
- covers default OpenAI, `OPENAI_BASE_URL`, GitHub/Copilot-style endpoints, and local `local-<port>` shims
- hides missing auth and failed/static provider sections to keep output high-signal
- in a TTY, `oy model` can open an interactive fuzzy picker for choosing and saving a model

Use native `genai` model ids in docs, config, and examples:

- plain provider-native ids when `genai` can infer the adapter:
  - `gpt-5.4-mini`
  - `gemini-2.0-flash`
  - `claude-3-7-sonnet-latest`
- explicit adapter-prefixed ids when needed:
  - `github_copilot::openai/gpt-4.1-mini`
  - `local-8080::qwen3.5`
  - `local-11434::qwen3.5`

Compatibility aliases like `copilot:...` and `local-8080:...` are still accepted, but the Rust CLI normalizes them to native `genai` ids internally.

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

Saved sessions keep the transcript and active agent profile under `~/.config/oy/sessions/`.

## Audit commands

`oy audit [focus]`:

- runs a repo audit with the normal audit prompt
- writes the final Markdown report to `ISSUES.md`
- includes a transparency line with the normalized active model id

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
| `OY_SHIM` | Apply a shim when the model name is bare |
| `OY_NON_INTERACTIVE` | Set to `1` to disable approval and prompt pauses |
| `OY_UNATTENDED_LIMIT` | Agent deadline window, such as `1h`, `30m`, or `3600s` |
| `OY_RALPH_LIMIT` | Ralph deadline window, such as `3h`, `90m`, or `3600s` |
| `OY_ROOT` | Run against a different workspace |
| `OY_SYSTEM_FILE` | Append extra system instructions |
| `OY_CONFIG` | Override config path (default: `~/.config/oy/config.json`) |
| `OY_YOLO` | Start with all tool approvals enabled |
| `OY_MAX_BASH_CMD_BYTES` | Override max accepted bash command size |
| `OPENAI_API_KEY` | OpenAI-compatible auth, including many local/self-hosted adapters |
| `OPENAI_BASE_URL` | Override the OpenAI-compatible endpoint |

### Config file

```json
{"shim": "github_copilot", "model": "openai/gpt-4.1-mini"}
```

Only `model` and `shim` are persisted in config.

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
cargo run -- model github_copilot::openai/gpt-4.1-mini
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
