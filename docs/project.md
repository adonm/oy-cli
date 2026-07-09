# Project direction

oy exists to improve the audit → review → remediate loop in opencode, not to become another general AI coding runtime.

## Mission and audience

**Mission:** make repository-wide audit, review, and remediation in opencode more repeatable, bounded, and reviewable.

**Primary user:** a maintainer already using opencode who wants better evidence coverage and durable reports without adopting another provider client, chat UI, or agent framework.

**Primary artifact:** a scoped, evidence-backed Markdown or SARIF report that can be rerun, compared, and handed to one-finding remediation.

## Product scope

| Scope | Responsibilities |
|---|---|
| Core | Audit, target-diff review, deterministic evidence tools, report rendering, stable IDs, and remediation handoff. |
| Supporting | Safe setup, doctor diagnostics, optional local evidence helpers, and opencode launch integration. |
| Compatibility | General oy agent, safety-mode aliases, run/chat/model wrappers, upgrade, and opencode passthrough. |

## Decision principles

1. **Own the evidence boundary, not the model.** Deterministic collection and rendering are oy's value.
2. **Fail closed instead of sampling silently.** Scope, limits, and exclusions should be inspectable.
3. **Optimize for handoff artifacts.** Reports and finding lifecycle matter more than chat features.
4. **Make setup reversible and unsurprising.** Owned writes and refresh behavior must be explicit.
5. **Add native code only for deterministic value.** Prefer host capabilities when they already solve the job.

## Roadmap summary

### Now: dependable core contract

- Preserve unrelated user config and validate setup ownership/idempotency.
- Add MCP transport, collection, diff, Markdown, and SARIF fixtures.
- Report included scope and skipped categories; resolve lockfile coverage.
- Publish tested opencode/platform compatibility and stronger doctor checks.
- Expand evaluated audit/review/remediation examples and quality canaries.

### Next: lower friction

- Keep CLI/MCP reference material verified against source.
- Improve practical SARIF/CI integration and machine-readable preflight.
- Improve explicit monorepo scoping without hidden sampling.
- Retain optional helpers only when evaluation shows measurable value.

### Later: stable host integration

Adopt stable opencode APIs/config changes when they reduce CLI coupling, while keeping the deterministic helper boundary small and migration straightforward.

Read the [canonical roadmap and success criteria](https://github.com/adonm/oy-cli/blob/main/ROADMAP.md).

## Non-goals

- Rebuild opencode's provider routing, model loop, chat, sessions, editing, shell, web, or general search.
- Add arbitrary shell, edit, network fetch, or clone capabilities to oy MCP.
- Claim deterministic findings from model reasoning.
- Persist provider credentials, transcripts, model selection, or session state.
- Run paid/provider-backed evaluations in default CI.
- Chase every host prerelease before its integration contract stabilizes.

## Contribute

Changes should improve evidence coverage, report usefulness, setup safety, or measured prompt quality without broadening the product into a second host.

Read the [contributor guide](https://github.com/adonm/oy-cli/blob/main/CONTRIBUTING.md), [architecture](architecture.md), and [evaluation playbook](evaluation.md).
