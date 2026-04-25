# oy

Small local AI coding CLI for your shell. It can inspect files, search content, fetch public docs, run commands, and edit files inside the current workspace.

This branch is the Rust implementation. It uses `genai` for model transport and keeps Python-compatible routing names where useful (`copilot`, `local-<port>`, `codex`, `opencode`, `bedrock-mantle`).

## Quick start

```bash
oy "inspect this repo and summarize the main risks"
oy chat
oy chat --agent plan
oy chat --continue-session
oy run --resume 20260325 "finish the refactor"
oy audit "focus on authentication"
oy audit-logic "focus on runtime behavior"
```

When developing from source, replace `oy` with `cargo run --`:

```bash
cargo run -- "inspect this repo"
cargo run -- chat
```

## Install

### Released binary with mise

```bash
mise use -g github:wagov-dtt/oy-cli
oy --help
```

Pin a specific release:

```bash
mise use -g github:wagov-dtt/oy-cli@v0.7.0
```

Release assets are `tar.gz` archives named `oy-<tag>-<target>.tar.gz`. The current release workflow builds:

- `x86_64-unknown-linux-gnu` on `ubuntu-latest`
- `aarch64-unknown-linux-gnu` on `ubuntu-24.04-arm`
- `aarch64-apple-darwin` on `macos-14`

Windows and Intel macOS assets are not built unless `.github/workflows/release.yml` is extended.

### From source

```bash
cargo install --path .
oy --help
```

### Local development

```bash
cargo build
cargo run -- --help
```

## Requirements

- Rust toolchain for source builds
- `bash`
- credentials for a `genai`-supported backend, or a local OpenAI-compatible server

## Commands

| Command | Purpose |
|---|---|
| `oy "prompt"` | Run one task in the current workspace |
| `oy run [prompt]` | Explicit one-shot task; also accepts piped stdin |
| `oy chat` | Interactive session with slash commands and history |
| `oy model [filter]` | List, choose, and save a model id/routing shim |
| `oy audit [focus]` | Write a repo audit report to `ISSUES.md` |
| `oy audit-logic [focus]` | Audit runtime behavior and security logic |
| `oy ralph "prompt"` | Re-run a maintenance prompt until the deadline |
| `oy renovate-local` | Run local Renovate lookup and write a JSON report |
| `oy --help` | Show CLI help |

## Common tasks

```bash
oy "inspect the main module and suggest improvements"
OY_ROOT=./my-project oy "fix the failing tests"
echo "update the changelog" | OY_NON_INTERACTIVE=1 oy run
oy chat
oy chat --agent accept-edits
oy chat --continue-session
oy ralph "re-run the maintenance prompt every minute"
oy model                         # list detected auth/env + available models
oy model copilot::gpt-4.1-mini
oy model local-8080::qwen3.5
```

## First model setup

Pick one path.

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
# start your server on 127.0.0.1:8080 first
oy model local-8080::qwen3.5
oy chat
```

Bedrock Mantle:

```bash
export OY_SHIM=bedrock-mantle
export AWS_PROFILE=my-profile
export AWS_REGION=ap-southeast-2
oy model
```

## Chat UX

In chat:

- Enter sends
- Alt+Enter or Shift+Enter inserts a newline
- pasted multiline text stays editable before submit
- `/help` lists commands
- short aliases include `/q`, `/h`, `/m`, `/t`, `/u`

`/ask <question>` is research-only: no `bash`, no file changes, but public `webfetch` is still allowed. It is no-write rather than no-network.

## Safety and approval modes

`oy` can run shell commands and modify files with your permissions. `bash` inherits your environment, so git, cloud, and SSH credentials visible to your shell are visible to commands.

| Mode | File edits | Bash | Notes |
|---|---:|---:|---|
| `default` | asks | asks | Normal interactive mode |
| `plan` | unavailable | unavailable | Read-only exploration and planning |
| `accept-edits` | auto | asks | Useful for trusted mechanical edits |
| `auto-approve` | auto | auto | Highest-risk unattended mode |
| `/ask` | unavailable | unavailable | No-write research; public `webfetch` allowed |
| `OY_YOLO=1` | auto | auto | Environment override for approvals |

Non-interactive mode (`OY_NON_INTERACTIVE=1` or piped stdin) cannot pause for approval or questions. Mutating tools proceed under the active agent/tool policy, so use it only in workspaces and automation contexts you trust.

Recommended:

- run in a repo or workspace you trust
- avoid exposing long-lived secrets in the environment
- use `/ask` for no-write research mode
- review generated changes before shipping
- prefer `--agent plan` for audits or design exploration where writes are not needed

Protections include workspace-bound file tools, public-only `webfetch`, read-only agent profiles, explicit approval modes, and clamped terminal previews.

## Preview and truncation behavior

Tool output shown in the terminal is a preview, not always the full tool result.

- `read` returns line slices; use `offset` and `limit` to fetch more.
- `search`, `list`, and `replace` previews show bounded item lists and say when more exist.
- `bash` and `webfetch` summarize long output before it goes back to the model and mark truncated stdout/stderr/body text.
- Preview lines are clamped so terminal output remains scannable.

If output looks missing, check the explicit `truncated`/`… more` line and ask for a narrower follow-up read/search.

## Model ids and routing shims

`oy model`:

- shows the current configured model id and active routing shim
- introspects relevant auth env vars and auto-populates `GITHUB_TOKEN` from `gh auth token` when missing
- sends direct `GET /models` requests to configured OpenAI-compatible endpoints
- includes built-in model hints as selectable choices even when endpoint introspection is unavailable
- in a TTY, can prompt for choosing and saving a model

Use exact `genai` model ids in config. `oy model` may also show endpoint-qualified choices (`shim::model`) so it can infer routing:

- provider-native ids when `genai` can infer the adapter:
  - `gpt-5.4-mini`
  - `gemini-2.0-flash`
  - `claude-3-7-sonnet-latest`
- explicit `genai` adapter ids when needed:
  - `openai_resp::gpt-5.5`
- endpoint-qualified choices when routing should be inferred:
  - `copilot::gpt-4.1-mini`
  - `local-8080::qwen3.5`
  - `local-11434::qwen3.5`

Config stores the exact `genai` model id in `model`. When you choose from an autodetected endpoint, Rust may also persist a `shim` such as `copilot`, `local-8080`, `codex`, or `opencode`; the shim selects endpoint/auth routing and does not rewrite the model id.

## Agent profiles

| Agent | Behavior |
|---|---|
| `default` | Normal tool approvals |
| `plan` | Read-only exploration and planning |
| `accept-edits` | Auto-approves file edits, but not shell commands |
| `auto-approve` | Auto-approves all available tools |

Examples:

```bash
oy chat --agent plan
oy run --agent accept-edits "rename the helper and update callers"
oy chat --agent auto-approve
```

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

## Audit commands

`oy audit [focus]`:

- runs a repo audit with the normal audit prompt
- writes the final Markdown report to `ISSUES.md`
- includes a transparency line with the active model id

`oy audit-logic [focus]` is stricter:

- focuses on runtime behavior, security boundaries, auth/authz, state changes, parsing, persistence, and network behavior
- deprioritizes docs/comment quality unless it changes behavior

Compatibility flags are accepted:

```bash
oy audit auth --from src/
oy audit-logic payments --phase phase2
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
| `OY_NON_INTERACTIVE` | Disable interactive approval/question pauses; use only in trusted automation |
| `OY_RALPH_LIMIT` | Ralph deadline window, such as `3h`, `90m`, or `3600s` |
| `OY_ROOT` | Run against a different workspace |
| `OY_SYSTEM_FILE` | Append extra system instructions |
| `OY_CONFIG` | Override config path |
| `OY_YOLO` | Start with all tool approvals enabled |
| `OY_MAX_BASH_CMD_BYTES` | Override max accepted bash command size |
| `OY_THEME` | Force terminal highlighting theme mode: `dark` or `light` |
| `OPENAI_API_KEY` | OpenAI-compatible auth, including many local/self-hosted adapters |
| `OPENAI_BASE_URL` | Override the OpenAI-compatible endpoint |
| `COPILOT_GITHUB_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN` | Copilot shim auth, matching Python lookup order |
| `COPILOT_BASE_URL` | Override Copilot shim endpoint; default `https://api.githubcopilot.com` |
| `LOCAL_API_KEY` | Local `local-<port>` shim key; falls back to `OPENAI_API_KEY` then `oy-local` |
| `OPENCODE_API_KEY` | OpenCode shim key; falls back to `~/.local/share/opencode/auth.json` |
| `AWS_REGION`, `AWS_DEFAULT_REGION` | Bedrock-Mantle region detection |

### Config file

```json
{"model": "openai_resp::gpt-5.5", "shim": "copilot"}
```

`model` is the exact `genai` id. `shim`, when present, only selects endpoint/auth routing.

## Troubleshooting

- **Missing credentials** — start a local OpenAI-compatible server on `127.0.0.1:8080`, set `OPENAI_API_KEY`, authenticate `gh` for Copilot, run `opencode auth`, or configure AWS credentials / SSO for Bedrock Mantle.
- **stdin is not a TTY** — piping input disables `ask` and approval pauses. Set `OY_NON_INTERACTIVE=1` to make that explicit in automation.
- **Local model not found** — confirm the server listens on the port in `local-<port>` and exposes an OpenAI-compatible `/v1/models` endpoint.
- **AWS SSO session is stale** — run `aws sso login --use-device-code --no-browser`.
- **Command denied** — rerun with a different agent profile if appropriate, or answer the approval prompt in an interactive TTY.
- **Output is truncated** — narrow the command/search/read, or request a later `read` offset.
- **Wrong workspace** — set `OY_ROOT=/path/to/project`.

## Rust implementation notes

The Rust branch aims to preserve the Python CLI's high-level UX and routing names where useful, but implementation details and audit internals may differ. Contributor workflow, repo layout, crate map, and style rules live in [`CONTRIBUTING.md`](CONTRIBUTING.md).

## Development

```bash
cargo fmt
cargo check
cargo test
cargo run -- --help
```

## License

Apache License 2.0
