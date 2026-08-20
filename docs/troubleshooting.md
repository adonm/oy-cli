# Troubleshooting

This page covers the most common first-run problems and their fixes. For setup details see [Getting started](getting-started.md); for report interpretation see [Workflow guide](workflows.md).

## Quick diagnosis

Run this first — it tells you what oy sees:

```bash
oy doctor --check
oy doctor --json | head -n 80   # redacted, safe to share
oy --version
```

## 1. `oy: command not found`

**Cause:** mise hasn't activated in this shell, or `~/.local/bin` isn't on `PATH`.

**Fix:**

```bash
# restart your shell, or:
exec $SHELL -l
oy --version

# if still missing, check where mise put it:
mise exec github:adonm/oy-cli@latest -- oy --version
echo $PATH | tr ':' '\n' | grep -E "mise|\.local/bin"
```

The installer configures mise activation for bash/zsh/fish via `mise use`. After install, you need a new shell session.

## 2. `oy doctor --check` says skills are missing

**Fix:**

```bash
oy setup                # global: ~/.agents/skills
oy doctor --check       # should now say "global skills ok"

# per-repo only:
oy setup --workspace
oy doctor --check       # should say "workspace skills ok"
```

Check the files exist:

```bash
ls ~/.agents/skills/oy-audit/SKILL.md
ls ~/.agents/skills/oy-review/SKILL.md
ls ~/.agents/skills/oy-setup/SKILL.md
```

If `OY_SKILLS_DIR` is set, it overrides `~/.agents/skills` — check `echo $OY_SKILLS_DIR` and `oy doctor --json`.

## 3. Agent says "skill not found" even though `oy doctor --check` passes

Different agents look in slightly different places:

| Agent | Looks in |
|---|---|
| OpenCode, Cursor, Codex, Copilot, Gemini CLI | `~/.agents/skills` or `.agents/skills` (standard) |
| Claude Code | `.claude/skills` |

**Fix:** ask your agent to run the `oy-setup` skill — it offers to copy or symlink the canonical skills to the host-specific directory:

```text
run the oy-setup skill
```

See [Compatibility](compatibility.md#agent-skills-hosts) for details.

## 4. Model not responding / authentication error

`oy` never handles API keys. Your agent does.

- **Cursor:** Settings → Models → add provider API key
- **OpenCode:** `~/.config/opencode/opencode.jsonc` or `.opencode/opencode.jsonc` with provider credentials
- **Codex / Copilot / Gemini CLI:** configure the provider in that host's settings

Test your model without oy first (ask the agent a simple question). If that fails, fix provider config before auditing.

## 5. `exceeds max-chunks 80` or "coverage limit"

The repo is larger than the default 80-chunk budget. `oy` fails closed rather than silently sampling.

**Fix — narrow the scope first:**

```text
audit src/auth with the oy-audit skill
audit src/api with the oy-audit skill
review src/cli with the oy-review skill
```

Only raise the limit when the broader scope is intentional:

```text
audit this repository with the oy-audit skill with max-chunks 120
```

Or for automation: `oy audit prepare --max-chunks 120`.

## 6. Target-diff review fails (`not a git repository` / unknown ref)

- Must be inside a git repo: `git status`
- Target must exist: `git rev-parse --verify main` (or `origin/main`, a commit SHA, tag)
- Try `git fetch origin` if the target is a remote branch
- Omit `--path` when using a target — the diff already scopes the input

## 7. Empty or "no findings" report — is it broken?

No — an empty findings array is a successful run:

````markdown
## Findings summary
No high-conviction findings.
```json oy-findings
[]
```
````

Check the report header for evidence digest and chunk count, and the CLI exit code (`0` = success). Try a more focused prompt or a larger scope if you expected findings.

## 8. Reports don't appear / wrong location

Default outputs are workspace-relative and must stay inside the workspace:

- `ISSUES.md` for audits
- `REVIEW.md` for reviews
- `oy.sarif` for SARIF

Check `ls -la ISSUES.md REVIEW.md oy.sarif` in the workspace root. Custom paths must not escape via `../` or symlinks.

## 9. `oy setup` or `oy doctor --install-missing` fails on tokei/ctags

These are optional context helpers — audits and reviews work without them. They help the `oy` persona explore large unfamiliar codebases faster.

Retry:

```bash
oy doctor --install-missing
```

They install prebuilt binaries via mise (`tokei 12.1.2`, `ctags` nightly release archives) — no Rust toolchain needed. If it still fails, ignore it or install manually.

## Still stuck?

```bash
oy doctor --json > /tmp/oy-doctor.json   # redact paths if sensitive before sharing
cat /tmp/oy-doctor.json
oy setup --dry-run                       # preview without writing
```

When asking for help, include:

- `oy --version`
- agent name and version
- OS and architecture (`uname -a`)
- install method and scope (global vs workspace)
- redacted `oy doctor --json`

Do not include credentials, prompts, or sensitive source. See [Compatibility](compatibility.md#reporting-a-compatibility-problem).
