# oy-cli

[![PyPI](https://img.shields.io/pypi/v/oy-cli)](https://pypi.org/project/oy-cli/)

**AI coding assistant for your shell.** Reads files, runs commands, and makes edits.

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

## System Prompts

The system prompt is short. Tool semantics live with the tool definitions; the system prompt focuses on operating rules and judgment:

### Base Prompt

```markdown
You are oy, a coding cli with tools.
Work by inspecting first, then making explicit changes. Prefer simple auditable solutions.
Keep going until done or blocked; if blocked, say what you tried and next steps.
Use grugbrain-style simplicity for complexity, OWASP-minded judgment for security, and performance-aware judgment.
Inspect with `search` for content and `list` for path discovery. Batch independent tool calls.
```

### Interactive Appendix

```markdown
Use ask only when clarification or direction is needed.
```

### Non-Interactive Appendix

```markdown
Non-interactive mode: do not pause for approval.
```

## Tools

Each tool description is passed directly to the model:

| Tool | Description |
|------|-------------|
| `list` | List paths by calling `Path.glob(path)`. Defaults to `path: "*"`. Use `src/*` or `src/**/*.py` exactly like pathlib glob patterns. Returns sorted entries, one per line, with / for directories. |
| `edit` | Edit files inside the workspace. Operations: replace, write, move, delete. `replace` supports literal replacement or regex replacement with `regex: true`. `replace` and `write` also accept 1-based `start_line` and `end_line` when targeting a file. |
| `bash` | Run shell commands for tests, builds, git, and scripts. Do not use for routine file inspection. Returns stdout and stderr together. |
| `search` | Search file contents with Python regex. Use this for content inspection. `path` limits where it searches and may be a file, directory, or pathlib-style glob. When `path` resolves to one file, `start_line` and `end_line` limit the search to that 1-based line range. |
| `httpx` | Fetch web pages or APIs over HTTP(S). Presets: page, json, post_json. Use json_path to extract nested fields. Sensitive headers are redacted. |
| `ask` | Ask the user a question in interactive runs. Use for ambiguity or decisions. Provide choices. |

**Output truncation:** tool output is clipped to preserve context window; `bash` keeps both head and tail. When clipped, narrow the next query or use `search` with a tighter `path` instead of re-running broad inspection.

**Conversation compaction:** interactive chat compresses prepared context with [Headroom](https://github.com/chopratejas/headroom) before each model request, then falls back to omitting the oldest messages if the transcript still does not fit.

**Parallel tool calls:** `oy` can execute multiple tool calls returned in a single assistant turn. Explicit provider flags for parallel tool calls are only sent where the upstream API supports them directly today; other providers rely on their native tool-calling behavior.

## Audit Command

`oy audit` runs the agent with a dedicated system prompt:

### Audit Prompt

```markdown
Audit the repo for security, unnecessary complexity, and major
performance issues, preserving project and human context.
First read key markdown docs, then refresh or generate an audit
header at the top of ISSUES.md that includes the current date,
the latest Git commit reference, and a codebase summary
using tools like `scc` or `tokei`. Next, fetch the current OWASP
ASVS (or MASVS if more relevant) and grugbrain.dev guidelines
using httpx, inspect the codebase against these, and write or
merge prioritised findings (max 10-15) into the ISSUES.md file.
Ensure each finding is formatted to include its location, category
(security, complexity, or performance), standard reference, a clear
recommendation, and has a Status, if existing findings have been
resolved, summarise and note them in a short log at the end.
```

```bash
oy audit                    # Full audit
oy audit "focus on auth"    # With focus area
OY_ROOT=./src oy audit      # Audit specific directory
```

## Configuration

**Environment variables:**

| Variable | Purpose |
|----------|---------|
| `OY_MODEL` | Override model for this session (bare name or `shim:model`) |
| `OY_SHIM` | Force a specific shim: `openai`, `codex`, `gemini`, `claude`, `copilot`, `bedrock`, or `bedrock-mantle` |
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

- Python 3.14+
- `bash`
- (Optional) `rg` (ripgrep) for faster search
- OpenAI API key or Codex local auth **OR** Gemini CLI OAuth credentials
  (`~/.gemini/oauth_creds.json`) **OR** Claude Code local auth **OR**
  AWS CLI configured for Bedrock

## Installation

```bash
uv tool install oy-cli  # Preferred
pip install oy-cli       # Alternative
```

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

Gemini, Claude, Copilot, and Codex (OpenAI) creds are introspected
and used, if creds are available `oy model` will show them in the model list.

**AWS Bedrock:** Uses your default AWS profile/region. Supports auto-refresh of stale SSO sessions.
```bash
export AWS_PROFILE=my-profile
export AWS_REGION=us-west-2
```

## Troubleshooting

**"Missing API credentials"** -> Set `OPENAI_API_KEY`, sign in with `codex`,
`gemini` or `claude`, or configure AWS CLI (`aws configure`). For Bedrock:
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

**Protections:** workspace-bound file access, structured edits
through `edit`, sensitive header redaction in `httpx`, and native boto3
credential resolution for Bedrock.

## Links

- [Issues](ISSUES.md) - Known issues and audit findings
- [Contributing](CONTRIBUTING.md) - Development and release notes

## License

Apache License 2.0
