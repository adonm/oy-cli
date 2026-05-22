# Code Quality Review

> Generated with [oy-cli](https://github.com/wagov-dtt/oy-cli): `OY_MODEL=github-copilot/gpt-5.5 oy review` · 2026-05-22


## Verdict

Block

Current triage after fixes: provider-support enforcement now lives in route resolution, Google/Gemini OpenCode entries are filtered from routable listings until their native protocol is ported, and `agent::model` has been split into a small facade plus execution, metadata-cache, reasoning, and test modules. The remaining review items are larger structural refactors around shared no-tools analysis, native protocol tool-loop extraction, patch normalization, typed LLM message role/content boundaries, tool argument/schema drift, and audit input decomposition.

## Findings summary

| Severity | Finding | Code reference |
|---|---|---|
| Fixed | Provider support gating lived in cache/metadata paths and was bypassable by normal routing/execution; route resolution now fails closed before constructing a route, and metadata caching is best-effort only. | `src/llm/route/resolve.rs::model_route`, `src/agent/model/metadata.rs::cache_model_limits`, `src/agent/opencode_models.rs::OpenCodeModel::is_openai_compatible_api` |
| Fixed | `src/agent/model.rs` crossed the 1,000-line threshold and mixed selection, metadata, execution, reasoning, and tests; it is now an 85-line facade over focused submodules. | `src/agent/model.rs`, `src/agent/model/exec.rs`, `src/agent/model/metadata.rs`, `src/agent/model/reasoning.rs`, `src/agent/model/tests.rs` |
| Major | `audit` and `review` are forking the same no-tools map/reduce review workflow instead of sharing a canonical pipeline. | `src/review.rs`, `src/audit.rs`, `src/audit/reduce.rs`, `src/audit/report.rs`, `src/audit/input`, `src/cli/app/audit_cmd.rs`, `src/cli/app/review_cmd.rs` |
| Major | Native protocol execution repeats the same tool-loop orchestration in multiple provider/protocol functions. | `src/llm/openai.rs::run_chat_completions`, `run_responses`, `run_anthropic_messages`, `run_bedrock_converse` |
| Major | Patch handling has two parser/application paths with duplicated safety-sensitive validation. | `src/tools/workspace/patch.rs::plan_patch`, `parse_patch_set`, `plan_apply_patch`, `parse_apply_patch`, `apply_context_hunks` |
| Major | LLM message role/content invariants are not represented in types, forcing protocol lowerers to rediscover invalid states. | `src/llm/mod.rs::Message`, `src/llm/protocols/openai_chat.rs::append_user_content`, `src/llm/protocols/shared.rs::assistant_parts` |
| Major | Tool argument schema and serde deserialization are independent contracts and can drift. | `src/tools/schema.rs`, `src/tools/args.rs` |
| Major | Audit input collection mixes skip policy, prioritization, indexing, manifesting, language detection, and chunking in one module. | `src/audit/input.rs` |

## Detailed findings

### 1. Fixed: Provider support gating is enforced by route resolution

**Severity:** Fixed

**Evidence:**

- `src/llm/route/resolve.rs::model_route` now calls a provider-support check before constructing `ModelRoute`.
- `src/agent/model/metadata.rs::cache_model_limits` no longer rejects unsupported providers; it only populates best-effort limit/provider metadata.
- `src/agent/opencode_models.rs::OpenCodeModel::is_openai_compatible_api` no longer treats `@ai-sdk/google` as routable, so Google/Gemini entries stay out of `oy model` listings until the native protocol is implemented.
- `src/agent/model/tests.rs::prepare_chat_rejects_unsupported_provider_before_auth_lookup` covers the fail-closed route boundary.
- `src/agent/opencode_models.rs::filters_google_models_until_native_protocol_is_supported` covers listing filtering.

The invariant is now local: if route resolution returns a route, the provider passed the supported-provider policy. Metadata/cache helpers are not execution gates.

---

### 2. Fixed: `src/agent/model.rs` is now a small facade

**Severity:** Fixed

**Evidence:**

- `src/agent/model.rs` is now 85 lines and owns only selection/listing facade behavior and public re-exports.
- `src/agent/model/exec.rs` owns `exec_chat` and route/request handoff.
- `src/agent/model/metadata.rs` owns model-limit/provider metadata caching.
- `src/agent/model/reasoning.rs` owns thinking/reasoning-effort policy.
- `src/agent/model/tests.rs` holds the former inline unit/live tests outside the production facade.

This removes the immediate 1,000-line blocker while preserving the public `agent::model` API used by sessions, audit, review, and CLI commands.

---

### 3. `audit` and `review` are forking the same no-tools map/reduce pipeline

**Severity:** Major

**Evidence:**

- `src/review.rs` contains its own orchestration: `run`, sizing, workspace/diff input preparation, token compaction, transparency-line insertion, shell quoting, and prompt construction.
- Similar pipeline concepts already exist under `src/audit.rs`, `src/audit/reduce.rs`, `src/audit/report.rs`, and `src/audit/input`.
- `src/cli/app/audit_cmd.rs::audit_command` and `src/cli/app/review_cmd.rs::review_command` are near-parallel command bodies: resolve root/model/focus/output, print no-tools prelude, run pipeline, emit JSON or success text.

This is the same architectural workflow twice: collect input, size/chunk it, map to candidate findings, reduce into final output, decorate/report, and write results.

**Why this hurts maintainability:**

- Chunking limits, retry behavior, truncation, progress output, JSON shape, and report decoration can drift.
- Fixes to prompt budgeting or compaction must be copied between systems.
- The command layer duplicates orchestration that should be policy-agnostic.
- `review` partially reuses audit input but not the rest of the pipeline, creating an awkward half-shared design.

**Required restructuring:**

Extract one shared no-tools analysis runner, for example:

```rust
AnalysisSpec {
    name,
    default_output_path,
    input_source,
    map_prompt_builder,
    reduce_prompt_builder,
    report_renderer,
    count_labels,
}
```

The shared runner should own:

- sizing and chunk/reduce budgeting,
- candidate-finding compaction,
- progress/no-tools prelude behavior,
- transparency/report decoration helpers,
- output writing and JSON/text emission shape where possible.

Keep audit/review prompts and report semantics separate. Do not keep two orchestration engines.

---

### 4. Native protocol execution has four near-identical tool loops

**Severity:** Major

**Evidence:**

Duplicated orchestration appears in:

- `src/llm/openai.rs::run_chat_completions`
- `src/llm/openai.rs::run_responses`
- `src/llm/openai.rs::run_anthropic_messages`
- `src/llm/openai.rs::run_bedrock_converse`

Each path appears to perform the same lifecycle: build protocol body, stream assistant step with retry, convert assistant output, detect tool calls, enforce round budget, update `ToolLoopState`, execute local tools, append tool results, and continue.

**Why this hurts maintainability:**

- Retry, round-budget, transcript preservation, and side-effect handling must be fixed in every protocol path.
- Protocol-specific lowering is mixed with canonical tool-loop control flow.
- Adding another protocol will likely copy the same loop again.
- Subtle behavior drift between providers is likely and hard to review.

**Required restructuring:**

Keep protocol serialization/deserialization explicit, but extract the shared control flow into one driver.

A small adapter boundary is enough:

```rust
trait ProtocolToolLoop {
    fn build_body(...);
    async fn stream_step(...);
    fn assistant_message(...);
    fn append_assistant(...);
    fn append_tool_result(...);
}
```

The shared driver should own:

- retry behavior,
- round-budget enforcement,
- `ToolLoopState`,
- local tool execution,
- transcript assembly,
- common stop/continue decisions.

Provider/protocol modules should only describe wire-format lowering and parsing.

---

### 5. Patch handling duplicates parsers and safety/application paths

**Severity:** Major

**Evidence:**

`src/tools/workspace/patch.rs` contains separate flows around:

- `plan_patch`
- `parse_patch_set`
- `plan_apply_patch`
- `parse_apply_patch`
- `apply_context_hunks`

The module supports both diffy unified/git patches and a bespoke `*** Begin Patch` format. The candidate flows duplicate path resolution, symlink checks, file-size checks, UTF-8/binary checks, duplicate-file checks, diff rendering, and plan construction.

**Why this hurts maintainability:**

Patch application is safety-sensitive. Duplicating validation across parser/application branches makes it easy for one format to bypass a check or diverge on edge cases. Embedding a custom mini-language parser inside the write sink also increases the amount of policy that future contributors must understand before changing patch behavior.

**Required restructuring:**

Normalize all patch formats into one typed internal representation before validation/application.

Preferred shape:

```rust
enum PatchFormat {
    Unified,
    Git,
    LegacyBeginPatch,
}

struct ParsedPatchFile { ... }

struct PatchPlan { ... }
```

Then run one shared pipeline:

1. Parse format-specific input into `ParsedPatchFile`.
2. Resolve paths once.
3. Run symlink/size/binary/UTF-8/duplicate-file validation once.
4. Render preview/diff once.
5. Apply/commit through one path.

If `*** Begin Patch` is only compatibility sugar, consider deleting it and requiring unified/git diffs. If it must stay, isolate it as a parser only; it should not own separate safety semantics.

---

### 6. Boundary contracts are too loose or duplicated

**Severity:** Major

Two related boundary issues should be cleaned up before they keep spreading.

#### LLM message types allow invalid role/content states

`src/llm/mod.rs::Message` uses a shared `Vec<MessageContent>` for both user and assistant messages. Invalid combinations are rejected later:

- `src/llm/protocols/openai_chat.rs::append_user_content` rejects user `ToolCall` / `Reasoning`.
- `src/llm/protocols/shared.rs::assistant_parts` rejects assistant `ToolResult`.

That means role invariants are not encoded where messages are constructed. Every protocol lowerer must remember to defend against illegal combinations.

**Remedy:** split content by role:

```rust
enum UserContent {
    Text(...),
    ToolResult(...),
    Opaque(...),
}

enum AssistantContent {
    Text(...),
    ToolCall(...),
    Reasoning(...),
    Opaque(...),
}
```

Keep serde/transcript compatibility at the boundary if needed, but make protocol lowering consume role-valid structures.

#### Tool schema and argument parsing are two sources of truth

`src/tools/schema.rs` hand-builds model-visible schemas/defaults/enums, while `src/tools/args.rs` separately defines serde structs/defaults/aliases. Defaults, enum values, numeric leniency, and aliases are duplicated.

Examples called out in the candidates:

- `DEFAULT_LIMIT` and numeric string handling appear in both schema construction and custom deserializers.
- Search/replace modes exist as serde enums and schema `enum_values`.
- `todo` aliases are documented in schema and reimplemented in `TodoArgs::deserialize`.

**Remedy:** each tool should own one local argument contract. Either collocate the `Args` type and schema next to the implementation, or extract small shared constants for defaults/enums/aliases used by both schema and serde. Avoid a magical generic schema generator; the needed fix is to remove central catalogue drift, not add another abstraction layer.

---

### 7. Audit input collection is carrying too many policies

**Severity:** Major

**Evidence:**

`src/audit/input.rs` contains collection, skip rules, security-priority scoring, security-index keyword scanning, manifest rendering, chunking, language detection, and path normalization.

The policy lists are already conceptually at odds: candidates note that `SKIP_FILENAME_SUBSTRINGS` skips filenames containing `"token"`, while `security_path_score` prioritizes paths containing `"token"`.

**Why this hurts maintainability:**

- Disclosure policy and prioritization policy are entangled in one collector.
- Special cases will accumulate as ad-hoc string lists.
- Security-relevant files can be silently omitted by one rule while another rule says they are important.
- Chunking and manifest formatting changes require touching collection policy.

**Required restructuring:**

Split the collector into focused modules:

- `skip_policy`
- `classify` / `priority`
- `security_index`
- `manifest`
- `chunking`
- `language`

Use a single path-classification API:

```rust
enum PathClassification {
    Skip { reason: SkipReason },
    Review { language, priority, security_tags },
}
```

Add tests for skip-vs-priority edge cases, especially `token`, `auth`, `secret`, and config filenames.
