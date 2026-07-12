# Audits and code reviews for OpenCode

`oy` adds a focused coding agent and a repeatable review workflow to [OpenCode 2](https://v2.opencode.ai/).

Use it to:

- audit a repository and write `ISSUES.md` or SARIF;
- review a workspace or `git diff <target>` and write `REVIEW.md`;
- fix one reported finding, verify it, and rerun the review.

## The simple mental model

```text
oy selects and freezes the review input
  → OpenCode analyzes it with your model and permissions
  → oy validates and writes the report
```

This prevents silent model-selected sampling and makes the reviewed input visible. Findings are still model-generated and can vary.

## Start here

```bash
curl -fsSL https://oy.adonm.dev/install.sh | sh
# Restart your shell if requested.
oy doctor --check

cd your-repository
oy audit
```

Then try:

```bash
oy review main
oy enhance <finding-id>
```

See [Getting started](getting-started.md) for manual installation, provider setup, and global versus project-local configuration.

## What oy owns

- gitignore-aware repository and target-diff collection;
- ordered evidence files and explicit coverage limits;
- changed-input and artifact-integrity checks;
- normalized Markdown/SARIF reports with stable finding IDs.

## What OpenCode owns

- models and provider credentials;
- permissions and approvals;
- shell, edit, web, and other tools;
- sessions, the TUI, and model execution.

`oy` does not broaden your OpenCode permissions and is not a sandbox.

## Choose your next page

- [Getting started](getting-started.md) — install and create a first report
- [Workflow guide](workflows.md) — choose scope, understand findings, and remediate
- [Examples and CI](examples.md) — inspect reports and upload SARIF
- [CLI reference](reference.md) — exact commands, setup behavior, and environment variables
- [Compatibility](compatibility.md) — supported platforms and OpenCode versions
- [Security policy](https://github.com/adonm/oy-cli/blob/main/SECURITY.md) — trust and disclosure boundaries
