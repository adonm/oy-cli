# Workflow guide

Use the same bounded evidence protocol for whole-repository audits, target-diff reviews, and report-driven remediation.

## The workflow contract

1. The outer workflow selects the OpenCode session and model; `oy audit|review prepare` binds the workspace, scope, output, format, chunk limit, and resolved review target OID. Preparation records a model only when `OY_OPENCODE_MODEL` is explicitly set.
2. It creates stable ordered chunks, checks the maximum chunk budget, writes `.oy/runs/<run-id>/index.json`, and stores authoritative hashes outside the workspace.
3. The `oy` agent follows the canonical skill under the user's OpenCode permissions, reads every page of every indexed chunk with native tools, and writes separate candidate Markdown and findings JSON.
4. `oy audit|review finalize` rejects changed evidence, artifacts, output, or malformed findings before normalizing the final report.
5. A later run reads the prior generated report once to carry forward only current findings.

> **Determinism stops at inference.** Collection and rendering are deterministic; finding quality and prose depend on the selected model.

## Security audit

```bash
oy audit
oy audit "authentication and authorization"
oy audit src/auth
oy audit --out reports/security.md --max-chunks 60
oy audit --format sarif --out oy.sarif
```

Free-form text is a lens. A single focus argument that exactly names a workspace-relative file or directory becomes the collection scope. Markdown defaults to `ISSUES.md`; SARIF defaults to `oy.sarif`.

The auditor executes under the user's normal OpenCode permissions. The deterministic collector and finalizer do not grant shell, edit, web, or network capability; OpenCode remains authoritative for those tools.

## Code-quality review

```bash
oy review
oy review main
oy review HEAD~3 --focus "error handling"
oy review main --out reports/change-review.md
```

Omit the target to review the collected workspace. Supply a branch, commit, tag, or other ref to review the current workspace against deterministic `git diff <target>` input. The default output is `REVIEW.md`.

The reviewer emphasizes high-conviction structural findings: unnecessary complexity, unclear state or ownership, weak boundaries/types, dependency cost, and files that need meaningful decomposition. It should report no major concern rather than fill space.

## Reports and finding lifecycle

Rendered Markdown includes a human summary plus a machine-readable `oy-findings` JSON block. Each normalized finding has:

- a stable ID derived from source, severity, title, and primary location when the model did not provide one;
- a normalized status such as `new`, `carried-forward`, `fixed?`, or `stale`;
- path, line, and symbol evidence when available;
- short evidence and remediation context.

Each new audit/review supersedes the old generated report. Current findings may be carried forward; stale findings should be dropped. Keep reports under version control only if that matches your team's disclosure policy.

## Remediate one finding

```bash
oy enhance audit-0123456789abcdef
oy enhance "the highest-severity actionable finding"
oy enhance --review-target main review-0123456789abcdef
oy enhance --interactive review-0123456789abcdef
```

The focus is positional. The `oy` agent reads `ISSUES.md`/`REVIEW.md`, chooses one actionable finding, makes the smallest focused change allowed by the user's OpenCode policy, and runs the narrowest available verification. `--interactive` delegates the same bound workflow to OpenCode `mini` for native permission prompts, questions, and forms. Oy does not define a separate remediation permission mode.

Then rerun the originating workflow:

```bash
oy audit                 # confirm an audit finding
oy review main           # confirm a target-review finding
```

## Coverage and failure limits

The default maximum is 80 chunks. Preparation rejects an input above that limit before writing a usable index, so the agent cannot sample around it. You can increase `--max-chunks`, but path scope is usually cheaper and easier to evaluate.

The repository collector omits:

- gitignored and hidden paths;
- `.git`, `.oy`, `target`, `node_modules`, `.venv`, and `.tmp` content;
- common lockfiles, generated reports, likely secrets, and private-key formats;
- binary, non-UTF-8, empty, unreadable, and larger-than-512-KiB repository files.

> These exclusions reduce accidental disclosure and context waste, but they also limit completeness. In particular, current lockfile exclusion means an oy audit is not a complete supply-chain audit.

Eligible large files and diff evidence are split deterministically before chunking. Each artifact stays bounded for practical native `read` paging; preparation uses fixed sizing instead of a model-requested `target_tokens` increase.

## Practical guidance

- Use a model with strong tool use and enough context for a 64,000-token chunk plus prompt/report overhead.
- Pin noninteractive workflow comparisons with `OY_OPENCODE_MODEL=provider/model#variant` when model variants matter.
- Start with default limits; narrow by path before raising limits on very large repositories.
- Do not put secrets under the workspace root solely because likely-secret names are skipped.
- Treat model findings as candidates until a human verifies the evidence and impact.
- Capture opencode version, model, oy version, target, and scope when comparing reports.

See [Examples and CI](examples.md) for representative output and SARIF upload configuration.
