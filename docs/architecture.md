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
  -> llm::{LlmRequest, Message, ToolSpec, ModelRoute, ChatBackend, NativeOpenAiBackend, LlmTool}
  -> tools::{registry, native LLM tool adapter}
  -> model provider / native tool loop
  -> src/tools.rs and src/tools/ for local capabilities
```

1. `src/main.rs` converts process errors into a user-facing exit code.
2. `src/lib.rs` keeps the public crate surface small.
3. `cli::app` parses commands, restores legacy argument shapes, and delegates command bodies to `cli::app/*_cmd.rs`.
4. `cli::config` is a facade over focused config modules for modes, paths, prompts, model config, environment knobs, and saved sessions.
5. `agent::session` owns session orchestration and saved sessions; transcript storage uses `llm::Message`, while context compaction, provider chat/retry logic, auth status, and endpoint discovery live in sibling `agent/` modules.
6. `agent::model` resolves a small chat route, builds an `llm::LlmRequest` from `oy`-owned messages/tool specs, and executes it through `llm::NativeOpenAiBackend`. OpenAI direct routes, GitHub Copilot API-token routes, and OpenAI-compatible providers listed by OpenCode use only Chat Completions or Responses protocols. `agent::opencode_models` remains the only source of OpenCode verbose model metadata; do not add local provider/model registries. Any provider-routing shim must be narrow, metadata-backed, and covered by focused tests.
7. `agent::auth` owns environment/OpenCode/Copilot API-token credential lookup; callers should not duplicate provider auth probing.
8. `src/tools.rs` and `src/tools/` validate tool arguments and enforce approval, workspace, network, and mutation boundaries. `src/tools/registry.rs` is the single `oy` tool schema registry; `src/tools/llm.rs` adapts enabled tools to the native `llm::LlmTool` trait.
10. `src/audit.rs` is separate from the tool loop: it orchestrates collection, chunk review/reduce, rendering, and report writing through focused `src/audit/` modules.

## Main modules

| Path | Responsibility |
|---|---|
| `src/agent.rs`, `src/agent/` | Model selection, auth discovery, OpenCode model metadata, sessions, `llm::Message` transcripts, context compaction, chat/tool loop orchestration |
| `src/llm/` | `oy`-owned LLM request/response, message, tool-spec, route, backend seam, and native OpenAI-compatible transport/tool loop |
| `src/audit.rs`, `src/audit/` | Deterministic no-tools audit orchestration, input collection, chunking, prompt construction, report/SARIF writing |
| `src/cli.rs`, `src/cli/` | Command parsing/dispatch, command handlers, config paths, safety modes, terminal UI, interactive chat shell |
| `src/tools.rs`, `src/tools/` | Tool schema registry, native LLM tool adapter, tool dispatch, previews, todos, workspace filesystem boundary, webfetch boundary, mutation approval |
| `src/lib.rs` | Small public facade used by the binary and tests |
| `src/main.rs` | Tokio entry point and process exit handling |
| `tests/snapshots.rs` | Snapshot tests for chat help and tool preview UX |

The current layout keeps top-level Rust files as facades/orchestrators where practical. When splitting files, prefer mechanical extraction with no behavior changes, keep trust-boundary validation near entry points, and keep `src/lib.rs` stable.

## LLM transition target

The desired LLM boundary is OpenCode-shaped but `oy`-sized:

```text
transcript/tools -> LlmRequest -> ModelRoute -> Protocol -> Transport -> LlmResponse
                                      ^                         |
                                      |                         v
                              OpenCode metadata             tool loop
```

Months 1 through 6 are in place: `src/llm/mod.rs` owns request/response, message, tool-spec, route, backend-trait, and native tool types; transcripts store `llm::Message`; `agent::model` accepts `oy` messages directly; `src/tools/registry.rs` is the single tool schema registry; `src/tools/llm.rs` adapts enabled tools to `llm::LlmTool`; and `src/llm/openai.rs` is the default non-streaming OpenAI Chat/Responses transport and hardened tool loop. The native loop returns recoverable `TOOL_ERROR`/`RECOVERY` text for tool failures, hints enabled tools for unknown names, blocks repeated identical failed calls, caps model-visible tool output before it enters the next provider request, stops tool-only churn before the broader tool-round budget is exhausted, and shares tool-round budget checks across Chat/Responses. Prompt-level provider retries use a small jittered backoff and stop once `tools::invoke_inner` records a write, shell, or persistent todo side-effect attempt.

Rules for that transition:

- own `LlmRequest`, `LlmResponse`, messages, tool definitions, and model routes in `oy`;
- do not reintroduce provider adapters without a concrete user need;
- keep native OpenAI Chat and OpenAI Responses as the only model wire protocols unless a concrete user need justifies more;
- keep provider metadata in OpenCode, auth in `agent::auth`, and policy checks in `tools`;
- prefer request/response golden tests over broad live-provider tests.

See `CONTRIBUTING.md` for the month-by-month roadmap.

## Trust boundaries

| Boundary | Entry point | Sink | Required posture |
|---|---|---|---|
| Workspace files | Tool paths, `OY_ROOT`, audit collector, `--out` | Reads/writes under the workspace | Validate near path resolution; fail closed outside workspace; test symlinks and traversal |
| Shell/process | `bash` tool, provider helper CLIs | User shell/process environment | Ask or deny by mode; filter credential-like child env vars; avoid implicit process execution; add timeouts |
| Network | `webfetch`, model providers, routing shims | HTTP requests and provider APIs | Separate model egress from tool egress; validate public-only fetches strictly |
| Model provider | Prompts, snippets, tool output, audit chunks | External or local model endpoint | Treat sent text as disclosed to that provider; avoid secrets by default |
| Local state | Config, sessions, history, `TODO.md` persistence | `~/.config/oy-rust/` and workspace files | Store only intentionally; use private local files; document sensitivity |
| Approval | Tool call from model | File writes and shell commands | Default-deny where non-interactive or read-only; preview before asking |
| Retry | Provider transient failure after tool execution | Replayed prompt/tool loop | Retry only before external side-effect attempts; fail closed after write/shell/persistent todo attempts |

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
