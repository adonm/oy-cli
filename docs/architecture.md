# Architecture

`oy` is a local AI coding CLI. It runs from a workspace, sends prompts and selected context to a configured model provider, and exposes explicit tools for workspace reads, edits, shell commands, public web fetches, and in-memory planning.

## Runtime flow

```text
user argv/stdin
  -> src/main.rs
  -> oy::run in src/lib.rs
  -> cli::app in src/cli.rs
  -> session in src/agent.rs
  -> model provider / tool loop
  -> src/tools.rs for local capabilities
```

1. `src/main.rs` converts process errors into a user-facing exit code.
2. `src/lib.rs` keeps the public crate surface small.
3. `cli::app` parses commands, resolves the workspace and safety mode, then starts `run`, `chat`, `model`, `doctor`, or `audit`.
4. `agent::session` owns transcripts, context budgeting, compaction, tool-call loops, and saved sessions.
5. `agent::model` and `agent::bedrock` resolve provider-specific clients and model routing.
6. `src/tools.rs` validates tool arguments and enforces approval, workspace, network, and mutation boundaries.
7. `src/audit.rs` is separate from the tool loop: it collects repository text first, then sends no-tools audit prompts to the model.

## Main modules

| Path | Responsibility |
|---|---|
| `src/agent.rs` | Provider integration, model selection, Bedrock support, sessions, transcripts, context compaction, tool loop |
| `src/audit.rs` | Deterministic no-tools audit collection, chunking, prompt construction, report writing |
| `src/cli.rs` | Config paths, safety modes, terminal UI, interactive chat shell, command handlers |
| `src/tools.rs` | Tool schemas, tool dispatch, previews, todos, workspace filesystem boundary, webfetch boundary, mutation approval |
| `src/lib.rs` | Small public facade used by the binary and tests |
| `src/main.rs` | Tokio entry point and process exit handling |
| `tests/snapshots.rs` | Snapshot tests for chat help and tool preview UX |

The current layout intentionally keeps only a few top-level Rust files. If a file is split later, prefer mechanical extraction with no behavior changes and keep `src/lib.rs` stable.

## Trust boundaries

| Boundary | Entry point | Sink | Required posture |
|---|---|---|---|
| Workspace files | Tool paths, `OY_ROOT`, audit collector, `--out` | Reads/writes under the workspace | Validate near path resolution; fail closed outside workspace; test symlinks and traversal |
| Shell/process | `bash` tool, provider helper CLIs | User shell/process environment | Ask or deny by mode; avoid implicit process execution; add timeouts |
| Network | `webfetch`, model providers, routing shims | HTTP requests and provider APIs | Separate model egress from tool egress; validate public-only fetches strictly |
| Model provider | Prompts, snippets, tool output, audit chunks | External or local model endpoint | Treat sent text as disclosed to that provider; avoid secrets by default |
| Local state | Config, sessions, history, `TODO.md` persistence | `~/.config/oy-rust/` and workspace files | Store only intentionally; use private local files; document sensitivity |
| Approval | Tool call from model | File writes and shell commands | Default-deny where non-interactive or read-only; preview before asking |

## Modes and policies

Safety modes are defined in `cli::config::SafetyMode` and converted to `tools::ToolPolicy`:

- `default` / `ask`: read tools are available; file writes and shell ask.
- `plan` / `read`: no file writes or shell; intended for first looks and untrusted repos.
- `accept-edits` / `edit`: file writes auto-approve; shell still asks.
- `auto-approve` / `auto`: file writes and shell auto-approve for trusted unattended work.
- `/ask`: research-only chat submode.

When changing capabilities, update `README.md`, `SECURITY.md`, and tests that assert tool exposure.

## Audit pipeline

`oy audit` does not expose tools to the model. It:

1. resolves the workspace and output path,
2. collects reviewable text files with skip rules and size caps,
3. builds a manifest and security index,
4. chunks large repositories,
5. runs one no-tools model prompt for small repos or map/reduce prompts for large repos,
6. inserts a transparency line and findings summary,
7. writes the report to the requested workspace path.

Audit still sends collected repository text to the configured model provider. Keep skip/redaction behavior conservative and document any override that increases disclosure.
