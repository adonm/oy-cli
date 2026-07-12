# Repeatable repository audits and reviews for opencode

`oy` gives [opencode](https://opencode.ai/) users one concise autonomous agent and a bounded path from deterministic repository inputs to security audits, code-quality reviews, and focused remediation.

## Why oy

### Visible coverage

Gitignore-aware manifests and ordered chunks replace silent, model-selected sampling. Oversized runs fail closed.

### Restricted review

Three canonical skills define audit, review, and remediation and execute through the single `oy` agent under the user's OpenCode permissions. Audit/review use file-backed CLI preparation and verified finalization; MCP remains only as an unregistered compatibility adapter.

### Reports that survive chat

Markdown and SARIF reports carry stable finding IDs and statuses into reruns and one-finding remediation.

## Quick start

```bash
curl -fsSL https://oy.adonm.dev/install.sh | sh
# Restart or activate your shell as instructed, then:
oy doctor
oy audit
```

The full installer uses [mise](https://mise.jdx.dev/) to install pinned oy 0.13.0, OpenCode 2 beta `0.0.0-next-15353`, and local evidence helpers; it verifies versions, prunes unreferenced old installs, and resets generated integration before global setup. [Review the installer](install.sh) before piping it to a shell.

For a minimal manual install:

```bash
mise use --global node@24 cargo-binstall cargo:oy-cli@0.13.0 npm:@opencode-ai/cli@0.0.0-next-15353
oy setup
oy audit
```

Continue with [Getting started](getting-started.md) or go directly to the [workflow guide](workflows.md).

## One focused loop

1. **Audit:** `oy audit` writes `ISSUES.md` or SARIF.
2. **Review:** `oy review main` writes `REVIEW.md` for a target diff.
3. **Remediate:** `oy enhance <finding-id>` fixes and verifies one finding.
4. **Confirm:** rerun the originating audit or review to update its status.

## A precise claim

> **Deterministic inputs, not deterministic conclusions.**

oy owns collection, ordering, limits, and report rendering. opencode owns model execution, and model findings vary by model and prompt. “Every chunk” means every collected chunk; [documented exclusions](workflows.md#coverage-and-failure-limits) still apply.

## Where to go next

- [Install and configure oy](getting-started.md)
- [Understand audit, review, and remediation](workflows.md)
- [See representative reports and CI integration](examples.md)
- [Look up CLI/package commands, compatibility MCP tools, and environment variables](reference.md)
- [Check supported and tested environments](compatibility.md)
- [Read the project direction](project.md)
- [Browse the Rust API](https://docs.rs/oy-cli)
