# Roadmap

_Updated July 2026. Ordered by outcome, not promised release date._

## Direction

**Mission:** make repository-wide audit, review, and remediation in opencode more repeatable, bounded, and reviewable.

**Primary user:** a maintainer already using opencode who wants better evidence coverage and durable reports, without adopting another model client or agent runtime.

**Core loop:** collect deterministic inputs → run a restricted audit/review agent → render a stable report → fix one finding → rerun to confirm.

Setup, launch, and legacy command aliases support that loop. They are compatibility surfaces, not separate product pillars.

## Product principles

1. **Own the evidence boundary, not the model.** oy owns collection, ordering, limits, and report normalization; opencode owns inference and general tools.
2. **Fail closed rather than sample silently.** Coverage limits and exclusions must be visible.
3. **Reports are handoff artifacts.** Stable IDs, statuses, SARIF, and one-finding remediation matter more than chat features.
4. **Keep setup reversible and unsurprising.** Generated ownership and explicit setup/removal behavior must be inspectable.
5. **Add native code only for deterministic value.** Prefer opencode built-ins unless an oy helper materially improves repeatability or safety.

## Recently completed

### v0.12.0-beta.1

- Dropped OpenCode 1 and moved noninteractive workflows to OpenCode 2's restored runner through the selected CLI executable.
- Added pinned beta host detection, session continuation/resume, mode-selected run agents, model overrides, and managed-API model listing.
- Migrated generated JSON, commands, MCP registration, and agent permissions to native OpenCode 2 with fail-closed legacy handling.
- Separated the optional pinned Sighthound source build from routine missing-tool installation.
- Consolidated workflow orchestration into three canonical skills with thin adapters; added rollback-capable setup/removal batches, effective runtime doctor checks, root-bound execution, typed workflow contexts, structured MCP errors/results, and transport-safe file slicing.

### v0.11.15

- Moved the Pages site to a pinned mdBook build with navigation, search, and CI/Pages build verification.
- Published an evidence-scoped release/opencode/helper compatibility matrix.
- Added checked-in audit, target-diff review, no-findings, remediation, SARIF, and GitHub upload examples.
- Added setup idempotency and generated-file ownership tests.
- Added request-level MCP initialize/tools-list coverage, an exact advertised-tool inventory check, and source-backed CLI/MCP reference drift tests.

## Now — make the core contract dependable

### Setup safety and compatibility

- Expand safe legacy-config migration only where behavior can be preserved exactly; keep ambiguous permission/provider/plugin conversions fail-closed.
- Preserve user-authored JSONC comments/formatting while retaining atomic multi-file setup/remove.
- Validate generated global and workspace config against the current opencode schema.
- Expand `oy doctor --check` with sanitized plugin/provider failure detail and stronger service validation.
- Add a pinned cross-version OpenCode API smoke matrix; keep the published evidence matrix current without claiming provider-backed integration coverage.

### Coverage and protocol confidence

- Add transport-level fixtures for MCP `initialize`, `tools/list`, and representative `tools/call` requests.
- Add deterministic fixtures for manifests, repository chunks, target diffs, Markdown, and SARIF.
- Include a collection summary in reports: included files/chunks plus skipped categories, oversized files, and unreadable/non-text files.
- Decide how lockfiles participate in security audits so supply-chain review is not silently excluded.

### Workflow usefulness

- Make finding selection explicit and testable across `audit → enhance → audit` and `review → enhance → review` loops.
- Expand the pinned evaluation corpus with known-vulnerability recall canaries, real regression diffs, and mature precision baselines.
- Track prompt changes by recall, precision, evidence quality, actionability, protocol compliance, latency, and cost.

## Next — reduce friction and improve integrations

- Improve scoped audits for monorepos while preserving explicit, reportable coverage.
- Expose machine-readable doctor/setup diagnostics suitable for CI preflight checks.
- Evaluate helper value and maintenance cost; keep only evidence tools that measurably improve findings.

## Later — adopt stable host capabilities

- Track tagged OpenCode API/config changes beyond the pinned beta contract.
- Keep MCP as the deterministic helper boundary unless an opencode-native tool/plugin interface provides the same isolation with less maintenance.
- Preserve a straightforward migration path for generated config and reports across major opencode versions.

## Success signals

- Setup is idempotent, schema-valid, and does not alter unrelated user configuration.
- Every report explains its scope, exclusions, model metadata, and coverage limit.
- Protocol fixtures catch tool/schema drift before release.
- Evaluation changes improve at least one target behavior without a material precision or safety regression.
- A finding ID can drive a focused fix and disappear or change status on the next report.
- The native dependency and command surface stays small as workflow quality improves.

## Non-goals

- Rebuilding opencode's provider routing, model loop, chat UI, sessions, editing, shell, web, or general search tools.
- Adding arbitrary shell execution, source editing, network fetch, or repository cloning to `oy mcp`.
- Claiming deterministic findings from nondeterministic model reasoning.
- Persisting provider credentials, transcripts, model selection, or session state in oy.
- Running paid/provider-backed model evaluations in default CI.
- Supporting every host prerelease before its integration contract stabilizes.
