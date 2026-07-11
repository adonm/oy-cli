# Workflow guide

Use the same bounded evidence protocol for whole-repository audits, target-diff reviews, and report-driven remediation.

## The workflow contract

1. oy binds the run/session/model/scope/output/format/chunk context and resolves a review target to its commit OID.
2. It creates stable, ordered chunks and checks the maximum chunk budget.
3. MCP verifies stable input and requires every collected chunk in order before rendering.
4. The restricted agent follows the canonical skill; the renderer rebinds report metadata and normalizes the final report.
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

Free-form text is a lens. A focus that exactly names a workspace-relative file or directory becomes the collection scope. Markdown defaults to `ISSUES.md`; SARIF defaults to `oy.sarif`.

The auditor has access to repository collection, the previous audit report, optional Sighthound candidates, and the report renderer. It has no generic read, search, edit, shell, or web tools. Sighthound runs only when the focus explicitly requests it, for example `oy audit "include Sighthound SAST"`; its output is candidate evidence and still requires source confirmation.

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

The focus is positional. The default enhancer reads `ISSUES.md`/`REVIEW.md`, chooses one actionable finding, allows focused edits, denies shell, and verifies through available non-shell checks. `--interactive` delegates the same bound workflow to OpenCode `mini` for native permission prompts, questions, and forms. Use `--mode edit` for edit-plus-ask shell behavior or `--mode auto` only in a trusted workspace when both edits and shell are intentionally pre-approved.

Then rerun the originating workflow:

```bash
oy audit                 # confirm an audit finding
oy review main           # confirm a target-review finding
```

## Coverage and failure limits

The default maximum is 80 chunks. MCP rejects a bound input above that limit before returning a usable summary, so the agent cannot sample around it. You can increase `--max-chunks`, but path scope is usually cheaper and easier to evaluate.

The repository collector omits:

- gitignored and hidden paths;
- `.git`, `target`, `node_modules`, `.venv`, and `.tmp` content;
- common lockfiles, generated reports, likely secrets, and private-key formats;
- binary, non-UTF-8, empty, unreadable, and larger-than-512-KiB repository files.

> These exclusions reduce accidental disclosure and context waste, but they also limit completeness. In particular, current lockfile exclusion means an oy audit is not a complete supply-chain audit.

Eligible large files and diff evidence are split deterministically before chunking. Raw chunk text remains below 40 KiB and 3,000 lines, leaving JSON-escaping headroom under the host result limit; bound workflows use fixed transport-safe sizing instead of a model-requested `target_tokens` increase.

Sighthound does not consume oy's collected file list. It uses independent gitignore-aware discovery, common directory filters, and its own file-size limit (currently 10 MiB), so an explicitly requested scan may inspect supported hidden source or source files omitted by oy's 512 KiB limit. Treat returned snippets as additional model-provider disclosure.

## Practical guidance

- Use a model with strong tool use and enough context for a 64,000-token chunk plus prompt/report overhead.
- Pin noninteractive workflow comparisons with `OY_OPENCODE_MODEL=provider/model#variant` when model variants matter.
- Start with default limits; narrow by path before raising limits on very large repositories.
- Do not put secrets under the workspace root solely because likely-secret names are skipped.
- Treat SAST and model findings as candidates until a human verifies the evidence and impact.
- Capture opencode version, model, oy version, target, and scope when comparing reports.

See [Examples and CI](examples.md) for representative output and SARIF upload configuration.
