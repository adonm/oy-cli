# oy

Small local AI coding CLI for your shell. It can inspect files, search content, fetch public docs, run commands, and edit files inside the current workspace.

## Quick start

```bash
oy doctor                         # check setup and see the next step
oy model                          # list detected/recommended model choices
oy "inspect this repo and summarize the main risks"
oy chat                           # interactive mode
oy chat --mode plan               # read-only mode for untrusted repos
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
| `oy doctor` | Check model/auth/local-state setup and safety-relevant defaults |
| `oy audit [focus]` | Deterministic no-tools repository audit; writes `ISSUES.md` by default |
| `oy --help` | Show CLI help |

## Common tasks

```bash
oy "inspect the main module and suggest improvements"
oy audit "security, complexity, and performance"   # writes ISSUES.md
oy run --out docs/plan.md "write a migration plan"
oy chat --continue-session
oy run --resume 20260325 "finish the refactor"
OY_ROOT=./my-project oy "fix the failing tests"
echo "update the changelog" | OY_NON_INTERACTIVE=1 oy run
```

## Audit

`oy audit [focus]` runs a deterministic no-tools repository audit and writes `ISSUES.md` by default. The runner, not the model, collects reviewable workspace text, builds a repository manifest and security-relevant index, then sends the included text to the model as either one full review or a simple map→reduce chunk review for larger repositories.

The audit prompt embeds an OWASP ASVS/MASVS checklist plus grugbrain complexity guidance, and asks for concise, evidence-first findings with severity, file/symbol evidence, trust boundary/sink where security-relevant, impact, exploitability/preconditions, references, and fixes. Generated reports include a transparency line showing the command/model context used, a succinct all-findings summary with code refs, and detailed writeups for only the most severe 10-20 findings.

```bash
oy audit                         # writes ISSUES.md
oy audit "security and complexity"
oy audit "auth paths" --out docs/audit.md
```

`oy audit` intentionally does not expose `read`, `search`, `webfetch`, `bash`, or file-edit tools to the model. This keeps the audit simpler and makes the reviewed input deterministic. Use `oy chat --mode plan` when you want an exploratory read-only agent instead.

## Model setup

Start with `oy doctor` and `oy model`; both show recommendations based on detected credentials. You can also save an exact model id directly, for example `oy model copilot::gpt-4.1-mini` or `oy model local-8080::qwen3.5`.

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

AWS Bedrock Converse:

```bash
aws configure sso        # one-time setup, or use normal AWS env credentials
export AWS_PROFILE=my-sso
export AWS_REGION=ap-southeast-2
oy model bedrock::global.amazon.nova-2-lite-v1:0
```

For SSO profiles, `oy` uses the AWS SDK default credential chain and runs `aws sso login [--profile ...]` when SDK credential loading reports expired/missing SSO credentials. Non-interactive runs fail closed and tell you to run `aws sso login` yourself.

Bedrock Mantle (OpenAI-compatible API):

```bash
export AWS_BEARER_TOKEN_BEDROCK=...  # or BEDROCK_MANTLE_API_KEY
export AWS_REGION=us-east-1          # base URL defaults from AWS/BEDROCK region
oy model bedrock-mantle::moonshotai.kimi-k2.5
```

Bedrock Mantle requires Bedrock-specific bearer credentials; `OPENAI_API_KEY` and `OPENAI_BASE_URL` are not used for Mantle discovery or requests.

OpenCode Zen / Go (OpenAI-compatible APIs):

```bash
opencode auth login                  # or export OPENCODE_API_KEY=...
oy model opencode::gpt-5.1-codex-max
oy model opencode-go::kimi-k2.5
```

Local OpenAI-compatible server:

```bash
oy model local-8080::qwen3.5
oy chat
```

Local endpoint auth uses `LOCAL_API_KEY` when set and otherwise sends a placeholder `oy-local` bearer token; it does not reuse `OPENAI_API_KEY` for localhost probes.

## Chat UX

- Enter sends
- Alt+Enter or Shift+Enter inserts a newline
- pasted multiline text stays editable before submit
- `/help` lists commands
- `/status` shows model, workspace, approvals, context, and todos
- short aliases include `/q`, `/h`, `/m`, `/t`, `/u`
- for 3+ step work, oy keeps an in-memory todo; `TODO.md` is written only when explicitly requested and allowed by the current mode

`/ask <question>` is research-only: no `bash`, no file changes, but public `webfetch` is still allowed.

## Safety and approval modes

`oy` is not a sandbox. It can run shell commands and modify files with your permissions. `bash` inherits your environment, so git, cloud, SSH, package registry, and Docker credentials visible to your shell may be visible to approved commands. Model providers may receive prompts, source snippets, tool output, and command output.

For untrusted repositories, start read-only and contained:

```bash
oy chat --mode plan
docker run --rm -it -v "$PWD:/workspace:ro" -w /workspace oy-image oy chat --mode plan
docker run --rm -it -v "$PWD:/workspace:rw" -w /workspace -e OPENAI_API_KEY oy-image oy chat
```

Avoid mounting the host Docker socket into AI-assisted containers; it is usually host-root-equivalent. Avoid `auto-approve` unless the workspace, model, and requested task are trusted.

| Mode | File edits | Bash | Notes |
|---|---:|---:|---|
| `default` (`ask`) | asks | asks | Normal balanced mode |
| `plan` (`read`) | unavailable | unavailable | Read-only exploration and planning |
| `accept-edits` (`edit`, `write`) | auto | asks | Useful for trusted mechanical edits |
| `auto-approve` (`auto`) | auto | auto | Highest-risk unattended mode; file tools remain workspace-bound |
| `/ask` | unavailable | unavailable | No-write research; public `webfetch` allowed |

Non-interactive mode cannot pause for approval or questions. Use explicit modes/env only in workspaces and automation contexts you trust.

Protections include workspace-bound file tools (even in auto modes), public-only `webfetch`, read-only modes, explicit approval modes, no-tools deterministic audits, and clamped terminal previews.

## Tool round budget

A tool round is one model response that requests one or more tool calls. Long coding runs commonly use hundreds of rounds because the model should inspect, edit, and verify in small steps.

Default: `512` tool rounds per prompt. If that is too low for a trusted long-running task, set:

```bash
OY_MAX_TOOL_ROUNDS=2048 oy "finish the migration"
OY_MAX_TOOL_ROUNDS=unlimited oy "large trusted cleanup"
```

`OY_MAX_TOOL_ROUNDS=0`, `unlimited`, `none`, and `off` all mean unlimited. Use unlimited only when the workspace, model, and command are trusted; approval policy, repeated no-op detection, deterministic context compaction, provider limits, and normal process interruption still apply, but an unlimited run can consume substantial time/tokens.

## Preview and truncation behavior

Tool output shown in the terminal is a preview, not always the full tool result. Tool progress is grouped by model round and kept dense:

```text
↻ tools r2 ×3
  → read · path=src/main.rs
  ✓ read 12ms · path=src/main.rs · lines 1-40/80
```

Text previews use bat-like defaults: a small title bar, line numbers, a gutter, and color when the terminal supports it. `OY_COLOR=always` forces ANSI color, while `OY_COLOR=never`, `OY_COLOR=off`, or `NO_COLOR` disables it.

- `read` returns line slices rendered with file name and source line numbers; use `offset` and `limit` to fetch more. Long lines are clamped to the preview width and tabs are expanded to stable columns so gutters stay aligned.
- `search`, `list`, and `replace` previews show bounded item lists and say when more exist.
- `bash` and `webfetch` summarize long output before it goes back to the model and render stdout/stderr/body snippets as numbered blocks.
- Preview lines are clamped so terminal output remains scannable instead of wrapping into misleading indentation.

## Model ids and routing shims

`oy model`:

- shows the current configured model id, active routing shim, and recommended next choices
- introspects relevant auth env vars and auto-populates `GITHUB_TOKEN` from `gh auth token` when missing
- sends direct `GET /models` requests to configured OpenAI-compatible endpoints
- includes built-in model hints as selectable choices even when endpoint introspection is unavailable
- in a TTY, can prompt for choosing and saving a model

Use exact model ids in config. Endpoint-qualified choices such as `copilot::gpt-4.1-mini`, `bedrock::global.amazon.nova-2-lite-v1:0`, `bedrock-mantle::moonshotai.kimi-k2.5`, `opencode::gpt-5.1-codex-max`, `opencode-go::kimi-k2.5`, or `local-8080::qwen3.5` infer routing.

## Sessions and local files

Saved chat sessions can be resumed in the same workspace they were saved from:

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
| `OY_SHIM` | Override routing shim (`openai`, `copilot`, `bedrock-mantle`, `opencode`, `opencode-go`, `local-<port>`) |
| `OY_NON_INTERACTIVE` | Disable interactive approval/question pauses |
| `OY_ROOT` | Run against a different workspace |
| `OY_SYSTEM_FILE` | Append extra system instructions |
| `OY_CONFIG` | Override config path |
| `OY_COLOR` | `auto`, `always`, or `never`; boolean aliases accepted; `NO_COLOR` disables color |
| `OY_MAX_BASH_CMD_BYTES` | Override max accepted bash command size |
| `OY_MAX_TOOL_ROUNDS` | Tool-round budget per prompt; default `512`; set a number, `0`, or `unlimited` |
| `OY_CONTEXT_LIMIT` | Approximate context-token limit; deterministic compaction starts near 80% |
| `OY_COMPACT_RECENT_MESSAGES` | Recent transcript messages to keep during deterministic compaction |
| `OPENAI_API_KEY`, `OPENAI_BASE_URL` | OpenAI/OpenAIResp auth and optional OpenAI-compatible endpoint override; not reused for other routing shims |
| `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`, `AWS_PROFILE`, `AWS_REGION`, `AWS_DEFAULT_REGION` | AWS Bedrock auth/region; SSO profiles use AWS CLI export/login |
| `BEDROCK_REGION`, `BEDROCK_RUNTIME_ENDPOINT` | Override Bedrock region/runtime endpoint |
| `AWS_BEARER_TOKEN_BEDROCK`, `BEDROCK_MANTLE_API_KEY`, `BEDROCK_MANTLE_BASE_URL` | Bedrock Mantle OpenAI-compatible auth/endpoint |
| `OPENCODE_API_KEY`, `OPENCODE_BASE_URL`, `OPENCODE_GO_BASE_URL` | OpenCode Zen/Go auth/endpoint; falls back to `~/.local/share/opencode/auth.json` |
| `COPILOT_GITHUB_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN` | Copilot shim auth |
| `LOCAL_API_KEY` | Local `local-<port>` shim key; falls back to placeholder `oy-local` |

## Troubleshooting

- No model configured: run `oy model copilot::gpt-4.1-mini`, set `OPENAI_API_KEY` and run `oy model gpt-4.1-mini`, or use `oy model local-8080::qwen3.5`.
- Tool denied: switch mode only if trusted, for example `oy chat --mode accept-edits` for automatic file edits.
- Model call failed: check `oy model`, provider credentials, routing shim, and local server availability.
- Untrusted repo: use `oy chat --mode plan` first, preferably inside a container or VM.

## Development

Top-level source layout is intentionally coarse-grained after the 0.7.7 maintainability pass:

| Path | Role |
|---|---|
| `src/agent.rs` | Model routing, Bedrock/OpenAI-compatible adapters, session state, transcript compaction |
| `src/cli.rs` | CLI commands, configuration, terminal UI, interactive chat shell |
| `src/tools.rs` | Workspace-bound tools, schemas, previews, approval and filesystem/network boundaries |
| `src/audit.rs` | Deterministic no-tools audit runner and audit prompts |
| `src/lib.rs`, `src/main.rs` | Public facade and binary entry point |

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run -- --help
```

## License

Apache License 2.0
