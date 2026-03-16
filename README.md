# oy-cli

[![PyPI](https://img.shields.io/pypi/v/oy-cli)](https://pypi.org/project/oy-cli/)

**Tiny AI coding assistant for your shell.** Reads files, runs commands, makes precise edits, and stays intentionally small.

```bash
uv tool install oy-cli
oy "add docstrings to public functions"
```

## Examples

```bash
# Basic usage
oy "read the main module and suggest improvements"

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
oy model                  # Show current default model
oy model <filter>         # Pick/save a model
oy model --token          # Print Bedrock OpenAI env exports
oy --help                 # Show all commands
```

## Why This Exists

Most AI coding tools are large, complex, or lock you into a single provider. `oy` is deliberately small, easy to audit, and built around a narrow tool surface.

**Design goals:** small auditable codebase, minimal tool surface, OpenAI-completions-focused CLI loop, multiple backends behind shims, fresh session each run, and explicit checkpoints when needed.

## System Prompt

The system prompt is intentionally short. Tool semantics live with the tool definitions; the system prompt focuses on operating rules and judgment:

> You are oy, a tiny coding cli with tools.
> Work by inspecting first, then making explicit changes. Prefer simple auditable solutions.
> Keep going until done or genuinely blocked; if blocked, say what you tried and next steps.
> Use grugbrain-style simplicity for complexity, OWASP-minded judgment for security, and performance-aware judgment to avoid obvious waste.

In interactive mode, the `ask` tool is available: *"Use ask only when significant clarification or direction is needed."*

In non-interactive mode (`OY_NON_INTERACTIVE=1`): *"Non-interactive mode: do not pause for approval."*

## Tools

Each tool description is passed directly to the model. These are the exact descriptions:

| Tool | Description |
|------|-------------|
| `list` | List a directory. Use this first on unfamiliar trees. Returns sorted entries, one per line, with `/` for directories. |
| `read` | Read a file or directory. Use before editing. Files return line-numbered text; directories fall back to list. Use offset/limit for large files. |
| `apply` | Edit files inside the workspace. Operations: replace, write, move, delete. Read first and keep edits precise. |
| `bash` | Run shell commands for tests, builds, git, and scripts. Do not use for routine file inspection. Returns stdout and stderr together. |
| `grep` | Search file contents by text or regex. Use file_glob to narrow by filename pattern. Returns matching lines with file and line numbers. |
| `glob` | Find files by name pattern like `*.py` or `src/**/*.js`. Use when you know the path shape. Supports `*`, `?`, and `**`. |
| `httpx` | Fetch web pages or APIs over HTTP(S). Presets: page, json, post_json. Use json_path to extract nested fields. Sensitive headers are redacted. |
| `ask` | Ask the user a question in interactive runs. Use for significant ambiguity or decisions. Provide choices when useful. |

**Output truncation:** tool output is clipped to preserve context window; `bash` keeps both head and tail. When clipped, narrow the next query.

## Audit Command

`oy audit` runs the agent with a dedicated system prompt:

> Audit the repo for security, unnecessary complexity, and major obvious performance issues.
> Fetch current OWASP ASVS and MASVS with httpx, inspect the codebase, and write/merge prioritised findings to ISSUES.md.
> Each finding should include location, category (security|complexity|performance), reference, recommendation, and status: OPEN.
> Avoid removing project or human context.

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
| `OY_SHIM` | Force a specific shim: `openai`, `codex`, `gemini`, `claude`, `bedrock`, or `bedrock-mantle` |
| `OY_NON_INTERACTIVE` | Set to `1` to disable checkpoints |
| `OY_ROOT` | Run against different workspace |
| `OY_SYSTEM_FILE` | Append extra system instructions |
| `OY_CONFIG` | Override config path (default: `~/.config/oy/config.json`) |

**Tuning variables** (rarely needed):

| Variable | Default | Purpose |
|----------|---------|---------|
| `OY_MAX_TOOL_OUTPUT_TOKENS` | `4096` | Max tokens kept from tool output |
| `OY_MAX_TOOL_TAIL_TOKENS` | `1024` | Tail tokens preserved when output is clipped |
| `OY_MAX_BASH_CMD_BYTES` | `65536` | Max command size for `bash` tool |
| `OY_MAX_CONTEXT_TOKENS` | `131072` | Context window budget |
| `OY_MAX_MESSAGE_TOKENS` | `4096` | Per-message truncation limit |
| `OY_DEFAULT_MAX_STEPS` | `512` | Max LLM turns per run |
| `OY_DEFAULT_MAX_TOOL_CALLS` | `512` | Max tool invocations per run |
| `OY_DEFAULT_LINE_LIMIT` | `500` | Default line limit for `read`/`list`/`glob` |
| `OY_BEDROCK_READ_TIMEOUT` | `120` | HTTP read timeout for Bedrock (seconds) |
| `OY_BEDROCK_MAX_OUTPUT_TOKENS` | `4096` | Max output tokens for Bedrock Converse |

**Config file** (`~/.config/oy/config.json`):
```json
{"shim": "gemini", "model": "gemini-2.5-pro"}
```

The `shim` field pins which backend to use regardless of what else is signed in. Use `oy model <filter>` to pick interactively; it merges models from available signed-in shims into a single list using `shim:model` prefixes.

On first run, if no model is configured, `oy` prompts you to pick one from the available backends. Set `OY_MODEL`, `OY_SHIM`, or save a config with `oy model` to pin behavior.

**Recommended model:** From testing, `glm-5` (via Bedrock) offers the best balance of intelligence, cost, and tool-use ability. `kimi-k2.5` is another strong option.

## Requirements

- Python 3.14+
- `bash`
- (Optional) `rg` (ripgrep) for faster search
- OpenAI API key or Codex local auth **OR** Gemini CLI OAuth credentials (`~/.gemini/oauth_creds.json`) **OR** Claude Code local auth **OR** AWS CLI configured for Bedrock

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

**Codex CLI (automatic):** If you have already logged in with `codex`, `oy` will reuse the credentials in `~/.codex/auth.json`. If that file contains a generated API key, `oy` uses the public OpenAI Responses API. If it contains a ChatGPT account session instead, `oy` uses the ChatGPT-backed Codex responses endpoint.

**Gemini CLI (automatic):** If you have the [Gemini CLI](https://github.com/google-gemini/gemini-cli) installed and have run `gemini` at least once to authenticate, `oy` will pick up the OAuth credentials automatically from `~/.gemini/oauth_creds.json`. No extra setup needed.

```bash
# Install and authenticate the Gemini CLI once
npm install -g @google/gemini-cli
gemini  # follow the login prompt

# Then just use oy — it auto-detects Gemini credentials
oy "refactor this function"
```

Default model when using Gemini: `gemini-2.5-pro`. Use `oy model <filter>` or `OY_MODEL` to switch.

**Claude Code (automatic):** If you have already signed in with Claude Code, `oy` can use that local session directly through the `claude` shim. No Anthropic API key is required.

```bash
claude auth login
oy --model claude:sonnet "refactor this function"
```

**AWS Bedrock (automatic):** Uses your default AWS profile/region. Supports auto-refresh of stale SSO sessions.
```bash
export AWS_PROFILE=my-profile
export AWS_REGION=us-west-2
```

## Troubleshooting

**"Missing API credentials"** -> Set `OPENAI_API_KEY`, sign in with `codex`, install and authenticate the Gemini CLI, sign in with `claude auth login`, or configure AWS CLI (`aws configure`). For Bedrock: ensure your profile has `bedrock:InvokeModel` permission.

**"stdin is not a TTY"** -> Piping input disables `ask`. Set `OY_NON_INTERACTIVE=1` to make explicit.

**"AWS SSO session is stale"** -> Run `aws sso login --use-device-code --no-browser` or run `oy` in a TTY for auto-prompt.

**"command timed out"** -> `bash` default timeout is 120s. Agent can increase `timeout_seconds` parameter.

**"replace target not found"** -> `apply` requires exact string match. Read file first, check whitespace.

**Output truncated** -> Tools clip at 4096 tokens by default. Agent auto-narrows queries, or guide explicitly: "read lines 100-200".

## Security

`oy` can run shell commands and modify files with your permissions. Treat it like any other local automation tool.

Recommended:
- run in a repo or workspace you trust
- mount only needed directories in containers
- avoid exposing long-lived secrets in the environment
- review generated changes before shipping

**Automatic protections:** workspace-bound file access, structured edits through `apply`, sensitive header redaction in `httpx`, and secure Bedrock token generation.

## Links

- [Issues](ISSUES.md) - Known issues and audit findings
- [Contributing](CONTRIBUTING.md) - Development and release notes

## License

Apache License 2.0
