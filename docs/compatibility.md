# Compatibility

## Which setup do I have?

Not sure? Run:

```bash
oy --version
oy doctor --check   # "global skills ok" or "workspace skills ok" means you're good
oy doctor --json | head -n 40
```

## Platforms

| Environment | Support |
|---|---|
| Linux x86_64 with glibc | Full CI and release archive |
| Linux aarch64 with glibc | Release archive; full suite not run on target |
| Other Linux targets | Source build; not release-tested |
| macOS, Windows | Unsupported; use WSL2 or build from source |
| Other operating systems | Unsupported at build time |

The installer requires a POSIX shell plus `curl` or `wget`. Its prebuilt oy release supports the two release-archive targets above; other Linux targets require a source build. Building from source requires Rust 1.96+.

## Agent skills hosts

oy does not require any specific agent. The skills are plain Agent Skills
(`SKILL.md` files) under the cross-agent `.agents/skills` location, which
the current releases of OpenCode, Cursor, Codex, GitHub Copilot, and Gemini
CLI all discover natively.

| Agent | Where it looks for skills |
|---|---|
| OpenCode, Cursor, Codex, Copilot, Gemini CLI | `~/.agents/skills` (global) or `.agents/skills` (workspace) |
| Claude Code | `.claude/skills` — the `oy-setup` skill offers to copy or symlink there |

If `oy doctor --check` passes but your agent says "skill not found", ask it:

```text
run the oy-setup skill
```

The skill will detect the host and offer to copy the files to the right place. No manual file copying needed.

The optional post-setup OpenCode location refresh uses `opencode2` (or
`OY_OPENCODE`). It is best-effort: when OpenCode is absent or unsupported,
setup simply skips the refresh and everything else still works.

## What `doctor --check` covers

```bash
oy doctor --check
```

This checks that the canonical skill files are installed with byte-exact
content in the global or workspace skills directory and that the obsolete
OpenCode plugin package cache is gone. It does not validate your agent's
permission choices or make a model request.

If it fails, run `oy setup` again and retry. See [Troubleshooting](troubleshooting.md) for common fixes.

## Setup locations

- Global skills: `~/.agents/skills/`, or `OY_SKILLS_DIR` when set
- Workspace skills: `OY_ROOT/.agents/skills/`
- Legacy OpenCode config cleaned by setup: `OPENCODE_CONFIG_DIR`, or the platform OpenCode config directory

Setup preserves unrelated configuration and backs up changed oy-owned entries. See [Setup ownership and backups](reference.md#setup-ownership-and-backups).

## Optional tools

`tokei` and Universal Ctags are optional context helpers for large unfamiliar scopes. Missing them does **not** block setup,
audit, review, or remediation. Install them with:

```bash
oy doctor --install-missing
```

The helper installs prebuilt artifacts only: tokei 12.1.2 through mise's Aqua backend and Universal Ctags release archives from the official nightly-build repository.

If the install fails, you can safely ignore it — your first audit will still work.

## Reporting a compatibility problem

Include:

- `oy --version`;
- your agent and its version;
- operating system and architecture;
- install method and setup scope;
- reviewed and redacted `oy doctor --json` output.

Do not include credentials, prompts, or sensitive source text.
