# oy-cli

[![PyPI](https://img.shields.io/pypi/v/oy-cli)](https://pypi.org/project/oy-cli/)

Small local AI coding CLI for your shell. It reads files, searches content, fetches public docs, and runs commands in the current workspace.

## Quick start

```bash
uv tool install oy-cli
oy "add docstrings to public functions"
oy chat
oy audit "focus on authentication"
```

## Use

```bash
oy "inspect the main module and suggest improvements"
OY_ROOT=./my-project oy "fix the failing tests"
echo "update the changelog" | OY_NON_INTERACTIVE=1 oy
OY_BEST_OF=5 oy "fix the flaky test"
oy chat
oy audit [focus]
oy ralph "prompt"
oy model [filter]
oy --help
```

In chat, `/ask <question>` is research-only and no-write: no `bash`, no file changes, but public `webfetch` is still allowed.

## Design goals

- keep the codebase small and auditable
- expose a narrow built-in tool set
- keep provider support behind thin shims
- start fresh by default for one-shot runs
- make approvals and checkpoints explicit when they matter

Model-facing prompt text and tool descriptions live in [`oy_cli/session_text.toml`](oy_cli/session_text.toml). Core modules are [`oy_cli/runtime.py`](oy_cli/runtime.py), [`oy_cli/agent.py`](oy_cli/agent.py), [`oy_cli/cli.py`](oy_cli/cli.py), [`oy_cli/tools.py`](oy_cli/tools.py), and [`oy_cli/providers.py`](oy_cli/providers.py). Contributor workflow lives in [`CONTRIBUTING.md`](CONTRIBUTING.md).

## Configuration

### Environment variables

| Variable | Purpose |
|---|---|
| `OY_MODEL` | Override model for this session (`model` or `shim:model`) |
| `OY_SHIM` | Force a shim: `openai`, `codex`, `copilot`, `opencode`, `opencode-go`, or `bedrock-mantle` |
| `OY_NON_INTERACTIVE` | Set to `1` to disable approval/checkpoint pauses |
| `OY_UNATTENDED_LIMIT` | Agent deadline window, like `1h`, `30m`, or `3600s` |
| `OY_RALPH_LIMIT` | Ralph deadline window, like `3h`, `90m`, or `3600s` |
| `OY_BEST_OF` | Override self-consistency sample count |
| `OY_ROOT` | Run against a different workspace |
| `OY_SYSTEM_FILE` | Append extra system instructions |
| `OY_CONFIG` | Override config path (default: `~/.config/oy/config.json`) |
| `OY_DEBUG` | Enable debug logging |
| `OY_YOLO` | Start with all tool approvals enabled |
| `OY_MAX_CONTEXT_TOKENS` | Override transcript/tool context budget |
| `OY_MAX_BASH_CMD_BYTES` | Override max accepted bash command size |

### Config file

```json
{"shim": "openai", "model": "glm-5"}
```

Only `model` and `shim` are persisted. Selection order is `OY_MODEL`, then saved config, then the first-run picker. `OY_SHIM` only changes backend choice when the model name is bare or no model has been saved yet.

From local testing, `glm-5` and `kimi-k2.5` are good defaults. `oy` uses best-of `3` for those models by default; override with `--best-of` or `OY_BEST_OF`.

## Requirements

- Python 3.13+
- `bash`
- OpenAI-compatible credentials, Codex auth, Copilot auth, OpenCode auth, or AWS credentials for Bedrock Mantle

## Installation

```bash
uv tool install oy-cli  # preferred
pip install oy-cli      # alternative
```

## Development

Use `uv` for local development. Contributor workflow lives in [`CONTRIBUTING.md`](CONTRIBUTING.md).

```bash
uv sync
uv run ruff check .
uv run pytest -q
uv run oy --help
uv build
```

## Authentication

OpenAI or compatible endpoint:

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

`oy` loads models from `GET /models` and sends chat requests to `POST /chat/completions` on the Mantle endpoint.

## Troubleshooting

- **Missing credentials** — set `OPENAI_API_KEY`, sign in with `codex`, authenticate `gh` for Copilot, run `opencode auth`, or configure AWS credentials / SSO for Bedrock Mantle.
- **stdin is not a TTY** — piping input disables `ask`; set `OY_NON_INTERACTIVE=1` to make that explicit.
- **AWS SSO session is stale** — run `aws sso login --use-device-code --no-browser`.

## Security

`oy` can run shell commands and modify files with your permissions. `bash` also inherits your environment, so git/cloud/SSH credentials visible to your shell are visible to the command.

Recommended:

- run in a repo or workspace you trust
- mount only needed directories in containers
- avoid exposing long-lived secrets in the environment
- use `/ask` when you want no-write research mode
- review generated changes before shipping

Protections include workspace-bound file tools, public-only `webfetch`, and default credential flows for supported providers. `oy` still acts with your user permissions, so treat generated shell commands and file edits as local code execution.

## License

Apache License 2.0
