# oy

Small local AI coding CLI for your shell. It can inspect files, search content, fetch public docs, run commands, and edit files inside the current workspace.

## Quick start

```bash
oy "inspect this repo and summarize the main risks"
oy audit "focus on security and complexity"
oy chat
oy chat --agent plan
oy chat --continue-session
oy run --resume 20260325 "finish the refactor"
```

When developing from source, replace `oy` with `cargo run --`:

```bash
cargo run -- "inspect this repo"
cargo run -- chat
```

## Install

```bash
cargo install --path .
oy --help
```

## Requirements

- Rust toolchain for source builds
- `bash`
- credentials for a `genai`-supported backend, or a local OpenAI-compatible server

## Commands

| Command | Purpose |
|---|---|
| `oy "prompt"` | Run one task in the current workspace |
| `oy run [--out path] [prompt]` | Explicit one-shot task; also accepts piped stdin |
| `oy chat` | Interactive session with slash commands and history |
| `oy model [filter]` | List, choose, and save a model id/routing shim |
| `oy audit [focus]` | Multi-pass LLM audit to `ISSUES.md` using docs, SLOC, and pinned workspace chunks |
| `oy ralph "prompt"` | Re-run a maintenance prompt until the deadline |
| `oy --help` | Show CLI help |

## Common tasks

```bash
oy "inspect the main module and suggest improvements"
OY_ROOT=./my-project oy "fix the failing tests"
echo "update the changelog" | OY_NON_INTERACTIVE=1 oy run
oy run --out docs/plan.md "write a migration plan"
oy audit "security, complexity, and performance"
oy chat --agent accept-edits
oy model copilot::gpt-4.1-mini
oy model local-8080::qwen3.5
```

## Audit

`oy audit [focus]` plans chunks from workspace text files, includes docs and SLOC context, asks the model to inspect each pinned chunk, appends draft findings to `ISSUES.md`, then runs a final reduction pass. The final report keeps up to 20 detailed high-priority findings and summarizes the rest. Use `--chunk-lines` to control chunk size.

```bash
oy audit
oy audit auth --chunk-lines 4000
oy audit --standards "auth and storage"
```

## Model setup

OpenAI-compatible endpoint:

```bash
export OPENAI_API_KEY=...
export OPENAI_BASE_URL=https://your-endpoint.example/v1  # optional
oy model gpt-4.1-mini
```

GitHub Copilot:

```bash
gh auth login
oy model copilot::gpt-4.1-mini
```

Local OpenAI-compatible server:

```bash
oy model local-8080::qwen3.5
oy chat
```

## Chat UX

- Enter sends
- Alt+Enter or Shift+Enter inserts a newline
- pasted multiline text stays editable before submit
- `/help` lists commands
- `/status` shows model, workspace, approvals, context, and todos
- short aliases include `/q`, `/h`, `/m`, `/t`, `/u`

`/ask <question>` is research-only: no `bash`, no file changes, but public `webfetch` is still allowed.

## Safety and approval modes

`oy` is not a sandbox. It can run shell commands and modify files with your permissions. `bash` inherits your environment, so git, cloud, SSH, package registry, and Docker credentials visible to your shell may be visible to approved commands. Model providers may receive prompts, source snippets, tool output, and command output.

For untrusted repositories, start read-only and contained:

```bash
oy chat --agent plan
docker run --rm -it -v "$PWD:/workspace:ro" -w /workspace oy-image oy chat --agent plan
docker run --rm -it -v "$PWD:/workspace:rw" -w /workspace -e OPENAI_API_KEY oy-image oy chat
```

Avoid mounting the host Docker socket into AI-assisted containers; it is usually host-root-equivalent. Avoid `auto-approve` and `OY_YOLO` unless the workspace, model, and requested task are trusted.

| Mode | File edits | Bash | Notes |
|---|---:|---:|---|
| `default` | asks | asks | Normal interactive mode |
| `plan` | unavailable | unavailable | Read-only exploration and planning |
| `accept-edits` | auto | asks | Useful for trusted mechanical edits |
| `auto-approve` | auto | auto | Highest-risk unattended mode |
| `/ask` | unavailable | unavailable | No-write research; public `webfetch` allowed |
| `OY_YOLO=1` | auto | auto | Legacy environment override for approvals |

Non-interactive mode cannot pause for approval or questions. Use explicit agents/env only in workspaces and automation contexts you trust.

Protections include workspace-bound file tools, public-only `webfetch`, read-only agent profiles, explicit approval modes, and clamped terminal previews.

## Preview and truncation behavior

Tool output shown in the terminal is a preview, not always the full tool result.

- `read` returns line slices; use `offset` and `limit` to fetch more.
- `search`, `list`, and `replace` previews show bounded item lists and say when more exist.
- `bash` and `webfetch` summarize long output before it goes back to the model and mark truncated stdout/stderr/body text.
- Preview lines are clamped so terminal output remains scannable.

## Model ids and routing shims

`oy model`:

- shows the current configured model id and active routing shim
- introspects relevant auth env vars and auto-populates `GITHUB_TOKEN` from `gh auth token` when missing
- sends direct `GET /models` requests to configured OpenAI-compatible endpoints
- includes built-in model hints as selectable choices even when endpoint introspection is unavailable
- in a TTY, can prompt for choosing and saving a model

Use exact `genai` model ids in config. Endpoint-qualified choices such as `copilot::gpt-4.1-mini` or `local-8080::qwen3.5` infer routing.

## Sessions and local files

Saved chat sessions can be resumed:

- `oy chat --continue-session`
- `oy run --continue-session "next task"`
- `oy run --resume <name-or-number> "next task"`
- in chat, `/save [name]` and `/load [name]`

Default paths:

| Path | Purpose |
|---|---|
| `~/.config/oy-rust/config.json` | Saved model id and routing shim |
| `~/.config/oy-rust/sessions/` | Saved transcripts |
| `~/.config/oy-rust/history/` | Chat history |

Override the config file path with `OY_CONFIG` and workspace with `OY_ROOT`.

## Configuration

| Variable | Purpose |
|---|---|
| `OY_MODEL` | Override model for this session |
| `OY_SHIM` | Override routing shim (`openai`, `copilot`, `local-<port>`, `codex`, `opencode`, `bedrock-mantle`) |
| `OY_NON_INTERACTIVE` | Disable interactive approval/question pauses |
| `OY_RALPH_LIMIT` | Ralph deadline window, such as `3h`, `90m`, or `3600s` |
| `OY_ROOT` | Run against a different workspace |
| `OY_SYSTEM_FILE` | Append extra system instructions |
| `OY_CONFIG` | Override config path |
| `OY_YOLO` | Start with all tool approvals enabled |
| `OY_COLOR` | `auto`, `always`, or `never`; `NO_COLOR` disables color |
| `OY_MAX_BASH_CMD_BYTES` | Override max accepted bash command size |
| `OPENAI_API_KEY`, `OPENAI_BASE_URL` | OpenAI-compatible auth/endpoint |
| `COPILOT_GITHUB_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN` | Copilot shim auth |
| `LOCAL_API_KEY` | Local `local-<port>` shim key; falls back to `OPENAI_API_KEY` then `oy-local` |
| `OPENCODE_API_KEY` | OpenCode shim key; falls back to `~/.local/share/opencode/auth.json` |
| `AWS_REGION`, `AWS_DEFAULT_REGION` | Bedrock-Mantle region detection |

## Troubleshooting

- No model configured: run `oy model copilot::gpt-4.1-mini`, set `OPENAI_API_KEY` and run `oy model gpt-4.1-mini`, or use `oy model local-8080::qwen3.5`.
- Tool denied: switch agent mode only if trusted, for example `oy chat --agent accept-edits` for automatic file edits.
- Model call failed: check `oy model`, provider credentials, routing shim, and local server availability.
- Untrusted repo: use `oy chat --agent plan` first, preferably inside a container or VM.

## Development

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run -- --help
```

## License

Apache License 2.0
