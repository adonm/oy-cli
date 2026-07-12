# Examples and CI

The reports below are illustrative. Paths, IDs, wording, and findings depend on the reviewed code and selected model.

## Audit with one finding

```bash
oy audit "authentication boundaries"
```

Shortened `ISSUES.md`:

````markdown
# Audit Issues

> Generated with [oy-cli](https://crates.io/crates/oy-cli): `oy audit 'authentication boundaries'` · 2026-07-13

## Findings summary

- `audit-2a71...` **High** `src/auth.rs:84` — Session lookup accepts an unscoped tenant ID _(status: new; fix: `oy enhance audit-2a71...`)_

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

The Markdown is for people; the JSON block preserves finding IDs and state for reruns and `oy enhance`.

## Target-diff review

```bash
oy review main --focus "types and boundaries"
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

## Successful no-findings review

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

A successful no-findings report still has generated metadata and an empty JSON array. A failed run exits nonzero or does not finalize the report.

## Fix and confirm

```bash
oy enhance audit-2a71...
# Inspect the source diff and verification output, then rerun:
oy audit "authentication boundaries"
```

The second audit should drop the finding if it no longer applies or update its lifecycle state from current evidence.

## SARIF

```bash
oy audit --format sarif --out oy.sarif
```

oy writes SARIF 2.1.0 with normalized rules, locations, severity, and provenance. Inspect it before upload, especially when repository paths or finding text are sensitive.

## GitHub code scanning

Provider-backed audits need OpenCode credentials. Use protected secrets and do not expose privileged credentials to untrusted pull-request code.

```yaml
permissions:
  contents: read
  security-events: write

steps:
  - uses: actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0 # v7.0.0
  - uses: jdx/mise-action@e6a8b3978addb5a52f2b4cd9d91eafa7f0ab959d # v4.2.0

  - name: Install oy and OpenCode
    run: |
      cargo install oy-cli --locked --version 0.13.6
      mise use --global --yes --minimum-release-age 0 node@24 npm:@opencode-ai/cli@next
      oy setup

  - name: Run audit
    env:
      # Replace with the environment variable required by your OpenCode provider.
      PROVIDER_API_KEY: ${{ secrets.PROVIDER_API_KEY }}
    run: oy audit --format sarif --out oy.sarif

  - name: Upload SARIF
    if: always() && hashFiles('oy.sarif') != ''
    uses: github/codeql-action/upload-sarif@v4
    with:
      sarif_file: oy.sarif
```

Pin versions in production CI and configure the provider according to the [OpenCode provider guide](https://v2.opencode.ai/providers). oy does not upload reports itself.
