# Roadmap

_Updated July 2026. Ordered by outcome, not promised release date._

## Direction

**Mission:** give OpenCode a concise autonomous agent plus deterministic repository evidence and report workflows.

**Primary user:** a maintainer already using OpenCode who wants oy to complete audit, review, and remediation work without adopting another model runtime or another permission system.

**Core loop:** prepare deterministic evidence → let OpenCode reason and edit under the user's permissions → validate a durable report → rerun to confirm.

The intended integration is CLI-first, package-delivered, and skill-led. MCP and host wrappers are transitional compatibility surfaces, not the destination.

## Product principles

1. **OpenCode owns execution policy.** Users configure models, agents, permissions, edits, shell, web, sessions, and approvals in OpenCode. Oy does not maintain parallel plan/edit/auto permission modes.
2. **Keep one useful agent.** The packaged `oy` agent is a concise autonomous system prompt. It adds completion discipline and engineering defaults without overriding the user's permissions.
3. **Own the evidence boundary, not the model.** Oy owns collection, ordering, limits, evidence identity, and report normalization; OpenCode owns inference and general tools.
4. **Skills are the integration contract.** Audit, review, and one-finding remediation protocols should be usable from normal OpenCode sessions and should not require dedicated permission-adapter agents.
5. **Prefer files over large tool responses.** Prepare immutable workspace-local evidence artifacts, return small structured descriptors, and let OpenCode read them with native tools.
6. **Fail closed rather than sample silently.** Coverage limits, exclusions, changed evidence, malformed reports, and incomplete runs must be visible.
7. **Reports are handoff artifacts.** Stable IDs, statuses, SARIF, and rerun semantics matter more than chat or launcher conveniences.
8. **Keep host coupling narrow.** Do not install, configure, version-gate, or upgrade more of OpenCode than the workflow requires.

## Current transition

Version 0.12 established deterministic collection, file-backed preparation/finalization, stable report rendering, one agent, and three canonical skills. MCP is now an unregistered compatibility adapter.

Completed in the current development cycle:

- Consolidated `oy`, `oy-plan`, `oy-edit`, and `oy-auto` into one autonomous `oy` agent.
- Removed dedicated auditor, reviewer, and enhancer permission adapters; all three skills execute under the user's effective OpenCode permissions.
- Removed oy's safety-mode and abstract tool-policy layers.
- Updated the `oy` prompt against OpenCode 2's Build-agent behavior: inspect first, make the smallest correct change, persist end-to-end, preserve unrelated worktree changes, verify, and report concisely.
- Added `oy audit|review prepare` and `finalize` with workspace-local evidence, private state, SHA-256 artifact binding, changed-input/output rejection, and strict candidate findings.
- Rewrote audit/review skills around native OpenCode reads and edits.
- Added the `@oy-cli/opencode` V2 package for the agent, skills, and commands.
- Made setup package-first and removed direct agent/skill/command installation.
- Stopped registering MCP and global tool-output overrides in default setup.

## Completed — make the CLI the deterministic boundary

### File-backed evidence

- [x] Add `oy audit prepare` and `oy review prepare` commands that write immutable artifacts under a workspace-local run directory and print a small versioned JSON descriptor.
- [x] Write an index containing scope, resolved target, coverage, exclusions, chunk paths, byte/line counts, and stable digests.
- [x] Keep authoritative run state outside model-writable artifacts; validate artifact hashes during finalization.
- [x] Bound artifacts for practical OpenCode `read` paging even though shell/MCP response limits no longer carry source text.
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
- Evaluate protocol compliance from session traces, while documenting that a file-based CLI cannot cryptographically prove the model read every artifact.

## Next — remove transitional host machinery

After the CLI and skills cover the deterministic contract:

- [x] Stop registering MCP in default setup; retain `oy mcp` temporarily as an adapter over the same typed core.
- [x] Stop writing global `tool_output` overrides.
- Reduce setup to installing/removing the `oy` agent and canonical skills, ideally without rewriting OpenCode JSON/JSONC.
- [x] Remove managed model listing, duplicate open/chat commands, and implicit TUI argument passthrough.
- Demote or remove exact beta version gates, session recovery wrappers, and coupled oy/OpenCode upgrades.
- Stop installing OpenCode from the oy installer; treat it as a user-managed prerequisite.
- Split repository evidence and report operations out of `src/mcp.rs` into typed reusable Rust services.

## Agent alignment

The `oy` system prompt intentionally remains much shorter than OpenCode's provider-specific prompts. Because a custom system prompt replaces those base prompts, maintain the following parity explicitly:

- inspect the repository before editing;
- use existing dependencies and conventions;
- implement rather than only propose when the request calls for action;
- persist through verification and a clear result;
- preserve dirty-worktree changes not made by the agent;
- avoid destructive Git operations and unrequested commits;
- prefer minimal changes and local reasoning;
- batch independent inspection and keep communication concise.

Do not copy provider-specific frontend preferences, formatting rules, tool names, or temporary implementation details into oy. Compare the prompt against tagged OpenCode 2 releases during compatibility updates and use live evaluations for behavioral changes.

## Success signals

- A normal OpenCode user can install oy, select the `oy` agent or load an oy skill, and keep their existing permission policy.
- Setup owns one agent and three skills, then eventually only the minimum files needed for discovery.
- Evidence preparation returns a small stable JSON descriptor and workspace-local artifacts with explicit coverage.
- Unchanged evidence and explicit metadata produce byte-stable canonical reports.
- A finding ID can drive one focused fix and disappear or change status on rerun.
- MCP and OpenCode API compatibility code shrink without losing collection, report, SARIF, helper, or workflow quality.
- Prompt evaluations show that the shorter `oy` agent matches or improves OpenCode Build on completion, verification, worktree safety, and concise communication.

## Non-goals

- Owning or bypassing OpenCode permissions.
- Rebuilding OpenCode's provider routing, model loop, chat UI, sessions, editing, shell, web, or general search tools.
- Claiming deterministic findings from nondeterministic model reasoning.
- Persisting provider credentials or transcripts.
- Adding arbitrary shell, edit, network, or clone capability to deterministic oy helpers.
- Supporting every OpenCode prerelease before its relevant integration contract is tested.
- Running paid/provider-backed evaluations in default CI.
