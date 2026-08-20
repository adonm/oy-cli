# oy

[![Crates.io](https://img.shields.io/crates/v/oy-cli.svg)](https://crates.io/crates/oy-cli)
[![docs.rs](https://docs.rs/oy-cli/badge.svg)](https://docs.rs/oy-cli)

**Deterministic-input audits, code reviews, and one-finding fixes as portable agent skills.**

`oy` helps your coding agent review a repository without quietly choosing a small sample. It prepares an ordered, reviewable set of files, lets your agent's model analyze them under its own permissions, and verifies the report before writing it. The workflows are standard Agent Skills that OpenCode, Cursor, Codex, Copilot, and Gemini CLI all discover from `.agents/skills`.

## What you get

- `oy-audit` skill — security-focused repository audits (`ISSUES.md` or SARIF)
- `oy-review` skill — whole-workspace or target-diff code reviews (`REVIEW.md`)
- `oy-enhance` skill — fix one reported finding at a time
- `oy-setup` skill — agent-driven setup, verification, and persona installation
- deterministic CLI: `oy audit|review prepare` and `finalize`, `oy setup`, `oy doctor`

Your agent owns models, credentials, sessions, and general tools. `oy` adds the evidence and report workflow; it is not a second agent runtime or permission system.

## Quick start

Requirements: Linux or macOS (WSL2 on Windows), `oy` on `PATH`, and any agent that reads Agent Skills.

> **Copy-paste for a new repo (30 seconds after install):**
> ```text
> run the oy-setup skill to finish setup
> audit this repository with the oy-audit skill
> ```

```bash
curl -fsSL https://oy.adonm.dev/install.sh | sh
# Choose global or current-workspace mise installation when prompted.
oy doctor --check

cd your-repository
# Ask your agent: "audit this repository with the oy-audit skill"
```

Use `--global` or `--workspace` to skip the mise scope prompt.

The installer uses [mise](https://mise.jdx.dev/) for prebuilt oy and the optional tokei/Universal Ctags context helpers, then runs `oy setup` to install the skills. [Review the installer](https://oy.adonm.dev/install.sh) before piping it to a shell.

Prefer a manual install or project-local setup? See [Getting started](https://oy.adonm.dev/getting-started.html).

Local dev from this checkout: `just install` (cargo install + `oy setup` + `oy doctor --check`).

## Common workflows

### Audit a repository

```text
audit this repository with the oy-audit skill
audit src/auth with the oy-audit skill
audit the authentication boundaries with the oy-audit skill
audit with sarif output and write oy.sarif
```

A single existing workspace path narrows collection. Other text is treated as review guidance.

### Review code

```text
review this repository with the oy-review skill
review the diff against main with the oy-review skill
review the diff against main with the oy-review skill, focusing on error handling
```

A branch, commit, tag, or ref selects target-diff review. Without a target, the skill reviews the workspace.

### Fix one finding

```text
use the oy-enhance skill to fix review-0123456789abcdef
```

Reports include stable finding IDs. The skill confirms the cited source, makes one focused fix, and runs the narrowest available verification. Rerun the originating audit or review to confirm.

## How repeatable review works

1. **Prepare:** oy collects eligible repository text or a Git diff into ordered files under `.oy/runs/`.
2. **Review:** your agent reads every prepared chunk and writes a candidate report.
3. **Verify:** oy rejects changed inputs, modified evidence, concurrent report changes, or malformed findings.
4. **Finalize:** oy writes normalized Markdown or SARIF with stable finding metadata.

> **The inputs are deterministic; the conclusions are not.** Model choice and prompt quality still affect findings.

“Every chunk” means every chunk collected by oy, not every byte in the repository. The collector excludes ignored/hidden paths, dependencies and build output, lockfiles, likely secrets, binary or unreadable files, and files larger than 512 KiB. See [Coverage and limits](https://oy.adonm.dev/workflows.html#coverage-and-limits) before using a report for high-assurance work.

## Safety

`oy` is not a sandbox. Prepared source may be sent to your configured model provider. The skills run under your agent's own permissions. Use a disposable environment for untrusted repositories and read [SECURITY.md](SECURITY.md).

## Documentation

- [Getting started](https://oy.adonm.dev/getting-started.html) — install, configure, and create a first report
- [Workflow guide](https://oy.adonm.dev/workflows.html) — scopes, findings, remediation, and limits
- [Examples and CI](https://oy.adonm.dev/examples.html) — report examples and SARIF upload
- [CLI reference](https://oy.adonm.dev/reference.html) — commands, environment variables, and setup ownership
- [Compatibility](https://oy.adonm.dev/compatibility.html) — supported platforms and agent hosts
- [Architecture](https://oy.adonm.dev/architecture.html) and [contributing](CONTRIBUTING.md) — maintainer documentation

Run `oy <command> --help` for the installed version's exact flags.
