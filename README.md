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
- transcript compaction text (`Current todo list`, omitted-history note, packed-history note)
- built-in tool descriptions exposed to the model

Code that reads this content lives in [`oy_cli/session_text.py`](oy_cli/session_text.py).
Runtime composition lives mainly in [`oy_cli/modes.py`](oy_cli/modes.py), [`oy_cli/agent.py`](oy_cli/agent.py), and [`oy_cli/tooling/core.py`](oy_cli/tooling/core.py).

## Configuration

**Environment variables:**

| Variable | Purpose |
|----------|---------|
| `OY_MODEL` | Override model for this session (bare name or `shim:model`) |
| `OY_SHIM` | Force a specific shim: `openai`, `codex`, `copilot`, `bedrock`, or `bedrock-mantle` |
| `OY_NON_INTERACTIVE` | Set to `1` to disable checkpoints |
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
- OpenAI API key or Codex local auth **OR**
  AWS CLI configured for Bedrock

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

**AWS Bedrock:** Uses your default AWS profile/region. Supports auto-refresh of stale SSO sessions.
```bash
export AWS_PROFILE=my-profile
export AWS_REGION=us-west-2
```

## Troubleshooting

**"Missing API credentials"** -> Set `OPENAI_API_KEY`, sign in with `codex`,
or configure AWS CLI (`aws configure`). For Bedrock:
ensure your profile has `bedrock:InvokeModel` permission.

**"stdin is not a TTY"** -> Piping input disables `ask`. Set `OY_NON_INTERACTIVE=1` to make explicit.

**"AWS SSO session is stale"** -> Run `aws sso login --use-device-code --no-browser`.

## Security

`oy` can run shell commands and modify files with your permissions. Treat it like any other local automation tool.

Recommended:
- run in a repo or workspace you trust
- mount only needed directories in containers
- avoid exposing long-lived secrets in the environment
- review generated changes before shipping

**Protections:** workspace-bound file access for built-in file tools and native boto3 credential resolution for Bedrock.

## Links

- [Issues](ISSUES.md) - Known issues and audit findings
- [Contributing](CONTRIBUTING.md) - Development and release notes

## License

Apache License 2.0
