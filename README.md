# oy-cli

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
oy audit                  # Security audit against OWASP ASVS/MSVS
oy models                 # Interactive model picker
oy model                  # Show current default model
oy --help                 # Show all commands
```

## Why This Exists

Most AI coding tools are large, complex, or lock you into a single provider. `oy` is deliberately small, easy to audit, and built around a narrow tool surface.

**Design goals:** small auditable codebase, minimal tool surface, OpenAI-completions-focused CLI loop, multiple backends behind shims, fresh session each run, and explicit checkpoints when needed.

**Built-in guidance:**
- complexity: grugbrain.dev style simplicity over abstraction
- security: OWASP-minded defaults and review
- performance: performance-aware programming; measure first, avoid obvious waste

## Tools

| Tool | Purpose | Best Use |
|------|---------|----------|
| `list` | List directory contents | First pass on unfamiliar trees |
| `read` | Read files or directories | Primary inspection tool; always before editing |
| `apply` | Modify files | Exact replacements, writes, moves, deletes |
| `grep` | Search file contents | Find code by text or regex |
| `glob` | Find files by pattern | Find paths when you know the filename shape |
| `bash` | Run shell commands | Tests, builds, git, scripts |
| `httpx` | Fetch web/API content | Docs, standards, and API responses |
| `ask` | Ask the user questions | Interactive approvals and checkpoints |

**Behavioral rules:**
- Inspect before changing.
- Prefer `list`/`read`/`grep`/`glob` over shelling out for inspection.
- Use `apply` for all file edits.
- Keep edits targeted and batch related operations.
- If output is clipped, narrow the query instead of guessing.

## Agent Behavior

**Core workflow:** inspect first, use the narrowest useful tool, make precise edits, and keep going until done or genuinely blocked.

**Prompt style:** the embedded system prompts are intentionally short. Tool semantics live with the tool definitions, while the system prompt focuses on operating rules and judgment.

**Reasoning defaults:**
- prefer grugbrain.dev style simplicity to reduce complexity
- use OWASP framing for security-sensitive work
- apply performance-aware programming judgment from Computer Enhance: measure before tuning, but avoid obvious waste

**Interactive mode:** use checkpoints for plans, ambiguous choices, and risky changes.

**Non-interactive mode:** do not pause for approval; recover from failures when possible and stop only with a concise blocked status.

**Output truncation:** tool output is clipped to preserve context; `bash` keeps both head and tail. When that happens, narrow the next query.

## Audit Command

Fetches current OWASP ASVS/MASVS standards, explores the repository, identifies security issues, complexity problems, and major obvious performance issues, then writes findings to `ISSUES.md`.

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
| `OY_SHIM` | Force a specific shim: `openai`, `codex`, `gemini`, `claude`, or `bedrock` |
| `OY_NON_INTERACTIVE` | Set to `1` to disable checkpoints |
| `OY_ROOT` | Run against different workspace |
| `OY_SYSTEM_FILE` | Append extra system instructions |
| `OY_CONFIG` | Override config path (default: `~/.config/oy/config.json`) |

**Config file** (`~/.config/oy/config.json`):
```json
{"shim": "gemini", "model": "gemini-2.5-pro"}
```

The `shim` field pins which backend to use regardless of what else is signed in. Use `oy models` to pick interactively; it merges models from available signed-in shims into a single list using `shim:model` prefixes.

By default, `oy` prefers a sensible available model automatically. If multiple providers are available, set `OY_MODEL`, `OY_SHIM`, or save a config to pin behavior.

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

Default model when using Gemini: `gemini-2.5-pro`. Use `oy models` or `OY_MODEL` to switch.

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

**Output truncated** -> Tools clip at ~16k chars. Agent auto-narrows queries, or guide explicitly: "read lines 100-200".

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
