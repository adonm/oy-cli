# Examples and CI

> **New?** Start with `audit this repository with the oy-audit skill` — the snippets below show what the generated reports look like so you know what to expect.

The reports below are illustrative. Paths, IDs, wording, and findings depend on the reviewed code and selected model.

## Audit with one finding

Prompt to your agent:

```text
audit this repository with the oy-audit skill, focusing on authentication boundaries
```

Shortened `ISSUES.md` (look for this file in your workspace root after the run):

````markdown
# Audit Issues

> Generated with [oy-cli](https://crates.io/crates/oy-cli): `oy audit prepare --focus 'authentication boundaries'` · 2026-07-13

## Findings summary

- `audit-2a71...` **High** `src/auth.rs:84` — Session lookup accepts an unscoped tenant ID _(status: new; fix: use the `oy-enhance` skill targeting `audit-2a71...`)_

## Detailed findings

### [High] Session lookup accepts an unscoped tenant ID

Evidence: `src/auth.rs:84` uses the caller-provided tenant before authorization.

## Machine-readable findings

```json oy-findings
[
  {
    "id": "audit-2a71...",
    "status": "new",
    "source": "audit",
    "severity": "High",
    "title": "Session lookup accepts an unscoped tenant ID",
    "locations": [{"path": "src/auth.rs", "line": 84}],
    "evidence": "Caller-provided tenant reaches session lookup before authorization.",
    "body": "Bind tenant scope to the authenticated principal before lookup.",
    "category": "access-control"
  }
]
```
````

The Markdown is for people; the JSON block preserves finding IDs and state for reruns and the `oy-enhance` skill. Copy the ID (e.g. `audit-2a71...`) to fix it:

```text
use the oy-enhance skill to fix audit-2a71...
```

## Target-diff review

Great for pull requests — only the changed code is reviewed.

```text
review the diff against main with the oy-review skill, focusing on types and boundaries
```

Shortened `REVIEW.md`:

````markdown
# Code Quality Review

## Verdict

Needs work.

## Findings summary

- **Medium** — Two structs represent the same persisted state (`src/cli/config.rs:41`).

## Detailed findings

### [Medium] Two structs represent the same persisted state

Both structs are serialized independently. Keep one persisted representation and convert at the boundary.

## Machine-readable findings

```json oy-findings
[{"id":"review-7bd1...","status":"new","source":"review","severity":"Medium","title":"Two structs represent the same persisted state","locations":[{"path":"src/cli/config.rs","line":41}],"evidence":"Both structs are serialized independently.","body":"Keep one persisted representation.","category":"state-ownership"}]
```
````

No target → whole workspace is reviewed. With a target (`main`, `HEAD~3`, a commit SHA), only that diff is collected.

## Successful no-findings review

This is success, not failure — the run passed and nothing high-conviction was found:

````markdown
# Code Quality Review

## Verdict

No major structural concerns.

## Findings summary

No high-conviction findings.

## Machine-readable findings

```json oy-findings
[]
```
````

A failed run exits nonzero or does not finalize the report at all. Check the CLI exit status and the header metadata (digest, chunk count) to tell the two apart.

## Fix and confirm

```text
use the oy-enhance skill to fix audit-2a71...
# Inspect the source diff and verification output, then rerun the same audit:
# "audit this repository with the oy-audit skill, focusing on authentication boundaries"
```

The second audit should drop the finding if it no longer applies or update its lifecycle state (`new` → `fixed?` → `stale` → gone) from current evidence. Always rerun the originating workflow to confirm.

## SARIF

The `oy-audit` skill writes SARIF when you ask for sarif output:

```text
audit this repository with the oy-audit skill, using sarif format and writing oy.sarif
```

The CLI normalizes SARIF 2.1.0 with rules, locations, severity, and provenance. Inspect it before upload, especially when repository paths or finding text are sensitive.

## GitHub code scanning

Agent-backed audits need the agent and its provider configured in CI. Use protected secrets and do not expose privileged credentials to untrusted pull-request code.

```yaml
permissions:
  contents: read
  security-events: write

steps:
  - uses: actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0 # v7.0.0
  - uses: jdx/mise-action@e6a8b3978addb5a52f2b4cd9d91eafa7f0ab959d # v4.2.0

  - name: Install oy
    run: |
      mise use --global --yes --minimum-release-age 0 github:adonm/oy-cli@0.15.2
      mise exec github:adonm/oy-cli@0.15.2 -- oy setup

  - name: Run audit with an agent
    env:
      # Replace with the environment variable required by your agent's provider.
      PROVIDER_API_KEY: ${{ secrets.PROVIDER_API_KEY }}
    run: |
      opencode2 run "Load the oy-audit skill and audit this repository with sarif output." --format json
      # Or use your CI's agent CLI (Cursor CLI, Codex, etc.) the same way.

  - name: Upload SARIF
    if: always() && hashFiles('oy.sarif') != ''
    uses: github/codeql-action/upload-sarif@v4
    with:
      sarif_file: oy.sarif
```

Pin versions in production CI and configure the provider in your agent of choice. oy does not upload reports itself.

## Tips for new users

- **Start narrow:** `audit src/auth` is faster and cheaper than `audit this repository` on a large codebase
- **Read the evidence:** every finding should cite a path/line/symbol — if it doesn't, treat it skeptically
- **One fix at a time:** let `oy-enhance` handle one ID, verify, rerun — don't batch unrelated fixes
- **Compare runs:** set `OY_OPENCODE_MODEL=provider/model#variant` to keep model choice explicit across reruns
