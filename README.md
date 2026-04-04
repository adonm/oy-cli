# oy-cli

[![PyPI](https://img.shields.io/pypi/v/oy-cli)](https://pypi.org/project/oy-cli/)

Small local AI coding CLI for your shell. It can inspect files, search content, fetch public docs, and run commands in the current workspace.

`oy-cli` is intentionally OpenResponses-first: provider integrations are expected to support the [Open Responses](https://www.openresponses.org/) / OpenAI Responses API shape, and `oy` is optimized around that interface rather than chat-completions compatibility layers.

## Quick start

```bash
uv tool install oy-cli
oy "add docstrings to public functions"
oy chat
oy audit "focus on authentication"
```

## Common tasks

```bash
oy "inspect the main module and suggest improvements"
OY_ROOT=./my-project oy "fix the failing tests"
echo "update the changelog" | OY_NON_INTERACTIVE=1 oy
oy chat
oy audit [focus]
oy ralph "prompt"
oy model [filter]
oy --help
```

In chat, `/ask <question>` is research-only: no `bash`, no file changes, but public `webfetch` is still allowed. It is no-write rather than no-network.

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

Only `model` and `shim` are persisted. Selection order is `OY_MODEL`, then saved config, then the first-run picker. `OY_SHIM` only changes backend choice when the model name is bare or no model has been saved yet.

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

## Troubleshooting

- **Missing credentials** — set `OPENAI_API_KEY`, sign in with `codex`, authenticate `gh` for Copilot, run `opencode auth`, or configure AWS credentials / SSO for Bedrock Mantle.
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
