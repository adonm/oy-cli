# oy

[![Crates.io](https://img.shields.io/crates/v/oy-cli.svg)](https://crates.io/crates/oy-cli)
[![docs.rs](https://docs.rs/oy-cli/badge.svg)](https://docs.rs/oy-cli)

**Repeatable repository audits and reviews for opencode.**

`oy` is for maintainers who already use [opencode](https://opencode.ai/) and want a bounded, reviewable workflow for repository-wide security audits, code-quality reviews, and finding remediation.

opencode still owns the model, provider, UI, sessions, permissions, edits, shell, and web tools. `oy` adds:

- deterministic, gitignore-aware repository and diff collection,
- restricted audit/review agents that cannot use generic shell, edit, or search tools,
- Markdown and SARIF rendering with stable finding IDs and statuses,
- a one-finding-at-a-time remediation handoff.

The **inputs, ordering, limits, and report rendering** are deterministic. Model conclusions are not; model choice and prompt quality still affect findings.

## Quick start

Requirements: opencode with a configured provider, plus `git` for diff reviews.

```bash
curl -fsSL https://adonm.github.io/oy-cli/install.sh | sh
# Restart or activate your shell as the installer prints, then:
oy doctor
oy audit
```

The installer uses [`mise`](https://mise.jdx.dev/) to install or upgrade `oy`, opencode, `tokei`, and Universal Ctags, then writes the global opencode integration with `oy setup`. Source-built Sighthound is opt-in with `OY_INSTALL_SIGHTHOUND=1` and requires Rust 1.85+.

For a minimal manual install:

```bash
mise use --global cargo-binstall cargo:oy-cli opencode
oy setup
oy doctor
```

Configure authentication and models with opencode's [provider guide](https://opencode.ai/docs/providers/). See the [getting-started guide](https://adonm.github.io/oy-cli/getting-started.html) for install behavior, supported release targets, and workspace-local setup.

## Core workflow

### 1. Audit a repository

```bash
oy audit
oy audit "authentication and authorization"
oy audit src/auth
oy audit --format sarif --out oy.sarif
```

The default report is `ISSUES.md`. An exact workspace-relative path narrows collection; other text is an audit lens. SARIF output can be consumed by code-scanning tools.

### 2. Review a workspace or target diff

```bash
oy review
oy review main
oy review main --focus "types and trust boundaries"
```

With no target, `oy` reviews the collected workspace. With a branch, commit, or ref, it reviews deterministic `git diff <target>` input. The default report is `REVIEW.md`.

### 3. Fix one finding

```bash
oy enhance <finding-id>
```

Reports include stable IDs and statuses. `oy enhance` selects one actionable finding, makes a focused change through opencode's permission system, and verifies it when possible. Rerunning an audit or review reads the previous generated report once and carries forward only findings that remain current.

See the [workflow guide](https://adonm.github.io/oy-cli/workflows.html) for report semantics, scope behavior, failure limits, and practical examples.

## Why not just prompt opencode?

A free-form agent can choose what to inspect and may silently sample a large repository. The oy audit/review protocol instead:

1. inventories the requested scope,
2. creates ordered chunks or target-diff chunks,
3. fails closed when the configured chunk budget is exceeded,
4. requires the restricted agent to read every collected chunk,
5. normalizes the final report for reruns and remediation.

This makes coverage decisions visible and repeatable without rebuilding opencode's general coding agent.

## Coverage boundary

“Every chunk” means every chunk produced by oy's collector, not every byte in the repository. Collection skips gitignored and hidden paths, common dependency/build directories, lockfiles, likely-secret files, generated reports, binary/non-UTF-8/empty files, unreadable files, and files larger than 512 KiB. Review these exclusions before treating an audit as complete for a high-assurance use case.

Sighthound uses independent gitignore-aware discovery and its own filters/size limit. It may inspect supported hidden source or source files larger than oy's 512 KiB collector limit. The auditor calls it only when the focus explicitly requests Sighthound or SAST; returned snippets may be sent to the model provider.

Optional local evidence tools add:

| Helper | MCP evidence |
|---|---|
| [`tokei`](https://github.com/XAMPPRocky/tokei) | source-line counts |
| [Universal Ctags](https://ctags.io/) | structural outlines |
| [Sighthound](https://github.com/Corgea/Sighthound) | bounded SAST candidates for supported languages |

Install them with:

```bash
mise use --global cargo:tokei
mise use --global github:universal-ctags/ctags
mise use --global cargo:https://github.com/Corgea/Sighthound@tag:1.0
```

Sighthound 1.0 is built from source and requires Rust 1.85+. It scans Python, JavaScript/TypeScript, Java, PHP, C#, Go, Ruby, HTML, and Django templates; it does not scan Rust or C/C++. `OY_TOKEI`, `OY_CTAGS`, and `OY_SIGHTHOUND` can select absolute helper paths. Automatic discovery rejects relative `PATH` entries.

## Setup and launcher behavior

`oy setup` writes `~/.config/opencode/opencode.json` plus generated agents and skills. Use `oy setup --workspace` for `.opencode/` integration or `oy setup --dry-run` to preview changes.

Launch-oriented commands (`oy`, `open`, `run`, `chat`, `model`, `audit`, `review`, and `enhance`) refresh the global integration before starting opencode and refresh an existing workspace integration when detected. The config merge owns `mcp.oy`, `command.oy-*`, and `tool_output.max_bytes`/`max_lines`. Other object keys are retained, but the JSON/JSONC file is pretty-serialized, so comments and formatting are not preserved. Generated Markdown files refuse to replace files without oy's generated marker.

`oy` with no subcommand launches the general `oy` coding agent. `run`, `chat`, `model`, safety modes, and unknown-command passthrough remain compatibility conveniences; audit, review, and remediation are the project's primary direction.

## Command map

| Command | Purpose |
|---|---|
| `oy audit [focus]` | Write `ISSUES.md` or SARIF from deterministic-input audit coverage |
| `oy review [target]` | Write `REVIEW.md` for a workspace or target diff |
| `oy enhance [focus]` | Fix one finding from `ISSUES.md` or `REVIEW.md` |
| `oy setup [--workspace] [--dry-run]` | Install or preview generated opencode integration |
| `oy doctor` | Show integration paths and optional helper availability |
| `oy` / `oy open ...` | Launch or pass arguments through to opencode |
| `oy run`, `chat`, `model`, `modes` | Compatibility and safety-mode conveniences |
| `oy upgrade` | Upgrade mise-managed `oy` and opencode together |
| `oy mcp` | Serve the local stdio MCP integration; normally started by opencode |

The full CLI and MCP inventory are in the [reference](https://adonm.github.io/oy-cli/reference.html).

## Safety

`oy` is not a sandbox. Repository text returned by MCP may be sent to the model provider selected in opencode. Native oy can read collected workspace text, run fixed read-only helper processes, write requested reports inside the workspace, update integration config, and launch opencode. General edits, shell, web, and provider traffic remain governed by opencode.

Use restrictive opencode [permissions](https://opencode.ai/docs/permissions/) and a disposable environment for untrusted repositories. Read [SECURITY.md](SECURITY.md) and the [tool safety notes](docs/tool-safety.md) before high-risk use.

## Project direction and development

The product contract is intentionally narrow: improve the audit → review → remediate loop without becoming another model client or general agent framework. See [ROADMAP.md](ROADMAP.md) for current outcomes and explicit non-goals.

```bash
just dev
just check
just eval
just docs
```

Contributor references: [architecture](docs/architecture.md), [evaluation](docs/evaluation.md), [contributing](CONTRIBUTING.md), and [docs.rs API](https://docs.rs/oy-cli).
