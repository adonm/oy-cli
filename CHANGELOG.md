# Changelog

## [0.11.0] - 2026-06-06

### Changed
- `oy` now delegates AI workflows to opencode. The default `oy` command installs/updates global integration files and launches `opencode --agent oy`.
- Convenience commands (`oy run`, `oy chat`, `oy model`, `oy audit`, `oy review`, `oy enhance`) wrap generated opencode commands; unknown top-level `oy` args pass through to opencode.
- Audit/review/enhance workflows now live in generated agents, skills, and commands.
- `oy run`, `oy chat`, and default `oy` map old safety modes to generated primary agents: `oy`, `oy-plan`, `oy-edit`, and `oy-auto`.
- Generated agents now emit short phase markers during longer non-interactive work.
- `sloc` now shells out to `tokei` when available instead of embedding the `tokei` crate; the tool is omitted from MCP listings when `tokei` is not on `PATH`.
- `outline` now shells out to Universal Ctags when available instead of embedding tree-sitter grammars; the tool is omitted from MCP listings when Universal Ctags is not on `PATH`.
- `oy doctor` now reports optional `tokei`/Universal Ctags availability and prints `mise`/Homebrew install hints when they are missing.
- Refreshed Cargo.lock to current Rust 1.96-compatible dependency releases.

### Added
- `oy setup` writes `~/.config/opencode/opencode.json`, agents, and skills. Use `oy setup --workspace` for project-local `.opencode` setup.
- `oy mcp` starts a local stdio MCP server exposing deterministic repository helpers: `repo_manifest`, `repo_chunks`, `git_diff_input`, optional `sloc`, optional `outline`, `render_audit_report`, and `render_review_report`.

### Removed
- Removed the legacy native LLM/provider/session/chat/tool-loop stack from `src/agent/`, `src/llm/`, and the old model-callable tool registry.
- Removed native implementations of shell, edit, webfetch, repo clone, todo, ask, think, search, read, and patch tools; opencode owns those capabilities.
- Removed embedded tree-sitter grammar dependencies; install Universal Ctags separately to enable the optional `outline` MCP tool.
- Removed the embedded `tokei` dependency; install `tokei` separately to enable the optional `sloc` MCP tool.
- Removed the obsolete good-first-issues document; starter work should now come from the current MCP roadmap.
- Removed `oy run --out`; `oy run` now always streams opencode output directly. Use shell redirection to save output.

## [0.10.7] - 2026-06-04

### Added
- `src/net.rs` — shared `is_public_ip()` helper used by both webfetch (`src/tools/network.rs`) and credential transport (`src/llm/route/auth.rs`). Normalises IPv4-mapped-IPv6 addresses and blocks multicast and deprecated site-local IPv6 ranges. Added focused tests for IPv4-mapped-IPv6 and unique-local alignment (REVIEW #1).

### Changed
- `src/tools/registry.rs`, `src/tools/args.rs` — gated `ToolId::Outline` variant and `default_depth()` behind `#[cfg(feature = "outline")]`, matching the existing feature gating on `mod outline` and the outline `ToolDef`. Builds without `--features outline` no longer warn about dead code (REVIEW #1 High).
- `src/llm/test/executor.rs` (748 lines) — split into `chat_tests.rs` (6 Chat Completions tests, ~240 lines) and `responses_tests.rs` (9 Responses API tests, ~310 lines). `executor.rs` retains 3 shared transcript tests and the `read_tool_spec` helper; all 19 tests pass unchanged (REVIEW #1 Medium).
- `src/tools/tests/workspace_tools.rs` (667 lines) — split into tool-oriented files under `src/tools/tests/`: `patch.rs` (9 tests, ~239 lines), `search.rs` (7 tests, ~201 lines), `replace.rs` (3 tests, ~74 lines), `read.rs` (3 tests, ~68 lines), `list.rs` (3 tests, ~62 lines), `sloc.rs` (1 test, ~23 lines). Old file removed; all 48 workflow tests pass unchanged (REVIEW #1 Medium).
- `src/cli/config/paths.rs` — extracted atomic-write implementation (`write_workspace_batch`, `prepare_workspace_write`, `commit_workspace_writes`, backup/rollback, `PreparedWorkspaceWrite`, `CommittedWorkspaceWrite`) into new `src/cli/config/atomic_write.rs`. The public API (`write_workspace_file`, `write_workspace_batch`) remains unchanged as thin delegation wrappers; `paths.rs` drops from 353 to ~210 lines (REVIEW #1 Low).
- `src/tools/preview.rs` — split 862-line file into sub-modules under `src/tools/preview/`: `common.rs` (shared helpers), `workspace.rs` (list, read, search, replace, patch, sloc, outline), `network.rs` (webfetch, repo_clone), `process.rs` (bash), `planning.rs` (todo, think, ask). The parent file is now a 99-line re-export shell. All files under 400 lines; all preview tests pass unchanged (REVIEW #3).
- `src/cli/config/paths.rs` — moved `restrict_to_owner` (Windows ACL) to `src/cli/config/platform/windows.rs` behind `#[cfg(windows)]`. Added `src/cli/config/platform/mod.rs` with `#[cfg(windows)]` re-export. The two call sites in `write_private_file` and `create_private_dir_all` now route through `super::platform::restrict_to_owner` (REVIEW #4).
- `src/agent/model/tests.rs` — moved 10 `#[ignore]` live integration tests and helpers (`is_auth_error`, `assert_model_responds`, `assert_model_uses_tool`, `Echo`, `EchoArgs`) to new `src/agent/model/live_tests.rs`. Unit test file drops from 841 lines to ~675 lines; live tests stay `#[ignore]` and run with `cargo test --lib --ignored` (REVIEW #5).
- `src/audit/input.rs` now includes git-diff input support: `collect_diff_files` (parses `git diff` output into `AuditFile` items, skipping binary diffs), `parse_numstat`, and `build_diff_manifest`. These replace the previous `ReviewChunk`/`DiffItem`/`NumstatEntry` types and duplicated chunking/validation functions in `src/review.rs` (REVIEW #2).
- `src/review.rs` — deleted `ReviewChunk`, `DiffItem`, `NumstatEntry` structs and five duplicated functions (`split_git_diff_items`, `chunk_diff_items`, `ensure_chunks_fit`, `parse_numstat`, `diff_manifest`). Both `prepare_workspace_input` and `prepare_diff_input` now go through the shared `AuditFile`/`AuditChunk` types and `chunk_files`/`ensure_chunks_fit_prompt`/`chunk_text` helpers. All tests pass unchanged.
- `src/llm/route/auth.rs` — `is_loopback_or_private_ip` now delegates to `!crate::net::is_public_ip`, gaining IPv4-mapped-IPv6 normalisation and site-local IPv6 blocking that the previous `IpAddr::is_*` methods missed.

### Removed
- `snapshot` tool. The 0.10.6 implementation was a model-callable stub that returned `success: true` for no-op actions; it has been removed from the registry, schema, args, and preview surface. Any in-flight model call that names `snapshot` will now surface as `unknown tool: snapshot` through the documented fail-closed path.
- Dead `GrepMode::Fuzzy` arm in `search_exact_file`. The only producer of `GrepMode` in the crate (`search_mode`) emits `Regex` or `PlainText`; the third arm now uses `unreachable!()` so the match stays exhaustive (a future enum addition breaks the build here) while the intent is honest.
- One-line `render_with_bat` and `render_plain_with_bat` passthroughs in `src/cli/ui/render.rs`. Both wrappers were renamed-thinned versions of `render_bat`; they are inlined at their five call sites and removed.

### Changed
- `repo_clone` now parses scp-style `git@host:owner/repo[.git]` references and preserves an optional `#fragment` (treated as a sub-path/ref annotation, not a URL fragment). The three `git` invocations are wrapped in `tokio::time::timeout` (300 s for clone/fetch, 30 s for `rev-parse`) and the tool is now registered as `external_side_effect = true` so the transient-retry guard in `tools::invoke_inner` covers it. Parsing of `https://…#fragment` and `git+ssh://…` URLs is preserved.
- `audit::run` is decomposed into `prepare` (collect files, build manifest, security index, and chunk plan, load prior `ISSUES.md`, validate against `--max-chunks`), `async fn execute` (single-chunk fast path or multi-chunk + reduce with bounded parallelism), and `finalize` (transparency line, structured-findings block, succinct summary, optional SARIF render, write). Behaviour, progress events, and outputs are unchanged; all 34 audit tests pass.
- `plan_patch` and `plan_apply_patch` in `src/tools/workspace/patch.rs` now share a `build_patch_plan` helper that owns the directory/symlink/size/read/decode guards, the skip-if-unchanged short-circuit, the display-path dedup check, and the diff computation. The two callers pass only the per-format apply step (unified diff via `diffy::apply` or `*** Begin Patch` via `apply_context_hunks`); behaviour and all 12 patch tests are preserved.
- `is_supported_by_native_openai` is renamed to `is_supported_by_native_backend` in `src/agent/opencode_models.rs`. The predicate covers every native protocol (OpenAI-compatible, Anthropic, Bedrock Converse, Gemini) and excludes only `vertexai`; the new name matches the file's "do not add local provider/model registries" comment and the single call site in `into_adapter_models`.

### Changed (week 3 review fixes)

- `src/audit/report.rs` is decomposed into three focused submodules plus a thin facade. `report.rs` is now a 19-line re-export shell; the original five concerns live in `transparency.rs` (snippet, shell quoting, transparency line, succinct findings summary, markdown post-processing), `findings.rs` (typed `Finding` / `FindingLocation` / `FindingSummary`, markdown + JSON extraction, structured-findings round-trip), and `enhance.rs` (`FindingSource`, `EnhanceFinding`, enhance parsing, `markdown_heading`, `clean_title`, `is_no_finding_title`). External callers keep using `crate::audit::report::*`; the `audit` module test count is unchanged (39 tests pass) and the `audit::run` decomposition from week 2 is preserved.
- `src/llm/providers/route.rs`: `prepare_openai_chat`, `prepare_xai_chat`, and `prepare_openrouter_chat` now share a `build_api_key_route(profile, auth_provider, auth_missing_msg, additional_params)` helper. The three OpenAI-shaped builders shrink to profile lookup + body-options resolution + a single call into the helper; `RouteAuth::ApiKey`, `base_url: Some(...)`, `query_params: None`, and `default_output_tokens: None` are set in one place. Anthropic / Google / Azure / Cloudflare / Bedrock paths stay separate because they use different `RouteAuth` shapes.
- `src/tools/network.rs`: `PublicWebfetchClient::from_target`'s 60-line redirect `Policy::custom` closure now calls a new `validate_public_url_target(url) -> Result<()>` helper that single-sources the scheme allowlist, host validation, IP-literal handling, and per-socket `validate_public_ip` check. The closure itself drops to a 10-line redirect-count + match-on-helper form, and `PublicWebfetchTarget::resolve` now runs the same helper before pinning the async-resolved addrs so a public-at-first host cannot drift to a private-IP target before the request is even built. Three new unit tests cover the helper.
- `src/agent/transcript.rs`: `with_compacted_tool_outputs` and `with_all_tool_outputs_compacted` collapse into a single `compact_tool_outputs(messages, max_bytes, re_compact)` helper. The only difference between the two public methods was the `if text.contains("[tool output compacted]") { continue; }` guard, which is now the `!re_compact` branch of one helper. Both public methods become 3-line wrappers; the call sites in `src/agent/session.rs` (cache-aware compaction + aggressive re-compaction) are unchanged and all session tests pass.

## [0.10.6] - 2026-05-30

### Added
- New workspace tools: `repo_clone` (git clone/refresh for remote repo analysis), `outline` (structural file outline), `snapshot` (conversation context checkpoints), and `think` (structured reasoning).
- `read_multiple_files` now supports `tail_lines` per file; preview tool for bat-backed file previews.

### Changed
- Bumped MSRV to Rust 1.96 and refreshed Cargo.lock (30 crate updates including fff-grep/search 0.8.4, reqwest 0.13.4, hyper 1.10.1).
- Session and transcript improvements; refactored tool registry and schema/args modules.
- Native Gemini/Anthropic/Bedrock protocol fixes and OpenAI route tweaks.

## [0.10.5] - 2026-05-27

### Added
- Native Anthropic Messages protocol support for OpenCode-routed models (e.g. direct Anthropic API via OpenCode), with dedicated provider profile and endpoint stripping.

### Changed
- Moved Anthropic routing out of the OpenAI-compatible bucket into its own `is_anthropic_api()` classifier and added coverage to the opencode models tests.
- Minor style and import-ordering cleanups in `audit/report`, `cli/app/enhance_cmd`, and LLM route modules.

## [0.10.4] - 2026-05-23

### Changed
- Removed legacy top-level task/continue/resume CLI argument rewrites; use explicit `oy run ...` forms.
- Split the large tools test module into boundary-focused test modules and refreshed architecture/contributor docs for the current native LLM boundary.
- Scoped OpenRouter provider-body options to OpenRouter routes, made the model-info cache key model-specific, and tightened audit/SARIF finding extraction to explicit severity headings instead of broad Markdown-heading guesses.
- Moved unsupported-provider enforcement into route resolution and split `agent::model` into facade, execution, metadata-cache, reasoning, and test modules.
- Added a native Gemini protocol for Google AI Studio/OpenCode Gemini models, including SSE parsing, function calling, usage mapping, and Gemini tool-schema projection.

## [0.10.3] - 2026-05-21

### Fixed
- Restored `webfetch` public IP classification to maintained `ip_rfc` global-address semantics, with explicit denials for multicast and deprecated IPv6 site-local ranges.

## [0.10.1] - 2026-05-21

### Changed
- Preserved raw terminal/ANSI output through `bash`, markdown, diff, and always-coloured bat-backed previews so bat/terminal formatting is not replaced with visible escape glyphs.

## [0.10.0] - 2026-05-21

### Changed
- Reorganized the Rust-native LLM backend toward OpenCode `packages/llm`: schema/events, provider profiles, route auth/framing/transport, protocol modules, cache policy, and tool runtime are now separated and covered by focused tests.
- Expanded native provider routing for xAI, OpenRouter, Azure OpenAI, Cloudflare AI Gateway, Cloudflare Workers AI, and Amazon Bedrock Converse while keeping Anthropic/Gemini providers fail-closed until their protocols are ported.
- Added OpenCode-style cache policy placement for inline-cache protocols and Bedrock Converse cache-point lowering with AWS event-stream decoding and SigV4/bearer auth support.
- Added terminal title/zellij pane progress updates for human-mode sessions while keeping quiet/JSON output clean.

## [0.9.8] - 2026-05-19

### Changed
- Completed Month 6 of the LLM internals roadmap: prompt-level provider retries are now side-effect aware, transient retry backoff uses fewer jittered attempts, Chat/Responses share tool-round budget handling, and model-visible schemas better describe common risky or malformed tool arguments.
- Completed Month 5 of the LLM internals roadmap: native OpenAI-compatible Chat/Responses tool loops now mark tool failures with `TOOL_ERROR`/`RECOVERY`, hint enabled tools for unknown names, block repeated identical failed calls, cap model-visible tool output with head/tail preservation, and stop long tool-only churn without lowering the default tool-round budget.
- Made audit input handling fail before truncating review chunks that exceed the model budget, and escaped terminal/control sequences in `bash` stdout/stderr before they enter tool output or previews.
- Added fuzzy path suggestions to missing `read` tool errors while keeping `read` exact-only and requiring a follow-up explicit path.

## [0.9.6] - 2026-05-19

### Fixed
- Preserved trusted syntax-highlighting/color ANSI in tool previews while still neutralizing untrusted terminal escape bytes from tool output and file content.
- Accepted `*** Begin Patch` / `*** Update File:` patch tool input for existing UTF-8 files while continuing to reject create/delete, symlink, binary, non-UTF8, and out-of-workspace patches.
- Stopped sending unsupported `previous_response_id` in native OpenAI Responses tool loops by replaying function calls/results in `input`.
- Round-tripped DeepSeek `reasoning_content` through native OpenAI-compatible Chat Completions tool loops.
- Sanitized terminal-bound tool progress, previews, errors, markdown, and diff previews to neutralize model/tool-supplied escape bytes before display.
- Replaced local public IPv4 classification logic for `webfetch` with `ip_rfc` global-address classification plus explicit public-fetch denials for multicast and deprecated IPv6 site-local addresses.
- Removed credential-like environment variables from `bash` child processes by default and documented the remaining shell trust boundary.
- Added focused maintenance coverage for the tool approval matrix, expanded webfetch IP cases, shell environment filtering, and `oy doctor --help` snapshots.
- Simplified local tooling so `mise install` plus `just check` uses only the pinned stable Rust toolchain and `just`; `just ci` keeps optional nextest/Miri parity checks.

### Changed
- Completed Month 4 of the LLM internals roadmap: the native OpenAI-compatible Chat/Responses backend is now the default for OpenAI, Copilot API-token, and OpenCode-compatible routes; `src/tools/llm.rs` adapts tools directly to `oy`'s `llm::LlmTool` boundary; and the previous external backend dependency, adapters, native-backend feature flag, and GitHub-token Copilot shim were removed.
- Completed Month 3 of the LLM internals roadmap: native OpenAI Chat and Responses requests route through a non-streaming backend with focused request/response goldens, while auth lookup and provider metadata stay in `agent::auth`/OpenCode.
- Completed Month 2 of the LLM internals roadmap: transcripts now store `oy`-owned `llm::Message` values, `agent::model` accepts those messages directly, tool schema exposure stays in one `oy` registry, and previous backend-specific message/tool conversions live in adapter modules only.
- Added the Month 1 `src/llm/` facade for `oy`-owned LLM request/response, message, tool-spec, route, and backend-trait types while keeping the then-current backend behind one adapter seam.

## [0.9.4] - 2026-05-13

### Fixed
- Reissued the release so GitHub Actions can publish the expected CI-built binary assets without the duplicate immutable-release state from v0.9.3.

## [0.9.3] - 2026-05-12

### Changed
- Made the `patch` tool more tolerant of LLM-generated diffs by retrying raw unprefixed paths when the default `strip = 1` target does not resolve.

### Fixed
- Improved failed patch-application errors with the failing hunk number and guidance to re-read the file before regenerating stale hunks.

## [0.9.0] - 2026-05-11

### Added
- Added a `patch` workspace tool for applying unified/git diffs to existing UTF-8 files, with file-write policy gating, approval previews, output summaries, and focused coverage for rejected unsafe patch shapes.

### Changed
- Switched workspace diff generation from `similar` to `diffy` so tool previews emit applyable unified diffs.
- Retried transient Rig `ApiResponse` parse failures through the normal LLM backoff path.

## [0.8.10] - 2026-05-08

### Added
- Google Gemini (`opencode/gemini-3-flash`, `opencode/gemini-3.1-pro`) and Anthropic Claude models via OpenCode are now visible in `oy model` and usable for chat/audit.
- Live integration tests for Google, Anthropic, DeepSeek, and Kimi models including tool-calling smoke tests. Run with `cargo nextest run --run-ignored ignored-only live_`.

### Changed
- `ISSUES.md` is always excluded from the initial audit collection context; existing `ISSUES.md` content is included in the final prioritise/rewrite step so the model can carry forward still-relevant findings.

## [0.8.7] - 2026-05-07

### Security
- Stripped ESC (`\x1b`) characters from model output in `render_markdown` and `paint` to prevent terminal ANSI escape injection (CWE-150, OWASP ASVS V5.3.4).
- Replaced `{err:#}` alternate formatting with plain `{err}` in `main.rs`, `chat.rs`, and `progress.rs` to avoid leaking API keys through error chains (OWASP ASVS V7.3).
- Added `192.0.0.0/24` (IETF Protocol Assignments) to the `is_public_ipv4` blocklist, closing an SSRF bypass in `tool_webfetch` (OWASP ASVS V5.2.6).
- Replaced `unsafe { std::env::set_var/remove_var }` calls in `/thinking` with a thread-safe `LazyLock<RwLock<Option<String>>>` store, eliminating undefined behaviour from concurrent environment mutation.

## [0.8.6] - 2026-05-07

### Changed
- Derived all audit sizing constants (chunk size, reduce prompt limit, findings budget, security index) from the current model's token limits instead of hardcoded values.
- Derived context config input limit and output reserve ratio from model-specific token limits via OpenCode metadata, so session compaction and budget enforcement adapt per model.
- Replaced the hardcoded `context_config()` with `context_config_for_model()` that takes optional model input/output limits; env vars `OY_CONTEXT_LIMIT` and `OY_CONTEXT_OUTPUT_RESERVE` still override.

### Removed
- Removed `rig-bedrock` and `rig-vertexai` dependencies, `src/agent/bedrock.rs`, Bedrock/VertexAI chat routes, and all associated provider mappings, auth status, and docs.
- Removed dead code: `wrap_line` function, `ProviderInfo.model` field, `OpenCodeVariant` struct field, and the now-unused `textwrap` dependency.

## [0.8.5] - 2026-05-06

### Changed
- Replaced low-level LLM plumbing with Rig agents.
- Simplified model metadata routing.
- Used OpenCode model metadata for reasoning capability and effort discovery.

### Fixed
- Fixed Bedrock adaptive thinking params and Converse routing regression.

## [0.8.0] - 2026-05-05

### Changed
- Trimmed the Rust library surface so only the command runner and diagnostic helper remain stable public API; internal modules are now crate-private.
- Moved snapshot coverage into the modules that own chat command help and tool preview rendering.
- Split CLI UI rendering/progress/text helpers and session storage/no-op guard helpers into smaller modules for local reasoning.
- Replaced ad-hoc JSON construction in tool implementations with typed internal output structs before serialization.
- Updated `ISSUES.md` with validation status for remediated and still-open audit findings.

### Fixed
- Enforced disabled-network policy inside the `webfetch` sink before URL resolution or outbound I/O.
- Prevented `list` glob expansion from reporting entries whose canonical path resolves outside the workspace through symlinks.
- Hardened audit input skipping for more secret-like filenames, including `.env.*`, credentials, secrets, and token files.
- Serialized transcript compaction input as escaped JSON records and marked message bodies as untrusted data to avoid pseudo-XML prompt-boundary confusion.
- Kept SARIF generation available when one model-produced code reference is unsafe by omitting only that result location.
- Preserved middle audit findings during reduce compaction by trimming per finding instead of raw head/tail truncation.
- Shell-quoted the Docker mount argument printed by `doctor` for container safety guidance.

## [0.7.16] - 2026-05-05

### Changed
- Updated agent guidance to favor simple, direct, data-oriented code with explicit local data/control flow, stable boundaries, and measured performance.
- Audit prompts now flag complexity that complects concerns, hides state/dataflow, blocks local reasoning, or obscures performance/security boundaries.
- Transcript compaction now preserves design constraints, invariants, and rejected abstractions when they affect follow-up work.

## [0.7.13] - 2026-04-29

### Changed
- `webfetch` now follows public redirects by default and sends non-credentialed document-friendly `User-Agent`/`Accept` headers so common public docs URLs work without extra model-supplied headers.

### Fixed
- Bounded large-audit reduce prompts so high-chunk audits compact candidate findings before hitting model prompt limits.

## [0.7.12] - 2026-04-28

### Added
- Added `oy audit --format sarif`, writing SARIF 2.1.0 output to `oy.sarif` by default for GitHub code scanning ingestion.

### Changed
- Audit transparency snippets now quote shell-sensitive model, output, and focus values.
- Centralized text/binary decoding for audit and file tools.

### Fixed
- Scoped `OPENAI_API_KEY` and `OPENAI_BASE_URL` to OpenAI/OpenAIResp requests so they are not applied to unrelated providers.
- Stopped Bedrock Mantle discovery from accepting OpenAI credentials or endpoint overrides; Mantle now requires Bedrock-specific bearer credentials.

## [0.7.7] - 2026-04-28

### Added
- Reworked `oy audit` as a deterministic no-tools audit pipeline that writes `ISSUES.md` by default, embeds OWASP ASVS/MASVS plus grugbrain guidance, and uses full-repo or map→reduce review depending on repository size.
- Added generated audit report transparency lines showing the `oy audit` command/model context used.

### Changed
- Consolidated the agent stack into `src/agent.rs` and CLI/runtime UI/configuration into `src/cli.rs`, leaving a smaller top-level module surface for future maintenance.
- Reorganized `src/tools.rs` with explicit review sections while keeping the tool registry in one place.
- Audit progress now emits consolidated phase updates instead of per-chunk detail spam.
- Audit reports now request and backfill a succinct all-findings summary with code refs, while reserving detailed writeups for the most severe 10-20 findings.
- Audit review input is now collected by the Rust runner rather than discovered by model tool calls, making included text and chunking deterministic.
- `read` previews now clamp long code lines to the terminal preview width and expand tabs to stable columns so line-number gutters do not visually drift.

## [0.7.6] - 2026-04-27

Consolidated changes since `v0.7.5`.

### Added
- Native AWS Bedrock Converse support with AWS SDK credential loading, SSO-expiry detection, `aws sso login` retry, and tool-use conversion.
- Bedrock Mantle routing via Bedrock API bearer tokens, `AWS_BEARER_TOKEN_BEDROCK`, and contemporary Moonshot/Kimi model hints.
- OpenCode Zen/Go routing shims (`opencode::`, `opencode-go::`) with `OPENCODE_API_KEY`, endpoint overrides, and fallback to `~/.local/share/opencode/auth.json`.

### Changed
- Tightened terminal UX with dense grouped tool-call progress, bat-like text previews, color-aware markdown/diff rendering, and clearer truncation.
- Simplified docs and examples around `--mode`, `copilot::`, OpenAI, AWS Bedrock, OpenCode, and local OpenAI-compatible defaults.
- Moved built-in prompts/tool descriptions into Rust, removed the TOML prompt asset, and trimmed terminal rendering dependencies.
- Refreshed the Rust toolchain/dependency baseline, including AWS SDK-backed Bedrock integration.

## [0.6.0] - 2026-04-22

Major changes since `v0.5.1`.

### Added
- Bedrock Mantle provider support for AWS credential/SigV4 mode, including model listing against the Mantle endpoint.
- A `SECURITY.md` policy pointing researchers to the WA Government vulnerability disclosure process.

### Changed
- Audit runs now show clearer wait/progress reporting while long-running review work is in flight.
- Audit dependency assessment now records Renovate warnings in `ISSUES.md` and keeps the phase1 summary idempotent.

### Fixed
- Bedrock Mantle chat requests now fall back from `/v1/responses` to `/v1/chat/completions` when a model does not support the responses API.
- Resumed audits continue to backfill missing `run_config` state and restore the generated transparency snippet in `ISSUES.md`.
- Audit review flow was tuned for better speed and output quality.

## [0.5.1] - 2026-04-15

### Fixed
- Audit reports now always upsert the transparency snippet instead of only when `# Audit Issues` is the first line, so banner/comment preambles no longer suppress it.
- Resumed audits now backfill missing `run_config` state and reapply the transparency line before review continues.
- Phase3 audit summary rewrites now reinsert the transparency line if the summary pass removes it.

## [0.5.0] - 2026-04-15

Major changes since `v0.4.6`.

### Added
- Session continuation and resume for `oy chat` and `oy run` via `--continue-session` and `--resume <name-or-number>`.
- Built-in modes for common approval policies: `plan`, `accept-edits`, and `auto-approve`.
- `oy renovate-local` for running Renovate locally and writing lookup reports to `.tmp/renovate-<date>.json`.
- An audit transparency snippet in generated `ISSUES.md` reports showing the `oy` command used.

### Changed
- Reworked `oy audit` into a resumable three-phase workflow (`plan`, `review`, `summary`) with per-workspace audit state stored in the session cache.
- Switched audit reporting to an inbox-based `ISSUES.md` flow so chunk reviews append findings first, then condense and reorganize them into the final summary.
- Improved audit chunking, retry/stall handling, and progress validation; audit retries with smaller chunks if a review pass fails to update `ISSUES.md`.
- Improved CLI/runtime previews, session handling, and test coverage while removing redundant helper code.
