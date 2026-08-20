# Workflow guide

Start with the task you want to perform. The lower-level evidence protocol is automatic.

| Goal | How to run it | Default output |
|---|---|---|
| Security audit | Ask your agent to audit with the `oy-audit` skill | `ISSUES.md` |
| Code-quality review | Ask your agent to review with the `oy-review` skill | `REVIEW.md` |
| Review current work against a ref | "Review the diff against `main`" | `REVIEW.md` |
| Fix one finding | Ask your agent to use the `oy-enhance` skill targeting the finding ID | source changes |

Your agent picks the matching skill from `.agents/skills` (or
`~/.agents/skills`) and runs its protocol: prepare evidence, read every
chunk, write candidates, and finalize the report.

## Choose what to review

### Audit scope and focus

When you ask for an audit, name the scope:

```text
audit this repository
audit src/auth
audit the authentication boundaries
```

A single existing workspace-relative path becomes the collection scope.
Other text guides the model without narrowing collection.

### Review scope and focus

```text
review this repository
review the diff against main
review the diff against HEAD~3, focusing on error handling
```

A branch, commit, tag, or ref selects target-diff review. Focus text adds review guidance.

Review findings are intentionally sparse. The reviewer prefers concrete structural issues—unclear ownership, unnecessary complexity, weak boundaries/types, expensive dependencies, or files needing meaningful decomposition—over generic advice.

## Read a report

Markdown reports contain:

- a verdict or summary;
- detailed evidence-backed findings;
- a machine-readable `oy-findings` JSON block;
- generation and evidence metadata.

Each finding has a stable ID, severity, status, location when available, evidence, and remediation guidance. Common statuses are `new`, `carried-forward`, `fixed?`, and `stale`.

A no-findings report is a successful result, not a failed run. Check the command exit status and generated metadata when distinguishing the two.

Treat findings as candidates until a person confirms the evidence and impact.

## Fix and confirm one finding

Ask your agent to fix a finding with the `oy-enhance` skill, ideally by ID:

```text
use the oy-enhance skill to fix audit-0123456789abcdef
use the oy-enhance skill to fix review-0123456789abcdef
```

The skill reads `ISSUES.md` or `REVIEW.md`, confirms the cited source, fixes
one actionable finding, and runs focused verification. It never broadens
permissions; if a needed check is denied, it reports the remaining check.

Then rerun the originating audit or review. The new report replaces the old generated report, carries forward findings that still apply, and drops stale ones.

## What happens under the hood

Audit and review follow four stages:

1. **Prepare** — collect eligible workspace files or a target diff into ordered chunks under `.oy/runs/<run-id>/`.
2. **Review** — the agent reads every prepared chunk under your current model and permissions.
3. **Verify** — reject changed inputs, modified evidence, concurrent output changes, or malformed finding data.
4. **Finalize** — write normalized Markdown or SARIF.

> Collection and report normalization are deterministic. Model findings and prose are not.

Advanced automation can call `oy audit|review prepare` and `finalize` directly; most users should not need those commands.

## Coverage and limits

The default limit is 80 evidence chunks. If preparation exceeds it, oy fails instead of silently sampling. Narrow the path first; increase `--max-chunks` only when the broader scope is intentional.

The workspace collector excludes:

- gitignored and hidden paths;
- `.git`, `.oy`, `target`, `node_modules`, `.venv`, and `.tmp`;
- common lockfiles, generated reports, likely secrets, and private-key formats;
- binary, non-UTF-8, empty, unreadable, and larger-than-512-KiB files.

These exclusions reduce accidental disclosure and context waste, but they also limit completeness. In particular, an oy audit is not a complete supply-chain audit because lockfiles are excluded.

Eligible large files and diffs are split into bounded chunks. Prepared source may be sent to the model provider configured in your agent.

## Practical guidance

- Begin with a small scope and inspect the first report.
- Narrow by path before raising `--max-chunks`.
- Use a model with reliable tool use and sufficient context.
- Do not rely on secret-like filenames as a security boundary.
- Keep generated reports private when their findings or paths are sensitive.
- Pin `OY_OPENCODE_MODEL=provider/model#variant` when comparing repeat runs.

See [Examples and CI](examples.md) for representative output and SARIF upload.
