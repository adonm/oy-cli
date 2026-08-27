# oy

[![Crates.io](https://img.shields.io/crates/v/oy-cli.svg)](https://crates.io/crates/oy-cli)
[![docs.rs](https://docs.rs/oy-cli/badge.svg)](https://docs.rs/oy-cli)

**Deterministic-input audits, code reviews, and one-finding fixes — as portable agent skills.**

`oy` helps your coding agent review a repository without quietly choosing a small sample. It prepares an ordered, reviewable set of files, lets your agent's model analyze them under its own permissions, and verifies the report before writing it. The workflows are standard Agent Skills that OpenCode, Cursor, Codex, Copilot, and Gemini CLI all discover from `.agents/skills`.

## What you get

- `oy-audit` skill — security-focused repository audits (`ISSUES.md` or SARIF)
- `oy-review` skill — whole-workspace or target-diff code reviews (`REVIEW.md`)
- `oy-enhance` skill — fix one reported finding at a time
- `oy-setup` skill — agent-driven setup, verification, and persona installation
- deterministic CLI: `oy audit|review prepare` and `finalize`, `oy setup`, `oy doctor`

Your agent owns models, credentials, sessions, and general tools. `oy` adds the evidence and report workflow; it is not a second agent runtime or permission system.

## Quick start — 2 minutes

**You need:** Linux (WSL2 works elsewhere), any agent that reads Agent Skills (OpenCode, Cursor, Codex, Copilot, or Gemini CLI) with a model provider already configured.

**Step 1 — Install oy:**

```bash
curl -fsSL https://oy.adonm.dev/install.sh | sh
# choose Global when prompted (or pass --global / --workspace to skip the prompt)
oy doctor --check   # expect: "global skills ok" or "workspace skills ok"
```

> Review [install.sh](https://oy.adonm.dev/install.sh) before piping to a shell. Prefer a manual install? See [Getting started](https://oy.adonm.dev/getting-started.html).

**Step 2 — Ask your agent (copy-paste one line):**

```text
run the oy-setup skill to finish setup
```

Then create your first report:

```text
audit this repository with the oy-audit skill
```

That's it — look for `ISSUES.md` in your workspace root. Try next:

```text
review the diff against main with the oy-review skill
use the oy-enhance skill to fix audit-0123456789abcdef
```

Local dev from this checkout: `just install` (cargo install + `oy setup` + `oy doctor --check`).

### What just happened?

1. `oy` collected eligible files into ordered chunks under `.oy/runs/<id>/`
2. Your agent read every chunk and wrote candidate findings
3. `oy` verified the evidence wasn't changed and normalized the report

> **The inputs are deterministic; the conclusions are not.** Model choice still affects findings. See [Coverage and limits](https://oy.adonm.dev/workflows.html#coverage-and-limits) before using a report for high-assurance work.

### New to Agent Skills?

Agent Skills are just `SKILL.md` files. `oy setup` writes them to `~/.agents/skills/` (or `.agents/skills/` for workspace-only). Your agent discovers them automatically — no API keys or separate daemon. If `oy doctor --check` passes but your agent can't see the skills, ask it to run the `oy-setup` skill; it will copy/symlink them to your host's preferred location (e.g. `.claude/skills`).

## Common workflows

### Audit a repository

```text
audit this repository with the oy-audit skill
audit src/auth with the oy-audit skill
audit the authentication boundaries with the oy-audit skill
audit with sarif output and write oy.sarif
```

A single existing workspace path narrows collection (e.g. `src/auth`). Other text is treated as review guidance for the model.

### Review code

```text
review this repository with the oy-review skill
review the diff against main with the oy-review skill
review the diff against main with the oy-review skill, focusing on error handling
```

A branch, commit, tag, or ref selects target-diff review. Without a target, the skill reviews the workspace.

### Fix one finding

```text
use the oy-enhance skill to fix audit-0123456789abcdef
```

Reports include stable finding IDs. The skill confirms the cited source, makes one focused fix, and runs the narrowest available verification. Rerun the originating audit or review to confirm.

## Troubleshooting

- `oy: command not found` → restart your shell (mise activation) or check `~/.local/bin` is on `PATH`
- `oy doctor --check` fails → run `oy setup` again, then ask your agent to run the `oy-setup` skill
- Agent can't find the skill → see [Compatibility](https://oy.adonm.dev/compatibility.html) for where each agent looks, or ask the `oy-setup` skill to copy them
- `exceeds max-chunks 80` → narrow the path first (e.g. `audit src/auth`), only then raise `--max-chunks`
- Model not configured → configure your provider in your agent (not in oy); `oy` never stores credentials

More help: [Getting started](https://oy.adonm.dev/getting-started.html) · [Workflow guide](https://oy.adonm.dev/workflows.html) · [Troubleshooting](https://oy.adonm.dev/troubleshooting.html) · `oy doctor` · `oy <command> --help`

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
