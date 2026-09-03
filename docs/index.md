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

## Start here — 3 steps

**1. Install**

```bash
curl -fsSL https://oy.adonm.dev/install.sh | sh
oy doctor --check   # expect "global skills ok"
```

**2. Finish setup in your agent** — copy-paste:

```text
run the oy-setup skill to finish setup
```

The skill checks that your agent can see `oy-audit`, `oy-review`, `oy-enhance`, and copies them to your host's preferred location if needed (for example `.claude/skills`).

**3. Create your first report**

```text
audit this repository with the oy-audit skill
```

Look for `ISSUES.md` in the workspace root. Then try:

```text
review the diff against main with the oy-review skill
use the oy-enhance skill to fix audit-0123456789abcdef
```

> **First time?** Follow the full walkthrough in [Getting started](getting-started.md) — it explains what each step does and what to do if something fails.

## New to Agent Skills?

Agent Skills are plain Markdown files (`SKILL.md`). `oy setup` writes four of them to `~/.agents/skills/`:

- `oy-audit`, `oy-review`, `oy-enhance` — the workflows
- `oy-setup` — verifies installation and skill discovery

Your agent loads the matching SKILL.md when you mention it. No extra daemon, no API keys stored by oy.

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

- [Getting started](getting-started.md) — install and create a first report (start here if you're new)
- [Workflow guide](workflows.md) — choose scope, understand findings, and remediate
- [Examples and CI](examples.md) — inspect reports and upload SARIF
- [Troubleshooting](troubleshooting.md) — fix the 6 most common first-run problems
- [CLI reference](reference.md) — exact commands, setup behavior, and environment variables
- [Compatibility](compatibility.md) — supported platforms and agent hosts
- [Security policy](https://github.com/adonm/oy-cli/blob/main/SECURITY.md) — trust and disclosure boundaries
