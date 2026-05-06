# Architecture

`oy` is a local AI coding CLI. It runs from a workspace, sends prompts and selected context to a configured model provider, and exposes explicit tools for workspace reads, edits, shell commands, public web fetches, and in-memory planning.

## Runtime flow

```text
user argv/stdin
  -> src/main.rs
  -> oy::run in src/lib.rs
  -> cli::app in src/cli/app.rs
  -> cli::app::{session_cmd, model_cmd, doctor_cmd, audit_cmd}
  -> agent::session in src/agent/session.rs
  -> agent::{chat, transcript, compaction, model, auth, opencode_models}
  -> model provider / tool loop
  -> src/tools.rs and src/tools/ for local capabilities
```

1. `src/main.rs` converts process errors into a user-facing exit code.
2. `src/lib.rs` keeps the public crate surface small.
3. `cli::app` parses commands, restores legacy argument shapes, and delegates command bodies to `cli::app/*_cmd.rs`.
4. `cli::config` is a facade over focused config modules for modes, paths, prompts, model config, environment knobs, and saved sessions.
5. `agent::session` owns session orchestration and saved sessions; transcript storage, context compaction, provider chat/retry logic, auth status, and endpoint discovery live in sibling `agent/` modules.
6. `agent::model` resolves a small chat route, then uses Rig clients for execution. `agent::opencode_models` is the only source of OpenCode verbose model metadata; do not add local provider/model registries. The only accepted provider-routing shim is the narrow Copilot `/responses` workaround for Rig versions that route only Codex models correctly.
7. `agent::auth` owns environment/OpenCode/GitHub credential lookup; callers should not duplicate provider auth probing.
8. `agent::bedrock` contains AWS-specific client/auth integration.
9. `src/tools.rs` and `src/tools/` validate tool arguments and enforce approval, workspace, network, and mutation boundaries.
10. `src/audit.rs` is separate from the tool loop: it orchestrates collection, chunk review/reduce, rendering, and report writing through focused `src/audit/` modules.

## Main modules

| Path | Responsibility |
|---|---|
| `src/agent.rs`, `src/agent/` | Provider integration, model selection, auth discovery, OpenCode model metadata, Bedrock support, sessions, transcripts, context compaction, chat/tool loop |
| `src/audit.rs`, `src/audit/` | Deterministic no-tools audit orchestration, input collection, chunking, prompt construction, report/SARIF writing |
| `src/cli.rs`, `src/cli/` | Command parsing/dispatch, command handlers, config paths, safety modes, terminal UI, interactive chat shell |
| `src/tools.rs`, `src/tools/` | Tool schemas, tool dispatch, previews, todos, workspace filesystem boundary, webfetch boundary, mutation approval |
| `src/lib.rs` | Small public facade used by the binary and tests |
| `src/main.rs` | Tokio entry point and process exit handling |
| `tests/snapshots.rs` | Snapshot tests for chat help and tool preview UX |

The current layout keeps top-level Rust files as facades/orchestrators where practical. When splitting files, prefer mechanical extraction with no behavior changes, keep trust-boundary validation near entry points, and keep `src/lib.rs` stable.

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
