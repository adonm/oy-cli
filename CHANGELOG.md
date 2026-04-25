# Changelog

## [Unreleased]

### Changed
- Rewrote the CLI in Rust around `genai`, `rustyline`, ripgrep ecosystem crates, and `toon-format`.
- Updated docs and examples to prefer native `genai` model ids such as `github_copilot::openai/gpt-4.1-mini` and `local-8080::qwen3.5`.

### Added
- Rust `audit`, `audit-logic`, and `renovate-local` command support.

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
- `oy audit-logic [focus]`, a logic-focused audit mode that skips docs and lockfiles, strips comments/docstrings where possible, and concentrates review on executable behaviour.
- Session continuation and resume for `oy chat` and `oy run` via `--continue-session` and `--resume <name-or-number>`.
- Built-in agent profiles for common approval modes: `plan`, `accept-edits`, and `auto-approve`.
- `oy renovate-local` for running Renovate locally and writing lookup reports to `.tmp/renovate-<date>.json`.
- An audit transparency snippet in generated `ISSUES.md` reports showing the `oy` command used.

### Changed
- Reworked `oy audit` into a resumable three-phase workflow (`plan`, `review`, `summary`) with per-workspace audit state stored in the session cache.
- Switched audit reporting to an inbox-based `ISSUES.md` flow so chunk reviews append findings first, then condense and reorganize them into the final summary.
- Improved audit chunking, retry/stall handling, and progress validation; audit retries with smaller chunks if a review pass fails to update `ISSUES.md`.
- Improved CLI/runtime previews, session handling, and test coverage while removing redundant helper code.
