# Workflow guide

> **First time?** Start with one line to your agent: `audit this repository with the oy-audit skill` — then come back here to understand scope, findings, and the fix loop.

| Goal | How to run it (copy-paste to your agent) | Output |
|---|---|---|
| Security audit | `audit this repository with the oy-audit skill` | `ISSUES.md` |
| Code-quality review | `review this repository with the oy-review skill` | `REVIEW.md` |
| Review current work against a ref | `review the diff against main with the oy-review skill` | `REVIEW.md` |
| Fix one finding | `use the oy-enhance skill to fix audit-0123456789abcdef` | source changes |

Your agent picks the matching skill from `.agents/skills` (or `~/.agents/skills`) and runs its protocol: prepare evidence, read every chunk, write candidates, and finalize the report.

## Your first audit — step by step

1. **Pick a small folder** to start fast and avoid the 80-chunk limit:
   ```text
   audit src/auth with the oy-audit skill
   ```
2. **Open `ISSUES.md`** in the workspace root. You'll see:
   - a header with the `oy audit prepare` command and date
   - **Findings summary** — one line per finding with ID, severity, and file location
   - **Detailed findings** — evidence, impact, and fix guidance
   - **Machine-readable block** — `oy-findings` JSON with stable IDs for reruns
3. **Treat findings as candidates** until you confirm the evidence and impact — they are model-generated.
4. **Rerun after a fix** — the next audit carries forward still-current findings, marks others `stale` or `fixed?`, and keeps IDs stable.

## Choose what to review

### Audit scope and focus

When you ask for an audit, oy interprets your words two ways:

| You say | What oy does |
|---|---|
| `audit src/auth` | **Narrows collection** — only files under `src/auth` are collected (fast, precise) |
| `audit the authentication boundaries` | **Guides the model** — collects the whole repo, but tells the model to focus on auth |
| `audit src/auth focusing on session handling` | Both — narrow collection *and* focus guidance |

> **Rule:** a single existing workspace-relative path becomes the collection scope. All other text guides the model without narrowing collection.

**Examples:**

```text
audit this repository
audit src/auth
audit the authentication boundaries
audit src/api focusing on input validation
```

### Review scope and focus

```text
review this repository
review the diff against main
review the diff against HEAD~3, focusing on error handling
```

- A branch, commit, tag, or ref selects **target-diff review** — only the diff is collected (great for PRs).
- Omit the target for a **whole-workspace review**.
- Add focus text (e.g. "focusing on error handling") to guide the reviewer without changing scope.

Review findings are intentionally sparse. The reviewer prefers concrete structural issues — unclear ownership, unnecessary complexity, weak boundaries/types, expensive dependencies, or files near 1000 lines needing decomposition — over generic advice.

### Path vs focus — cheat sheet

```text
# path (narrows what oy collects)     vs     focus (guides what model looks for)
audit src/auth                                 audit focusing on auth
review src/cli                                 review focusing on error handling
# combine both:
audit src/auth focusing on session handling
review the diff against main focusing on types and boundaries
```

## Read a report

Markdown reports contain:

- a verdict or summary;
- detailed evidence-backed findings;
- a machine-readable `oy-findings` JSON block;
- generation and evidence metadata (digest, chunk count, date).

Each finding has a **stable ID** (`audit-...` or `review-...`), **severity** (High/Medium/Low/Info), **status** (`new`, `carried-forward`, `fixed?`, `stale`), location when available, evidence, and remediation guidance.

A **no-findings report is success**, not failure — check the exit status and metadata to tell the difference. It will have an empty JSON array and a verdict like "No major structural concerns."

## Fix and confirm one finding

Ask your agent to fix a finding with the `oy-enhance` skill, ideally by ID:

```text
use the oy-enhance skill to fix audit-0123456789abcdef
use the oy-enhance skill to fix review-0123456789abcdef
```

The skill reads `ISSUES.md` or `REVIEW.md`, confirms the cited source in code, fixes **one** actionable finding with the smallest correct change, and runs focused verification. It never broadens permissions; if a needed check is denied, it reports the remaining check.

Then **rerun the originating audit or review**. The new report replaces the old one, carries forward findings that still apply, and drops stale ones. Use the stable ID to track a finding across reruns.

## Hit the chunk limit?

The default limit is **80 evidence chunks**. If preparation exceeds it, oy **fails instead of silently sampling** — this is intentional.

**Fix it in order:**

1. **Narrow the path first** (best):
   ```text
   audit src/auth with the oy-audit skill
   audit src/api with the oy-audit skill
   ```
2. **Raise `--max-chunks` only when the broader scope is intentional** — your agent can pass it via the skill, or you can call `oy audit prepare --max-chunks 120` directly for automation.

Large files and diffs are split into bounded chunks automatically; you don't need to chunk manually.

## What happens under the hood

Audit and review follow four stages:

1. **Prepare** — collect eligible workspace files or a target diff into ordered chunks under `.oy/runs/<run-id>/`.
2. **Review** — the agent reads every prepared chunk under your current model and permissions.
3. **Verify** — reject changed inputs, modified evidence, concurrent output changes, or malformed finding data.
4. **Finalize** — write normalized Markdown or SARIF.

> Collection and report normalization are deterministic. Model findings and prose are not.

Advanced automation can call `oy audit|review prepare` and `finalize` directly; most users should just use the skills.

## Coverage and limits

The workspace collector excludes:

- gitignored and hidden paths;
- `.git`, `.oy`, `target`, `node_modules`, `.venv`, and `.tmp`;
- common lockfiles, generated reports, likely secrets, and private-key formats;
- binary, non-UTF-8, empty, unreadable, and larger-than-512-KiB files.

These exclusions reduce accidental disclosure and context waste, but they also limit completeness. In particular, an oy audit is **not a complete supply-chain audit** because lockfiles are excluded.

Eligible large files and diffs are split into bounded chunks. Prepared source may be sent to the model provider configured in your agent — see [SECURITY.md](https://github.com/adonm/oy-cli/blob/main/SECURITY.md).

## Practical guidance

- Begin with a small scope and inspect the first report before going wider.
- Narrow by path before raising `--max-chunks`.
- Use a model with reliable tool use and sufficient context.
- Do not rely on secret-like filenames as a security boundary.
- Keep generated reports private when their findings or paths are sensitive.
- Pin `OY_OPENCODE_MODEL=provider/model#variant` when comparing repeat runs.
- Prefer one finding per `oy-enhance` pass — fix, verify, rerun.

See [Examples and CI](examples.md) for representative output and SARIF upload, and [Troubleshooting](troubleshooting.md) if a workflow fails.
