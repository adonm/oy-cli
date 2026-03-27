# oy-cli

[![PyPI](https://img.shields.io/pypi/v/oy-cli)](https://pypi.org/project/oy-cli/)

**AI coding assistant for your shell.** Reads files, searches content, and runs commands.

```bash
uv tool install oy-cli
oy "add docstrings to public functions"
```

## Examples

```bash
# Basic usage
oy "inspect the main module and suggest improvements"

# Work in a specific directory
OY_ROOT=./my-project oy "fix the failing tests"

# Non-interactive mode (CI/pipelines)
echo "update the changelog" | OY_NON_INTERACTIVE=1 oy

# Security audit
oy audit
oy audit "focus on authentication"
```

## Commands

```bash
oy "prompt"              # Run with a prompt (default)
oy chat                   # Interactive multi-turn session
oy audit                  # Security audit against OWASP ASVS/MASVS
oy model                  # Show current model, pick model from available endpoints
oy --help                 # Show all commands
```

## Why This Exists

`oy` is small, auditable, and built around a narrow tool surface.

**Design goals:** small auditable codebase, minimal tool surface,
OpenAI-completions-focused CLI loop, multiple backends behind shims,
new session each run, and explicit checkpoints when needed.

## Session Text and Prompts

All text that is sent as part of model sessions lives in [`oy_cli/session_text.toml`](oy_cli/session_text.toml).

That includes:

- base system prompt text
- interactive/non-interactive prompt suffixes
- audit prompt text
- research-only `/ask` suffix
- transcript compaction text (`Current todo list`, omitted-history note, TOON packed-history note)
- built-in tool descriptions exposed to the model

Code that reads and composes this content now lives mainly in [`oy_cli/runtime.py`](oy_cli/runtime.py), with transcript/agent flow in [`oy_cli/agent.py`](oy_cli/agent.py) and CLI entrypoints in [`oy_cli/cli.py`](oy_cli/cli.py).

## Configuration

**Environment variables:**

| Variable | Purpose |
|----------|---------|
| `OY_MODEL` | Override model for this session (bare name or `shim:model`) |
| `OY_SHIM` | Force a specific shim: `openai`, `codex`, `copilot`, `opencode`, `opencode-go`, or `bedrock-mantle` |
| `OY_NON_INTERACTIVE` | Set to `1` to disable approval/checkpoint pauses |
| `OY_ROOT` | Run against different workspace |
| `OY_SYSTEM_FILE` | Append extra system instructions |
| `OY_CONFIG` | Override config path (default: `~/.config/oy/config.json`) |

**Config file** (`~/.config/oy/config.json`):
```json
{"shim": "openai", "model": "glm-5"}
```

The `shim` field pins which backend to use regardless of what else is signed in.
Use `oy model <filter>` to pick interactively; it merges models from available
signed-in shims into a single list using `shim:model` prefixes.

On first run, if no model is configured, `oy` prompts you to pick one from
the available backends. Set `OY_MODEL`, `OY_SHIM`, or save a config with
`oy model` to pin behavior.

**Model notes:** From testing, `glm-5` balances intelligence,
cost, and tool-use ability. `kimi-k2.5` is another option.
The [Artificial Analysis Comparison of Open Source Models](https://artificialanalysis.ai/models/open-source)
is a reference.

## Requirements

- Python 3.13+
- `bash`
- OpenAI API key or compatible endpoint credentials, Codex local auth, Copilot auth, OpenCode auth, or AWS CLI configured for Bedrock Mantle

## Installation

```bash
uv tool install oy-cli  # Preferred
pip install oy-cli       # Alternative
```

## Development

For local development, linting, tests, and builds, use `uv`.
Do not run bare `pytest`, `ruff`, or `pip install -e .` commands in this repo.

```bash
uv sync
uv run ruff format .
uv run ruff check .
uv run python -m pytest tests/ -v
uv run oy --help
uv build
```

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the contributor workflow.

## Authentication

**OpenAI:**
```bash
export OPENAI_API_KEY=sk-...
```

For OpenAI-compatible endpoints:
```bash
export OPENAI_BASE_URL=https://your-endpoint.example/v1
export OPENAI_API_KEY=...
```

Copilot and Codex (OpenAI) creds are introspected
and used, if creds are available `oy model` will show them in the model list.

**AWS Bedrock Mantle:** `oy` uses the Bedrock Mantle OpenAI-compatible endpoint (`https://bedrock-mantle.<region>.api.aws/v1`) and signs requests directly with SigV4 service `bedrock-mantle`.

```bash
export OY_SHIM=bedrock-mantle
export AWS_PROFILE=my-profile
export AWS_REGION=ap-southeast-2
```

`oy` loads models from `GET /models` on the Mantle endpoint and sends chat requests to `POST /chat/completions` on the same endpoint.

## Troubleshooting

**"Missing API credentials"** -> Set `OPENAI_API_KEY`, sign in with `codex`, authenticate `gh` for Copilot, run `opencode auth`, or for Bedrock Mantle configure AWS credentials / SSO and set `AWS_REGION`.

**"stdin is not a TTY"** -> Piping input disables `ask`. Set `OY_NON_INTERACTIVE=1` to make explicit.

**"AWS SSO session is stale"** -> Run `aws sso login --use-device-code --no-browser`.

## Security

`oy` can run shell commands and modify files with your permissions. Treat it like any other local automation tool.

Recommended:
- run in a repo or workspace you trust
- mount only needed directories in containers
- avoid exposing long-lived secrets in the environment
- review generated changes before shipping

**Protections:** workspace-bound file access for built-in file tools and default SDK credential flows for supported providers.

## Links

- [Issues](ISSUES.md) - Known issues and audit findings
- [Contributing](CONTRIBUTING.md) - Development and release notes

## License

Apache License 2.0
