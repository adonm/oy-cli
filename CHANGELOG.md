# Changelog

## Unreleased

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
