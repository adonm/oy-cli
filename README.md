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

### OpenAI or OpenAI-compatible

```bash
export OPENAI_API_KEY=...
export OPENAI_BASE_URL=https://your-endpoint.example/v1  # optional
oy model gpt-4.1-mini
```

### GitHub Copilot

```bash
gh auth login
oy model copilot::gpt-4.1-mini
```

### AWS Bedrock

```bash
aws configure sso        # one-time setup, or use normal AWS env credentials
export AWS_PROFILE=my-sso
export AWS_REGION=ap-southeast-2
oy model bedrock::global.amazon.nova-2-lite-v1:0
```

### Local OpenAI-compatible server

```bash
oy model local-8080::qwen3.5
oy chat
```

Useful exact model ids include:

- `copilot::gpt-4.1-mini`
- `bedrock::global.amazon.nova-2-lite-v1:0`
- `bedrock-mantle::moonshotai.kimi-k2.5`
- `opencode::gpt-5.1-codex-max`
- `opencode-go::kimi-k2.5`
- `local-8080::qwen3.5`

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
- `webfetch` can fetch public docs/API pages and follows redirects by default; it sends an honest `oy-cli/<version>` user agent plus doc-friendly `Accept` headers, while still blocking private/local targets and sensitive headers

For multi-step work, `oy` keeps an in-memory todo list. It writes `TODO.md` only when you explicitly ask and the current mode allows it.

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
| `~/.config/oy-rust/config.json` | Saved model id and routing shim |
| `~/.config/oy-rust/sessions/` | Saved transcripts |
| `~/.config/oy-rust/history/` | Chat history |

## Useful environment variables

| Variable | Purpose |
|---|---|
| `OY_MODEL` | Override model for this session |
| `OY_SHIM` | Override routing shim |
| `OY_ROOT` | Run against a different workspace |
| `OY_NON_INTERACTIVE` | Disable approval/question prompts for automation |
| `OY_CONFIG` | Override config file path |
| `OY_COLOR` | `auto`, `always`, or `never`; `NO_COLOR` disables color |
| `OY_MAX_TOOL_ROUNDS` | Tool-call budget per prompt; default `512` |
| `OPENAI_API_KEY`, `OPENAI_BASE_URL` | OpenAI-compatible auth/endpoint |
| `COPILOT_GITHUB_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN` | Copilot auth |
| `AWS_PROFILE`, `AWS_REGION`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN` | Bedrock auth/region |
| `LOCAL_API_KEY` | Local `local-<port>` shim key |

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
| `src/agent.rs` | Model routing, providers, sessions, context compaction |
| `src/cli.rs` | CLI commands, config, terminal UI, chat shell |
| `src/tools.rs` | Workspace tools, approvals, previews, safety boundaries |
| `src/audit.rs` | Deterministic audit runner and prompts |
| `src/lib.rs`, `src/main.rs` | Library facade and binary entry point |

Checks:

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked
cargo run --locked -- --help
```

## License

Apache License 2.0
