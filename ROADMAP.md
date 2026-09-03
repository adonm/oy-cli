# Roadmap

_Updated July 2026. Ordered by outcome, not promised release date._

## Direction

**Mission:** give the coding agent the user already runs deterministic repository evidence and report workflows.

**Primary user:** a maintainer already using OpenCode who wants oy to complete audit, review, and remediation work without adopting another model runtime or another permission system.

**Core loop:** prepare deterministic evidence → let OpenCode reason and edit under the user's permissions → validate a durable report → rerun to confirm.

The integration is CLI-first, package-delivered, and skill-led. OpenCode wrappers remain only where the workflow needs them.

## Product principles

1. **OpenCode owns execution policy.** Users configure models, agents, permissions, edits, shell, web, sessions, and approvals in OpenCode. Oy does not maintain parallel plan/edit/auto permission modes.
2. **No custom agent.** Oy ships skills and deterministic evidence, not a system prompt or default-agent override. Completion discipline (inspect first, smallest change, verify, report concisely) lives in the skill protocols and runs under the user's own agent.
3. **Own the evidence boundary, not the model.** Oy owns collection, ordering, limits, evidence identity, and report normalization; OpenCode owns inference and general tools.
4. **Skills are the integration contract.** Audit, review, and one-finding remediation protocols should be usable from normal OpenCode sessions and should not require dedicated permission-adapter agents.
5. **Prefer files over large tool responses.** Prepare immutable workspace-local evidence artifacts, return small structured descriptors, and let OpenCode read them with native tools.
6. **Fail closed rather than sample silently.** Coverage limits, exclusions, changed evidence, malformed reports, and incomplete runs must be visible.
7. **Reports are handoff artifacts.** Stable IDs, statuses, SARIF, and rerun semantics matter more than chat or launcher conveniences.
8. **Keep OpenCode coupling narrow.** Do not install, configure, version-gate, or upgrade more of OpenCode than the workflow requires.

## Current transition

Version 0.13 established deterministic collection, file-backed preparation/finalization, stable report rendering, one agent, and three canonical skills.

Completed in the current development cycle:

- Consolidated `oy`, `oy-plan`, `oy-edit`, and `oy-auto` into one autonomous `oy` agent.
- Removed dedicated auditor, reviewer, and enhancer permission adapters; all three skills execute under the user's effective OpenCode permissions.
- Removed oy's safety-mode and abstract tool-policy layers.
- Updated the `oy` prompt against OpenCode 2's Build-agent behavior: inspect first, make the smallest correct change, persist end-to-end, preserve unrelated worktree changes, verify, and report concisely.
- Added `oy audit|review prepare` and `finalize` with workspace-local evidence, private state, SHA-256 artifact binding, changed-input/output rejection, and strict candidate findings.
- Rewrote audit/review skills around native OpenCode reads and edits.
- Added the `@oy-cli/opencode` V2 package for the agent, skills, and commands.
- Made setup package-first and removed direct agent/skill/command installation.
- Removed the separate Cursor CLI/assets stack; Cursor remains available only as an OpenCode provider.
- Removed the obsolete MCP adapter, MCP-only wrappers, and Sighthound integration after file-backed workflows reached parity.
- Stopped writing global tool-output overrides in default setup.

## Completed — make the CLI the deterministic boundary

### File-backed evidence

- [x] Add `oy audit prepare` and `oy review prepare` commands that write immutable artifacts under a workspace-local run directory and print a small versioned JSON descriptor.
- [x] Write an index containing scope, resolved target, coverage, exclusions, chunk paths, byte/line counts, and stable digests.
- [x] Keep authoritative run state outside model-writable artifacts; validate artifact hashes during finalization.
- [x] Bound artifacts for practical OpenCode `read` paging without carrying source text through tool responses.
- Add cleanup and stale-run handling without touching tracked `.gitignore` files.

### Report finalization

- [x] Add `oy audit finalize` and `oy review finalize` commands that validate the bound output, evidence identity, report shape, findings payload, stable IDs, and SARIF/Markdown metadata.
- [x] Make the model write the candidate report with normal OpenCode tools; keep the final canonical rewrite in Rust.
- [x] Make generation time explicit by binding the preparation date.
- [x] Replace implementation-defined evidence hashes with a versioned SHA-256 digest.

### Skill migration

- [x] Rewrite the three canonical skills around `prepare → native reads/edits → finalize`.
- [x] Package the skills, agent, and commands through the OpenCode V2 plugin API while retaining local installation.
- [x] Keep `oy run --auto` as a thin convenience over the single `oy` agent; explicit OpenCode denies remain authoritative.
- Evaluate protocol compliance from session traces, while documenting that a file-based CLI cannot cryptographically prove the model read the index, previous report, and every indexed chunk.

## Next — remove transitional OpenCode machinery

After the CLI and skills cover the deterministic contract:

- [x] Remove `oy mcp`, MCP-only wrappers, and Sighthound integration.
- [x] Stop writing global `tool_output` overrides.
- [x] Reduce setup to registering/removing the version-matched plugin package while preserving unrelated OpenCode JSON/JSONC.
- [x] Remove `oy model`, `oy open`, `oy chat`, and implicit passthrough of unknown arguments; keep bare `oy` as the integration-aware TUI launcher.
- Demote or remove exact beta version gates, session recovery wrappers, and coupled oy/OpenCode upgrades.
- When OpenCode 2 leaves beta, replace the moving `beta` dependencies with the stable `latest` channel and remove beta-specific host handling.
- Stop installing OpenCode from the oy installer; treat it as a user-managed prerequisite.

## Success signals

- A normal OpenCode user can install oy, load an oy skill, and keep their existing permission policy.
- Setup owns only version-matched package registration and migration of legacy oy entries.
- Evidence preparation returns a small stable JSON descriptor and workspace-local artifacts with explicit coverage.
- Unchanged evidence and explicit metadata produce byte-stable canonical reports.
- A finding ID can drive one focused fix and disappear or change status on rerun.
- OpenCode API compatibility code shrinks without losing collection, report, SARIF, or workflow quality.

## Non-goals

- Owning or bypassing OpenCode permissions.
- Rebuilding OpenCode's provider routing, model loop, chat UI, sessions, editing, shell, web, or general search tools.
- Claiming deterministic findings from nondeterministic model reasoning.
- Persisting provider credentials or transcripts.
- Adding arbitrary shell, edit, network, or clone capability to deterministic oy helpers.
- Supporting every OpenCode prerelease before its relevant integration contract is tested.
- Running paid/provider-backed evaluations in default CI.
