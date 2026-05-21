# oy

[![Crates.io](https://img.shields.io/crates/v/oy-cli.svg)](https://crates.io/crates/oy-cli)
[![docs.rs](https://docs.rs/oy-cli/badge.svg)](https://docs.rs/oy-cli)

`oy` is a local AI coding CLI for your shell. It helps you inspect a codebase, ask questions, make small edits, run commands, and audit repositories from the current workspace.

## Quick start

```bash
mise use cargo-binstall cargo:oy-cli # install oy with mise
oy doctor                                    # check setup
oy model                                     # choose or confirm a model
oy "summarize this repo"
oy audit                                     # write an audit report to ISSUES.md
oy chat                                      # start an interactive session
```

For an untrusted repository, start read-only:

```bash
oy chat --mode plan
```

## Install

Recommended:

```bash
mise use cargo-binstall cargo:oy-cli
oy --help
```

With Cargo:

```bash
cargo install oy-cli
oy --help
```

When developing without installing, replace `oy` with `cargo run --`:

```bash
cargo run -- "summarize this repo"
cargo run -- chat
```

## What you need

- `bash`
- a model provider credential, or a local OpenAI-compatible server
- Rust only if building from source

Start with:

```bash
oy doctor
oy model
```

These commands show what is configured and what to do next.

## Common commands

| Command | Use it for |
|---|---|
| `oy "prompt"` | Run one task in the current workspace |
| `oy chat` | Interactive chat with slash commands and history |
| `oy chat --mode plan` | Read-only mode for looking around safely |
| `oy run [prompt]` | Explicit one-shot run; also accepts piped input |
| `oy run --out path "prompt"` | Save the response to a file |
| `oy audit [focus]` | Audit the repo and write `ISSUES.md` by default |
| `oy model [filter]` | List, choose, or save a model |
| `oy doctor` | Check setup and local state |
| `oy --help` | Show CLI help |

## Examples

```bash
oy "explain the project layout"
oy "inspect src/main.rs and suggest a simpler design"
oy "fix the failing tests"
oy audit "security and complexity"
oy run --out docs/plan.md "write a migration plan"
echo "update the changelog" | OY_NON_INTERACTIVE=1 oy run
```

Use a different workspace:

```bash
OY_ROOT=../my-project oy "summarize this repo"
```

## Model setup

`oy` supports several model backends. The easiest path is to run `oy doctor`, then `oy model`.

### Model metadata and selection

If you don't have a model provider yet, [OpenCode Go](https://opencode.ai/go) is a decent starting subscription for open-weight models — it provides access to DeepSeek V4, Qwen, Kimi, GLM, and others for $10/month. Install and configure OpenCode, subscribe to Go, then `oy` will pick up the credentials automatically.

`oy model` uses `opencode models --verbose` as the model metadata source. That keeps provider/model listings in OpenCode; `oy` only keeps the small route/profile metadata needed by its Rust-native backend. Install and configure OpenCode credentials for the providers you want listed, then run:

```bash
oy model                 # list currently routable models from opencode models --verbose
oy model <provider/model-from-list>
```

OpenAI can also be used directly without OpenCode metadata:

```bash
export OPENAI_API_KEY=...
export OPENAI_BASE_URL=https://your-endpoint.example/v1  # optional
oy model openai/gpt-4.1
```

GitHub Copilot, OpenAI-compatible providers listed by OpenCode, xAI, OpenRouter, Azure OpenAI, Cloudflare AI Gateway, Cloudflare Workers AI, and Amazon Bedrock Converse route through `oy`'s Rust-native LLM backend. Transcripts, tool schemas, cache hints, streamed events, and tool execution use `oy`-owned types. Anthropic and Google/Gemini provider entries are detected but fail closed until their native protocols are ported. Newer reasoning models that require `/responses` are handled by narrow compatibility shims; Copilot routes require a Copilot API token rather than a GitHub access token.

Provider-specific direct environment variables:

| Provider | Variables |
|---|---|
| OpenAI | `OPENAI_API_KEY`, optional `OPENAI_BASE_URL` |
| GitHub Copilot | `GITHUB_COPILOT_API_KEY` or `COPILOT_API_KEY` |
| xAI | `XAI_API_KEY`, optional `XAI_BASE_URL` |
| OpenRouter | `OPENROUTER_API_KEY`, optional `OPENROUTER_BASE_URL` |
| Azure OpenAI | `AZURE_OPENAI_API_KEY` plus `AZURE_OPENAI_BASE_URL` or `AZURE_OPENAI_RESOURCE_NAME`; optional `AZURE_OPENAI_API_VERSION` |
| Cloudflare AI Gateway | `CLOUDFLARE_API_TOKEN` or `CF_AIG_TOKEN`, plus `CLOUDFLARE_AI_GATEWAY_BASE_URL` or `CLOUDFLARE_ACCOUNT_ID`; optional `CLOUDFLARE_AI_GATEWAY_ID` |
| Cloudflare Workers AI | `CLOUDFLARE_API_KEY` or `CLOUDFLARE_WORKERS_AI_TOKEN`, plus `CLOUDFLARE_WORKERS_AI_BASE_URL` or `CLOUDFLARE_ACCOUNT_ID` |
| Amazon Bedrock | `BEDROCK_API_KEY` or `AWS_BEARER_TOKEN_BEDROCK`, or SigV4 via `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`; optional `AWS_SESSION_TOKEN`, `AWS_REGION`, `AWS_DEFAULT_REGION`, `BEDROCK_BASE_URL` |

The last five saved model selections are kept as a local quick history. When two or more recent models exist, interactive `oy model` and `/model` show that recent list first, with options to inspect the full OpenCode listing or clear the recent history.

## Audit

`oy audit [focus]` creates a deterministic, no-tools repository audit. By default it writes `ISSUES.md`.

```bash
oy audit
oy audit "security and complexity"
oy audit "auth paths" --out docs/audit.md
oy audit --max-chunks 240
```

The model does not get file-edit tools, shell access, or live search during an audit. The runner collects the review input first, then asks the model to report evidence-first findings. Large repositories fail closed above 80 review chunks by default; pass `--max-chunks N` when you intentionally want a larger audit.

## Interactive chat

In `oy chat`:

- Enter sends
- Alt+Enter or Shift+Enter inserts a newline
- pasted multiline text stays editable before submit
- `/help` lists commands
- `/status` shows model, workspace, mode, context, and todos
- `/ask <question>` is read-only research; it cannot edit files or run `bash`
- `webfetch` can fetch public docs/API pages and return markdown, text, HTML, or XML using the Spider MCP scrape shape; this build uses Spider's default HTTP crawler setup without Chrome/wait/proxy support

For multi-step work, `oy` keeps an in-memory todo list. It writes `TODO.md` only when you explicitly ask and the current mode allows it.

The native tool loop treats tool results as a recovery boundary: failures are returned to the model with `TOOL_ERROR`/`RECOVERY` guidance, unknown tool names include enabled-tool hints, repeated identical failed calls are blocked, hosted/provider-executed tool events are not dispatched locally, and oversized model-visible tool output is truncated with head/tail preservation.

## Safety modes

`oy` is not a sandbox. It can run commands and edit files with your user permissions. Command output, file snippets, and prompts may be sent to your model provider.

Use safer modes when you are unsure:

| Mode | File edits | Bash | When to use |
|---|---:|---:|---|
| `default` / `ask` | asks | asks | Normal work |
| `plan` / `read` | no | no | Untrusted repos or first look |
| `accept-edits` / `edit` | auto | asks | Trusted mechanical edits |
| `auto-approve` / `auto` | auto | auto | Trusted unattended runs only |
| `/ask` | no | no | Research-only questions |

Avoid `auto-approve` unless you trust the workspace, task, and model. For untrusted code, prefer a container or VM and start with `oy chat --mode plan`.

## Sessions and local files

Resume previous work:

```bash
oy chat --continue-session
oy run --continue-session "next task"
oy run --resume <name-or-number> "next task"
```

Default local paths:

| Path | Purpose |
|---|---|
| `~/.config/oy-rust/config.json` | Saved model id and recent model history |
| `~/.config/oy-rust/sessions/` | Saved transcripts |
| `~/.config/oy-rust/history/` | Chat history |

## Useful environment variables

| Variable | Purpose |
|---|---|
| `OY_MODEL` | Override model for this session |
| `OY_ROOT` | Run against a different workspace |
| `OY_NON_INTERACTIVE` | Disable approval/question prompts for automation |
| `OY_CONFIG` | Override config file path |
| `OY_COLOR` | `auto`, `always`, or `never`; `NO_COLOR` disables color |
| `OY_MAX_TOOL_ROUNDS` | Tool-call budget per prompt; default `512` |
| `OPENAI_API_KEY`, `OPENAI_BASE_URL` | OpenAI auth/endpoint |
| `GITHUB_COPILOT_API_KEY`, `COPILOT_API_KEY` | Copilot API-token auth |
| `OPENROUTER_API_KEY`, `XAI_API_KEY`, `AZURE_OPENAI_API_KEY` | Direct provider auth for OpenRouter, xAI, and Azure OpenAI |
| `BEDROCK_API_KEY`, `AWS_BEARER_TOKEN_BEDROCK`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` | Amazon Bedrock auth |
| `OY_TITLE` | Terminal title/zellij pane progress: `off`, `never`, `0` disable; `on`, `always`, `1` force in human output modes |
| `LOCAL_API_KEY` | Optional local `local-<port>` shim key; defaults to `oy-local` |

## Troubleshooting

- **No model configured:** run `oy doctor`, then `oy model`.
- **Provider call failed:** check credentials, selected model, and network/local server access.
- **Tool denied:** switch mode only if the workspace is trusted, for example `oy chat --mode accept-edits`.
- **Untrusted repo:** use `oy chat --mode plan` first.
- **Long task stopped early:** increase `OY_MAX_TOOL_ROUNDS`, for example `OY_MAX_TOOL_ROUNDS=2048 oy "finish the migration"`.

## Development

Maintainer docs:

- `CONTRIBUTING.md` — local checks, design rules, and release-note expectations.
- `docs/architecture.md` — runtime flow, module map, trust boundaries, and audit pipeline.
- `docs/tool-safety.md` — tool capabilities, approval modes, and boundary guidance.

Top-level source layout:

| Path | Role |
|---|---|
| `src/agent.rs`, `src/agent/` | Model routing, providers, sessions, context compaction |
| `src/cli.rs`, `src/cli/` | CLI commands, config, terminal UI, chat shell |
| `src/tools.rs` | Workspace tools, approvals, previews, safety boundaries |
| `src/audit.rs` | Deterministic audit runner and prompts |
| `src/lib.rs`, `src/main.rs` | Library facade and binary entry point |

Checks:

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo nextest run --all-targets --locked --profile ci
cargo test --doc --locked
cargo +nightly miri test --locked miri_smoke
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked
cargo run --locked -- --help
```

## License

Apache License 2.0
