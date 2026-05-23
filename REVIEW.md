# Code Quality Review

> Generated with [oy-cli](https://github.com/wagov-dtt/oy-cli): `OY_MODEL=github-copilot/gpt-5.5 oy review` · 2026-05-23


## Verdict

Needs work

## Findings summary

- **Medium** — Session turns are committed piecemeal, leaving partial saved state on failures: `src/agent/session.rs::run_prompt_with_policy`
- **Medium** — Provider/model routing metadata has multiple sources of truth: `src/llm/providers.rs`, `src/llm/route/resolve.rs::model_route`, `src/llm/providers/route.rs`, `src/agent/model/reasoning.rs`
- **Medium** — `oy review` duplicates the audit map/reduce pipeline instead of reusing it: `src/review.rs`, `src/audit.rs`, `src/audit/report.rs`
- **Medium** — Native LLM execution repeats the same tool loop across protocol paths: `src/llm/openai.rs::{run_gemini,run_anthropic_messages,run_bedrock_converse,run_chat_completions,run_responses}`
- **Medium** — Audit input skip rules exclude security-relevant source files by basename: `src/audit/input.rs::should_skip_path`
- **Medium** — Exact-file search greps the parent directory and post-filters results: `src/tools/workspace/search.rs::fff_search_target`

## Detailed findings

### Medium — Session turns are committed piecemeal

**Evidence:** `src/agent/session.rs::run_prompt_with_policy` pushes the user message into `session.transcript.messages` before later steps run: context-budget checks, tool context construction, route/auth resolution, model execution, todo updates, and assistant/tool transcript appends.

**Structural impact:** A failure after the initial push leaves the persisted session with a user prompt but no corresponding assistant/tool turn. Retrying can duplicate the prompt or continue from misleading state. If tool execution partially happened before a later failure, transcript state and external side effects can also diverge.

**Simplification:** Stage the whole turn locally: build a candidate transcript/tool context, run budget checks and model/tool execution against that staged state, then commit transcript and todos together on success. If failed tool turns need to be preserved, commit an explicit failed-turn/tool-error record rather than silently keeping only the user message.

### Medium — Provider/model routing metadata has multiple sources of truth

**Evidence:**

- `src/llm/providers.rs::PROVIDERS` defines provider family, auth envs, default URLs, and support state.
- `src/llm/route/resolve.rs::model_route` separately matches provider strings to `prepare_*` functions.
- `src/llm/providers/route.rs` separately hardcodes auth/env/base-url handling per provider.
- `src/agent/model/reasoning.rs::reasoning_effort_option` also owns provider/model capability quirks, including Moonshot/Kimi handling, OpenCode metadata lookup, and a static fallback list.
- Alias drift already exists: metadata lists `amazon-bedrock`, while routing accepts both `"bedrock" | "amazon-bedrock"`.

**Structural impact:** Adding or changing a provider requires coordinated edits across metadata, route dispatch, provider-specific builders, auth handling, and agent reasoning policy. Provider capability behavior is no longer locally reasoned about in the LLM routing layer.

**Simplification:** Make one provider descriptor the source of truth: canonical id, aliases, family, default URL, auth envs, support state, route builder, and model capability/reasoning policy. Resolve the descriptor once in `model_route`, then dispatch by descriptor/family. Keep `agent::model` passing user/env overrides rather than encoding provider-specific rules.

### Medium — `oy review` duplicates the audit map/reduce pipeline

**Evidence:** `src/review.rs::{run,sizing,prepare_workspace_input,compact_to_tokens,transparency_snippet,shell_quote,with_transparency_line}` mirrors orchestration and helpers from `src/audit.rs::{run,audit_constants}` and `src/audit/report.rs::{transparency_snippet,shell_quote,with_transparency_line}`. `review.rs` already depends on audit input helpers such as `collect_files`, `build_manifest`, `chunk_files`, and `ensure_chunks_fit_prompt`.

**Structural impact:** The no-tools workspace review flow now has two owners for sizing, chunk fan-out, reduce budgeting, transparency output, shell quoting, and output insertion. Fixes to deterministic map/reduce behavior can drift between `audit` and `review`.

**Simplification:** Extract a shared no-tools map/reduce runner parameterized by prompts, report title/source label, output format, and existing-report behavior. Leave `audit` and `review` as thin specializations. Move transparency-line insertion and shell quoting into the shared report helper.

### Medium — Native LLM execution repeats the same tool loop across protocol paths

**Evidence:** `src/llm/openai.rs::{run_gemini,run_anthropic_messages,run_bedrock_converse,run_chat_completions,run_responses}` each repeats the same control flow: build request body, stream assistant output, convert assistant state, return when no tool calls remain, enforce tool-round budget, record assistant turn, execute tool calls, and append tool results.

**Structural impact:** Tool-loop behavior is a cross-protocol invariant but must be fixed in five places. Drift is already visible in naming: `ensure_tool_round_budget` reports `native OpenAI {protocol} exceeded...` even for Gemini, Anthropic, and Bedrock protocol paths.

**Simplification:** Extract the shared loop around a small protocol adapter: `build_body`, `stream_step`, `append_assistant`, and `append_tool_result`. Keep protocol wire-format lowering local, but make budget enforcement, transcript updates, tool execution, and stop/return behavior single-owner.

### Medium — Audit input skip rules exclude security-relevant source files by basename

**Evidence:** `src/audit/input.rs::should_skip_path` applies `SKIP_FILENAME_SUBSTRINGS = ["credential", "secret", "token"]` to filenames globally. The same audit path scoring later prioritizes security concepts such as auth/token/secret.

**Structural impact:** The collector conflates likely secret artifacts with security-relevant source code. Files such as `token.rs`, `secret_manager.rs`, or `credential_store.go` can be silently omitted from audit input, making audit coverage hard to trust.

**Simplification:** Split the policy: skip known secret/config artifacts such as `.env*`, private keys, `.netrc`, and credential dotfiles, but do not skip recognized source extensions solely because the basename contains `token`, `secret`, or `credential`. If needed, redact suspicious literal contents inside source files instead.

### Medium — Exact-file search greps the parent directory and post-filters

**Evidence:** `src/tools/workspace/search.rs::fff_search_target` uses `target.parent()` as the grep base when `target.is_file()`, runs `picker.grep(... page_limit: limit)` across the parent directory, then filters matches by `exact_target` afterward.

**Structural impact:** Exact-file search is not enforced at the search boundary. Sibling file matches can consume the page limit before the target file is filtered in, so results for an exact file can be truncated or omitted depending on directory contents/index order.

**Simplification:** Add a direct file-search path for `target.is_file()` that scans only that file with the selected regex/literal mode. Keep `fff` grep for directory targets and delete the parent-base plus post-filter special case.
