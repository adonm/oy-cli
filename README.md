# oy-cli

[![PyPI](https://img.shields.io/pypi/v/oy-cli)](https://pypi.org/project/oy-cli/)

Small local AI coding CLI for your shell. It can inspect files, search content, fetch public docs, and run commands in the current workspace.

`oy-cli` is intentionally OpenResponses-first: provider integrations are expected to support the [Open Responses](https://www.openresponses.org/) / OpenAI Responses API shape, and `oy` is optimized around that interface rather than chat-completions compatibility layers.

## Quick start

```bash
uv tool install oy-cli
oy "add docstrings to public functions"
oy chat
oy chat --agent plan
oy chat --continue-session
oy run --resume 20260325
oy audit "focus on authentication"
oy audit-logic "focus on authentication logic"
```

## Common tasks

```bash
oy "inspect the main module and suggest improvements"
OY_ROOT=./my-project oy "fix the failing tests"
echo "update the changelog" | OY_NON_INTERACTIVE=1 oy
oy chat
oy chat --agent plan
oy chat --agent accept-edits
oy chat --continue-session
oy run --resume 20260325 "finish the refactor"
oy audit [focus]
oy audit-logic [focus]
oy renovate-local
oy ralph "prompt"
oy model [filter]
oy --help
```

In chat, `/ask <question>` is research-only: no `bash`, no file changes, but public `webfetch` is still allowed. It is no-write rather than no-network.

## Audit workflow

`oy audit` now runs a resumable 3-phase repo audit automatically:

- phase1 `plan` — Python uses `sloc` plus `tiktoken` counts to build a SLOC-ranked 64k-token chunk plan
- phase2 `review` — Python reads each chunk, gives the model only that chunk plus the ASVS/grugbrain context, and exposes only `search` plus `replace` scoped to `ISSUES.md`
- phase3 `summary` — rewrite `ISSUES.md` so the 10-15 most important issues keep detail and the rest become concise

`oy audit-logic` is a stricter variant for reviewing actual software behaviour:

- phase1 skips docs and lockfiles from the review backlog
- phase2 builds chunk context from code and behaviour-shaping config, stripping comments and docstrings where possible before sending context to the model
- limited `search` inside audit review also defaults to excluding docs and lockfiles so the audit budget stays on executable logic, trust boundaries, authz/authn checks, state changes, parsing, persistence, and network behaviour
- phase3 keeps the final `ISSUES.md` summary focused on behavioural bugs and runtime-impacting configuration

Normal `oy audit` is still useful when docs, comments, or dependency metadata may matter. `oy audit-logic` is for concentrating hard on control flow and real runtime behaviour.

Each audit mode keeps its own resumable state file in the session dir so long-running audits can resume cleanly without colliding. After each chunk, Python verifies that `ISSUES.md` changed; if not, it retries with a smaller chunk instead of pretending review progress happened.

`.tmp/` is still used for things like `oy renovate-local` reports, but audit progress itself now lives in the session cache.

## Agent profiles

`oy` now supports simple built-in agent profiles inspired by familiar coding-agent modes:

- `default` — normal tool approvals
- `plan` — read-only exploration and planning
- `accept-edits` — auto-approves file edits, but not shell commands
- `auto-approve` — auto-approves all available tools

Examples:

```bash
oy chat --agent plan
oy run --agent accept-edits "rename the helper and update callers"
oy chat --agent auto-approve
```

## Session continuation

Saved chat sessions can now be resumed from the CLI:

- `oy chat --continue-session` — continue the most recent saved session
- `oy run --continue-session "next task"` — continue the most recent session, then run one prompt
- `oy run --resume <name-or-number> "next task"` — resume a specific saved session
- in chat, `/save [name]` and `/load [name]` still work

Saved sessions now keep both the transcript and the active agent profile.

## Design goals

- keep the codebase small and auditable
- expose a narrow built-in tool set
- keep provider support behind thin shims
- target providers that implement the Open Responses / OpenAI Responses API surface
- start fresh by default for one-shot runs
- make approvals and checkpoints explicit when they matter

Prompt text and tool descriptions live in [`oy_cli/session_text.toml`](oy_cli/session_text.toml). Core modules are [`oy_cli/runtime.py`](oy_cli/runtime.py), [`oy_cli/agent.py`](oy_cli/agent.py), [`oy_cli/cli.py`](oy_cli/cli.py), [`oy_cli/tools.py`](oy_cli/tools.py), and [`oy_cli/providers.py`](oy_cli/providers.py). Contributor workflow lives in [`CONTRIBUTING.md`](CONTRIBUTING.md).

## Configuration

### Environment variables

| Variable | Purpose |
|---|---|
| `OY_MODEL` | Override the model for this session (`model` or `shim:model`) |
| `OY_SHIM` | Force a shim when the model name is bare |
| `OY_NON_INTERACTIVE` | Set to `1` to disable approval and prompt pauses |
| `OY_UNATTENDED_LIMIT` | Agent deadline window, such as `1h`, `30m`, or `3600s` |
| `OY_RALPH_LIMIT` | Ralph deadline window, such as `3h`, `90m`, or `3600s` |
| `OY_ROOT` | Run against a different workspace |
| `OY_SYSTEM_FILE` | Append extra system instructions |
| `OY_CONFIG` | Override config path (default: `~/.config/oy/config.json`) |
| `OY_DEBUG` | Enable debug logging |
| `OY_YOLO` | Start with all tool approvals enabled |
| `OY_MAX_CONTEXT_TOKENS` | Override transcript and tool context budget |
| `OY_MAX_BASH_CMD_BYTES` | Override max accepted bash command size |

### Config file

```json
{"shim": "openai", "model": "glm-5"}
```

Only `model` and `shim` are persisted in config. Session continuation uses per-session files under `~/.config/oy/sessions/`. Selection order is `OY_MODEL`, then saved config, then the first-run picker. `OY_SHIM` only changes backend choice when the model name is bare or no model has been saved yet.

From local testing, `glm-5` and `kimi-k2.5` are good defaults.

## Installation

```bash
uv tool install oy-cli  # preferred
pip install oy-cli      # alternative
```

## Requirements

- Python 3.13+
- `bash`
- Provider credentials for a backend that supports the Open Responses / OpenAI Responses API shape (for example via OpenAI credentials, Codex auth, Copilot auth, OpenCode auth, or AWS credentials for Bedrock Mantle)

## Development

Use `uv` for local development. Contributor workflow lives in [`CONTRIBUTING.md`](CONTRIBUTING.md).

```bash
uv sync
uv run ruff check .
uv run pytest -q
uv run pytest tests/test_providers.py -q
uv run oy --help
uv build
```

## Authentication

OpenAI or other Open Responses-compatible endpoint:

```bash
export OPENAI_API_KEY=...
export OPENAI_BASE_URL=https://your-endpoint.example/v1  # optional
```

Copilot and Codex credentials are discovered automatically when available.

Bedrock Mantle:

```bash
export OY_SHIM=bedrock-mantle
export AWS_PROFILE=my-profile
export AWS_REGION=ap-southeast-2
```

`oy` loads models from `GET /models` and targets the Open Responses / OpenAI Responses API at `POST /responses`. Provider support in `oy` is intentionally centered on that API shape. Providers that do not support `/responses` fail with a clear error instead of falling back to legacy chat-completions behavior.


## Local model workflow

Run any OpenAI-compatible server on localhost. By default `oy` probes:

- `local-8080` at `http://127.0.0.1:8080/v1` (typical `llama-server` port)
- `local-11434` at `http://127.0.0.1:11434/v1` (typical Ollama port)

Examples:

```bash
OY_MODEL=local-8080:qwen3.5 oy chat
# or save it once:
oy model local-11434:qwen3.5
oy chat
```

You can also target any localhost port with the `local-<port>` shim form.

## Troubleshooting

- **Missing credentials** — start a local OpenAI-compatible server on `127.0.0.1:8080` or `127.0.0.1:11434`, set `OPENAI_API_KEY`, sign in with `codex`, authenticate `gh` for Copilot, run `opencode auth`, or configure AWS credentials / SSO for Bedrock Mantle.
- **stdin is not a TTY** — piping input disables `ask`; set `OY_NON_INTERACTIVE=1` to make that explicit.
- **AWS SSO session is stale** — run `aws sso login --use-device-code --no-browser`.

## Security

`oy` can run shell commands and modify files with your permissions. `bash` also inherits your environment, so git, cloud, and SSH credentials visible to your shell are visible to the command.

Recommended:

- run in a repo or workspace you trust
- mount only the directories you need in containers
- avoid exposing long-lived secrets in the environment
- use `/ask` when you want no-write research mode
- review generated changes before shipping

Protections include workspace-bound file tools, public-only `webfetch`, and default credential flows for supported providers. For provider authors, the intended compatibility target is Open Responses compliance rather than ad hoc OpenAI-compatible subsets. `oy` still acts with your user permissions, so treat generated shell commands and file edits as local code execution.

## License

Apache License 2.0
