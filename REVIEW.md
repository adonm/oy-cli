# Code Quality Review

> Generated with [oy-cli](https://github.com/wagov-dtt/oy-cli): `OY_MODEL=github-copilot/gpt-5.5 oy review` · 2026-05-22


## Verdict

Block

The workspace has multiple structural maintainability issues that affect safety-sensitive boundaries: tool dispatch/effect classification, session safety policy, provider routing, workspace mutation, and report/finding generation. Several areas also show clear architectural drift through duplicated pipelines and near-god-file accumulation.

## Findings summary

| Severity | Finding | Code reference |
|---|---|---|
| Blocker | Tool registry is not the canonical tool definition; schema, dispatch, gating, and side-effect classification are split across stringly parallel structures. | `src/tools/registry.rs::TOOL_DEFS`, `src/tools.rs::invoke_inner`, `src/tools.rs::tool_may_have_external_side_effect` |
| Blocker | Session safety mode and enforced tool policy are duplicated sources of truth and can diverge on load. | `src/agent/session.rs::Session`, `src/agent/session/storage.rs::load_saved` |
| Blocker | Shared tool context uses clone-and-partial-merge state management, making future mutable state easy to lose or corrupt. | `src/tools.rs::invoke_shared` |
| Blocker | Multi-file patch application has a planning phase but no atomic write boundary; coordinated edits can partially apply. | `src/tools/workspace/patch.rs::tool_patch`, `src/cli/config/paths.rs::write_workspace_file` |
| Major | Provider-specific route options are owned by the wrong provider path. | `src/llm/providers/route.rs::prepare_openai_chat`, `src/llm/providers/route.rs::prepare_openrouter_chat` |
| Major | `agent::model` is a near-1k-line god file and its singleton cache breaks local reasoning about model-specific APIs. | `src/agent/model.rs` |
| Major | `review` duplicates the audit map/reduce pipeline instead of reusing the canonical runner. | `src/review.rs`, `src/audit.rs`, `src/audit/input.rs`, `src/audit/reduce.rs`, `src/audit/report.rs` |
| Major | Native LLM execution repeats the same tool-loop state machine per protocol. | `src/llm/openai.rs::{run_anthropic_messages, run_bedrock_converse, run_chat_completions, run_responses}` |
| Major | Audit/SARIF findings are derived from broad Markdown heading heuristics instead of a typed finding boundary. | `src/audit/report.rs::extract_findings`, `src/audit/report.rs::is_finding_heading`, `src/audit/sarif.rs::render_sarif` |
| Medium | Patch dialect parsing and mutation orchestration are growing in the same safety-sensitive sink. | `src/tools/workspace/patch.rs::tool_patch` |

## Detailed findings

### Blocker: Tool registry is not actually the canonical tool definition

**Evidence:** `src/tools/registry.rs::TOOL_DEFS` defines exposed tool metadata, but behavior remains split across `src/tools.rs::invoke_inner` and `src/tools.rs::tool_may_have_external_side_effect`.

This is a serious structural problem. The code appears to advertise a registry as the source of truth, but adding or changing a tool still requires multiple unrelated string-based edits:

- schema/exposure/gating in the registry,
- execution dispatch in `invoke_inner`,
- retry-boundary side-effect classification in `tool_may_have_external_side_effect`.

That is not a canonical registry; it is three parallel registries. The failure modes are not cosmetic. A tool can be exposed without a dispatcher, or a mutating/process/network tool can be added without being classified as externally side-effecting. That directly worsens local reasoning because reviewers must audit several distant match tables to understand one tool.

**Required restructuring:** collapse this into one `ToolDef`/`ToolId` model that owns the tool’s schema, gating, executor, preview behavior, and effect classification. Generate enabled tool specs, dispatch, and side-effect marking from that single definition. Delete the parallel stringly match tables.

---

### Blocker: Session mode and policy are duplicated sources of truth

**Evidence:** `src/agent/session.rs::Session` stores both `mode: SafetyMode` and `policy: ToolPolicy`. `src/agent/session/storage.rs::load_saved` overrides `mode` from the saved session but keeps the caller-provided `policy`.

This makes the safety invariant unrepresentable. A loaded session can display or persist one safety mode while enforcing a different tool policy. That is exactly the kind of boundary drift that should not exist in safety-sensitive code.

The maintainability issue is that every future caller must remember that `mode` and `policy` can diverge. The type shape says both are independent, while the product model appears to require “mode determines policy.” Once the invariant lives in caller discipline instead of the type system, every load/resume path becomes suspect.

**Required restructuring:** store only the effective `SafetyMode` on `Session` and derive `policy()` from it at the use site, or introduce a single `SafetyProfile { mode, policy }` constructed through one canonical constructor that cannot create inconsistent pairs. On load, compute the effective mode and policy together in one place.

---

### Blocker: Shared tool context uses clone-and-partial-merge state management

**Evidence:** `src/tools.rs::invoke_shared` clones `ToolContext`, runs the tool outside the lock, then merges back only selected fields such as `todos` and `external_side_effects`.

This pattern is a maintainability trap. It hides mutable-state ownership behind clone/merge behavior and requires every future mutable field to be manually added to the merge logic. Missing one field will silently discard updates. Concurrent tool calls also become harder to reason about because updates are reconstructed after execution rather than owned and applied through one clear state path.

The current design makes the state invariant implicit and fragile: some fields are live, some are copied, some are merged, and new fields can accidentally become stale snapshots.

**Required restructuring:** split immutable tool environment from mutable tool state. Then choose one explicit ownership model:

- serialize tool execution through a tool actor/queue,
- keep a `tokio::sync::Mutex<ToolState>` and mutate state through narrow APIs,
- or make tools return typed `ToolEffects` that are applied atomically under the lock.

The clone-and-partial-merge pattern should be removed.

---

### Blocker: Multi-file patch application plans atomically but writes sequentially

**Evidence:** `src/tools/workspace/patch.rs::tool_patch` builds a full plan and asks for approval, but then writes each planned file in sequence using `config::write_workspace_file`. `src/cli/config/paths.rs::write_workspace_file` opens the destination with truncate/write semantics.

The code has the shape of a transaction during planning, but not during mutation. A coordinated multi-file patch can partially apply if a later write fails after earlier files have already been truncated and rewritten. That is a structural mismatch: the approval and planning layer treats the change as one unit, while the write layer commits it as unrelated individual operations.

This worsens local reasoning for every caller that assumes “the patch applied” means the entire approved patch applied.

**Required restructuring:** introduce a shared `WorkspaceWriteBatch` sink. It should prevalidate all destinations, write temporary files beside targets, preserve permissions where needed, then rename into place. On failure, originals should remain untouched or be restored from backups. Use this same batch writer for patch and any other multi-file replace/edit operation.

---

### Major: Provider-specific options leak into the wrong route

**Evidence:** `src/llm/providers/route.rs::prepare_openai_chat` reads `OPENROUTER_PROVIDER_OPTIONS` into `additional_params`, while `prepare_openrouter_chat` sets `additional_params: None`.

Provider policy is owned by the wrong route. OpenRouter-specific request-body configuration is attached to OpenAI preparation, and the OpenRouter route does not own the option. That makes provider behavior non-local: debugging OpenRouter request shape requires auditing an OpenAI builder, and OpenAI requests can inherit an OpenRouter-specific body shape.

**Required restructuring:** move `OPENROUTER_PROVIDER_OPTIONS` parsing into `prepare_openrouter_chat`. `prepare_openai_chat` should only read OpenAI-owned knobs. If provider-specific environment handling needs structure, use small provider-local option helpers such as `OpenRouterOptions` and `OpenAiOptions`; do not add cross-provider conditionals.

---

### Major: `agent::model` is a near-1k-line god file with misleading model-specific cache APIs

**Evidence:** `src/agent/model.rs` is about 910 lines and combines model selection, OpenCode listing, provider information, a global model-info cache, chat execution, reasoning-effort logic, unit tests, and ignored live integration tests. The candidate evidence notes that `model_limits(_model_spec)` ignores its argument and `provider_info(model_spec)` can return the last cached provider regardless of the requested model.

This file is already near the 1,000-line decomposition threshold and is accumulating unrelated responsibilities. More importantly, the API shape implies model-specific lookup, while the cache behavior is singleton-like. That breaks local reasoning: callers pass a `model_spec` and reasonably expect the returned provider/limits to correspond to that model.

**Required restructuring:** split the module before it crosses the threshold:

- `model/selection.rs`
- `model/info_cache.rs`
- `model/reasoning.rs`
- `model/executor.rs`
- production-independent live/integration tests outside the production module

Then either key cached model info by canonical model spec or remove `model_spec` parameters from APIs that are not actually model-specific. The type/API boundary should make cache semantics obvious.

---

### Major: `review` duplicates the audit map/reduce pipeline

**Evidence:** `src/review.rs` reimplements workspace sizing, input collection/chunk conversion, diff chunking, parallel chunk review, reduce prompt compaction, transparency snippets, shell quoting, and report title insertion. Similar machinery already exists under `src/audit.rs`, `src/audit/input.rs`, `src/audit/reduce.rs`, and `src/audit/report.rs`.

This is architectural drift. There are now two no-tools review pipelines that must evolve in lockstep. Fixes to token budgeting, chunk failure handling, transparency output, prompt compaction, or report post-processing can land in one path and not the other.

**Required restructuring:** extract a shared deterministic `NoToolsReviewPipeline` with pluggable pieces:

- `InputSource`
- `PromptSet`
- `ReportConfig`

Keep audit/security-specific prompts in `audit` and maintainability-review prompts in `review`, but reuse sizing, chunk execution, reduce compaction, transparency rendering, and output writing.

---

### Major: Native LLM execution repeats the same tool-loop state machine per protocol

**Evidence:** `src/llm/openai.rs::{run_anthropic_messages, run_bedrock_converse, run_chat_completions, run_responses}` each perform the same orchestration pattern: build a request body, stream an assistant step, convert assistant output, detect tool calls, enforce the tool-round budget, record loop state, execute tools, append tool results, and repeat.

Protocol-specific wire formats are legitimate. Duplicating the turn/tool-loop state machine is not. Every tool-loop fix now has to be patched across several protocol paths, which invites drift in retry behavior, transcript updates, tool-budget enforcement, and tool-result appending.

**Required restructuring:** extract one shared `run_tool_loop` over a small protocol driver interface:

- `build_body`
- `stream_step`
- `assistant_to_message`
- `append_assistant`
- `append_tool_result`

Keep wire-format lowering protocol-local. Centralize turn orchestration and tool execution once.

---

### Major: Audit/SARIF finding extraction is based on broad Markdown heuristics

**Evidence:** `src/audit/report.rs::extract_findings` and `is_finding_heading` treat many `##`–`####` headings as possible findings unless they match a small negative list. `src/audit/sarif.rs::render_sarif` depends on that prose parser.

This is a weak boundary for a structured artifact. Adding report sections or subheadings like “Evidence,” “Impact,” or “Remediation” can produce bogus findings, and every formatting exception grows the exclusion list. SARIF generation should not depend on broad Markdown heading guesses.

**Required restructuring:** introduce a typed finding model:

```rust
Finding {
    severity,
    title,
    code_ref,
    body,
}
```

Render Markdown and SARIF from that model. If model output must remain Markdown, enforce an explicit positive grammar such as `### [Severity] Title` plus a required `Evidence:` or code-reference line. Reject non-conforming headings instead of maintaining a negative heuristic list.

---

### Medium: Patch dialect support is growing inside the mutation sink

**Evidence:** `src/tools/workspace/patch.rs::tool_patch` branches on `is_apply_patch_format` and also contains unified/git diff parsing, custom `*** Begin Patch` parsing, context-hunk application, path resolution, approval, diff rendering, and writes.

This concentrates parser dialects, planning, approval, and filesystem mutation in one busy safety-sensitive path. Future patch-format fixes will add more branchy coupling around the write sink.

**Required restructuring:** normalize patch input before mutation planning. Split dialect parsing into modules such as:

- `patch/dialect_unified.rs`
- `patch/dialect_apply_patch.rs`

Both should lower into one internal representation, for example `PatchPlanInput { path, hunks }`. Keep `tool_patch` focused on approve → batch-write, with path safety and atomic writes handled by shared helpers.
