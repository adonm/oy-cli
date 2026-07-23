# oy

[![Crates.io](https://img.shields.io/crates/v/oy-cli.svg)](https://crates.io/crates/oy-cli)
[![docs.rs](https://docs.rs/oy-cli/badge.svg)](https://docs.rs/oy-cli)

**Focused OpenCode and Cursor integrations with repeatable audits, code reviews, and one-finding fixes.**

`oy` helps OpenCode or Cursor review a repository without quietly choosing a small sample. It prepares an ordered, reviewable set of files, lets the model analyze them under the host's existing permissions, and verifies the report before writing it.

## What you get

- `oy audit` for security-focused repository audits (`ISSUES.md` or SARIF)
- `oy review` for whole-workspace or target-diff code reviews (`REVIEW.md`)
- `oy enhance` for fixing one reported finding at a time
- one concise `oy` coding agent plus `/oy-audit`, `/oy-review`, and `/oy-enhance` inside OpenCode or Cursor

OpenCode or Cursor still owns models, credentials, permissions, sessions, edits, shell commands, and web access. `oy` adds the evidence and report workflow; it is not a second agent runtime or permission system.

## Quick start

Requirements: Linux or macOS (WSL2 on Windows), `oy` on `PATH`, and either a supported OpenCode 2 or Cursor installation with a configured model provider.

```bash
# OpenCode 2 (default)
curl -fsSL https://oy.adonm.dev/install.sh | sh
# Restart your shell if the installer asks you to.
oy doctor --check

cd your-repository
oy audit
```

```bash
# Cursor CLI plus the global oy Cursor integration
curl -fsSL https://oy.adonm.dev/install.sh | sh -s -- --cursor

# Or install both hosts and integrations
curl -fsSL https://oy.adonm.dev/install.sh | sh -s -- --both
```

The installer uses [mise](https://mise.jdx.dev/) for prebuilt oy and context helpers. The OpenCode target provisions Node.js and OpenCode's documented npm package; the Cursor target uses Cursor's official CLI installer because Cursor has no official mise package. [Review the installer](https://oy.adonm.dev/install.sh) before piping it to a shell.

Prefer a manual install or project-local setup? See [Getting started](https://oy.adonm.dev/getting-started.html).

## Common workflows

### Audit a repository

```bash
oy audit                                  # writes ISSUES.md
oy audit src/auth                         # audit one path
oy audit "authentication boundaries"     # apply a focus lens
oy audit --format sarif --out oy.sarif
```

A single argument that exactly matches a workspace path narrows collection. Other text is treated as review guidance.

### Review code

```bash
oy review                                 # review the collected workspace
oy review main                            # review git diff main
oy review main --focus "error handling"
```

A branch, commit, tag, or ref selects target-diff review. Without a target, oy reviews the workspace.

### Fix one finding

```bash
oy enhance review-0123456789abcdef
# Confirm the result by rerunning the originating workflow.
oy review main
```

Reports include stable finding IDs. `oy enhance` confirms the cited source, makes one focused fix, and runs the narrowest available verification.

You can run the same workflows inside OpenCode or Cursor with `/oy-audit`, `/oy-review`, and `/oy-enhance`.

## How repeatable review works

1. **Prepare:** oy collects eligible repository text or a Git diff into ordered files under `.oy/runs/`.
2. **Review:** the selected agent host reads every prepared chunk and writes a candidate report.
3. **Verify:** oy rejects changed inputs, modified evidence, concurrent report changes, or malformed findings.
4. **Finalize:** oy writes normalized Markdown or SARIF with stable finding metadata.

> **The inputs are deterministic; the conclusions are not.** Model choice and prompt quality still affect findings.

“Every chunk” means every chunk collected by oy, not every byte in the repository. The collector excludes ignored/hidden paths, dependencies and build output, lockfiles, likely secrets, binary or unreadable files, and files larger than 512 KiB. See [Coverage and limits](https://oy.adonm.dev/workflows.html#coverage-and-limits) before using a report for high-assurance work.

## Safety

`oy` is not a sandbox. Prepared source may be sent to your configured model provider, and the `oy` agent uses your effective host permissions. Use a disposable environment for untrusted repositories and read [SECURITY.md](SECURITY.md).

## Documentation

- [Getting started](https://oy.adonm.dev/getting-started.html) — install, configure, and create a first report
- [Workflow guide](https://oy.adonm.dev/workflows.html) — scopes, findings, remediation, and limits
- [Examples and CI](https://oy.adonm.dev/examples.html) — report examples and SARIF upload
- [CLI reference](https://oy.adonm.dev/reference.html) — commands, environment variables, and setup ownership
- [Compatibility](https://oy.adonm.dev/compatibility.html) — supported platforms and agent hosts
- [Architecture](https://oy.adonm.dev/architecture.html) and [contributing](CONTRIBUTING.md) — maintainer documentation

Run `oy <command> --help` for the installed version's exact flags.
