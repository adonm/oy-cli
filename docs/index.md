# Audits and code reviews for coding agents

`oy` adds repeatable, deterministic-input audit and review workflows to the coding agent you already use. The workflows ship as standard [Agent Skills](https://agentskills.io/) under `.agents/skills`, which OpenCode, Cursor, Codex, Copilot, and Gemini CLI all read natively.

Use it to:

- audit a repository and write `ISSUES.md` or SARIF;
- review a workspace or `git diff <target>` and write `REVIEW.md`;
- fix one reported finding, verify it, and rerun the review.

## The simple mental model

```text
oy selects and freezes the review input
  → your agent analyzes it with your model and permissions
  → oy validates and writes the report
```

This prevents silent model-selected sampling and makes the reviewed input visible. Findings are still model-generated and can vary.

## Start here

```bash
curl -fsSL https://oy.adonm.dev/install.sh | sh
# Choose global or current-workspace mise installation when prompted.
oy doctor --check

cd your-repository
# Ask your agent: "audit this repository with the oy-audit skill"
```

Then try:

```text
"review the diff against main with the oy-review skill"
"fix audit-0123456789abcdef with the oy-enhance skill"
```

See [Getting started](getting-started.md) for manual installation, the `oy-setup` skill, mise scope, and global versus project-local skills.

## What oy owns

- gitignore-aware repository and target-diff collection;
- ordered evidence files and explicit coverage limits;
- changed-input and artifact-integrity checks;
- normalized Markdown/SARIF reports with stable finding IDs;
- skill installation and legacy OpenCode plugin migration.

## What your agent owns

- models and provider credentials;
- permissions and approvals;
- shell, edit, web, and other tools;
- sessions, UI, and model execution.

The skills run under your agent's own permission model and never broaden it. `oy` is not a sandbox; see the [security policy](https://github.com/adonm/oy-cli/blob/main/SECURITY.md).

## Choose your next page

- [Getting started](getting-started.md) — install and create a first report
- [Workflow guide](workflows.md) — choose scope, understand findings, and remediate
- [Examples and CI](examples.md) — inspect reports and upload SARIF
- [CLI reference](reference.md) — exact commands, setup behavior, and environment variables
- [Compatibility](compatibility.md) — supported platforms and agent hosts
- [Security policy](https://github.com/adonm/oy-cli/blob/main/SECURITY.md) — trust and disclosure boundaries
