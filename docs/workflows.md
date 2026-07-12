# Workflow guide

Start with the task you want to perform. The lower-level evidence protocol is automatic.

| Goal | Command | Default output |
|---|---|---|
| Security audit | `oy audit` | `ISSUES.md` |
| Code-quality review | `oy review` | `REVIEW.md` |
| Review current work against a ref | `oy review main` | `REVIEW.md` |
| Fix one finding | `oy enhance <finding-id>` | source changes |

The same workflows are available inside OpenCode as `/oy-audit`, `/oy-review`, and `/oy-enhance`.

## Choose what to review

### Audit scope and focus

```bash
oy audit                                  # workspace
oy audit src/auth                         # one existing path
oy audit "authentication boundaries"     # focus text
oy audit --out reports/security.md
oy audit --format sarif --out oy.sarif
```

When the audit receives exactly one argument and it names an existing workspace-relative file or directory, that path becomes the collection scope. Other text guides the model without narrowing collection.

### Review scope and focus

```bash
oy review                                 # workspace
oy review main                            # git diff main
oy review HEAD~3 --focus "error handling"
oy review main --out reports/review.md
```

A positional branch, commit, tag, or ref selects target-diff review. `--focus` adds review guidance and can be repeated.

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

```bash
oy enhance audit-0123456789abcdef
oy enhance review-0123456789abcdef
```

`oy enhance` reads `ISSUES.md` or `REVIEW.md`, confirms the cited source, fixes one actionable finding, and runs focused verification. Use `--interactive` when you want OpenCode `mini` to show native permission prompts, questions, and forms:

```bash
oy enhance --interactive review-0123456789abcdef
```

Then rerun the originating command:

```bash
oy audit
oy review main
```

The new report replaces the old generated report, carries forward findings that still apply, and drops stale ones.

## What happens under the hood

Audit and review follow four stages:

1. **Prepare** — collect eligible workspace files or a target diff into ordered chunks under `.oy/runs/<run-id>/`.
2. **Review** — OpenCode reads every prepared chunk under your current model and permissions.
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

Eligible large files and diffs are split into bounded chunks. Prepared source may be sent to the model provider configured in OpenCode.

## Practical guidance

- Begin with a small scope and inspect the first report.
- Narrow by path before raising `--max-chunks`.
- Use a model with reliable tool use and sufficient context.
- Do not rely on secret-like filenames as a security boundary.
- Keep generated reports private when their findings or paths are sensitive.
- Pin `OY_OPENCODE_MODEL=provider/model#variant` when comparing repeat runs.

See [Examples and CI](examples.md) for representative output and SARIF upload.
