# oy

[![Crates.io](https://img.shields.io/crates/v/oy-cli.svg)](https://crates.io/crates/oy-cli)
[![docs.rs](https://docs.rs/oy-cli/badge.svg)](https://docs.rs/oy-cli)

**Repeatable repository audits and reviews for opencode.**

`oy` is for maintainers who already use [opencode](https://opencode.ai/) and want a bounded, reviewable workflow for repository-wide security audits, code-quality reviews, and finding remediation.

opencode still owns the model, provider, UI, sessions, permissions, edits, shell, and web tools. `oy` adds:

- deterministic, gitignore-aware repository and diff collection,
- three canonical audit/review/enhance workflow skills with thin command and agent adapters,
- restricted audit/review agent permissions plus Rust/MCP enforcement of bound workflow inputs,
- Markdown and SARIF rendering with stable finding IDs and statuses,
- a one-finding-at-a-time remediation handoff.

The **inputs, ordering, limits, and report rendering** are deterministic. Model conclusions are not; model choice and prompt quality still affect findings.

## Quick start

Requirements: OpenCode 2 with a configured provider, plus `git` for diff reviews. oy 0.12.0-beta.1 no longer supports OpenCode 1.

```bash
curl -fsSL https://oy.adonm.dev/install.sh | sh
# Restart or activate your shell as the installer prints, then:
oy doctor
oy audit
```

The installer uses [`mise`](https://mise.jdx.dev/) to install or upgrade `oy`, `@opencode-ai/cli@0.0.0-next-15323`, `tokei`, and Universal Ctags, then writes the global OpenCode integration with `oy setup`. Source-built Sighthound is opt-in with `OY_INSTALL_SIGHTHOUND=1`; the installer provisions Rust 1.96 for it.

For a minimal manual install:

```bash
mise use --global node@24 cargo-binstall cargo:oy-cli npm:@opencode-ai/cli@0.0.0-next-15323
oy setup
oy doctor
```

Configure authentication and models with OpenCode's [provider guide](https://opencode.ai/docs/providers/). See the [getting-started guide](https://oy.adonm.dev/getting-started.html) for install behavior, supported host versions, and workspace-local setup.

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

See the [workflow guide](https://oy.adonm.dev/workflows.html) for report semantics, scope behavior, failure limits, and practical examples.

## Why not just prompt opencode?

A free-form agent can choose what to inspect and may silently sample a large repository. The oy audit/review protocol instead:

1. inventories the requested scope,
2. creates ordered chunks or target-diff chunks,
3. fails closed when the configured chunk budget is exceeded,
4. rejects changed input, skipped chunks, out-of-order reads, and early rendering,
5. normalizes the final report for reruns and remediation.

This makes coverage decisions visible and repeatable without rebuilding opencode's general coding agent.

## Coverage boundary

“Every chunk” means every chunk produced by oy's collector, not every byte in the repository. Collection skips gitignored and hidden paths, common dependency/build directories, lockfiles, likely-secret files, generated reports, binary/non-UTF-8/empty files, unreadable files, and files larger than 512 KiB. Eligible large files and diff evidence are sliced so chunk text stays below 240 KiB and 19,000 lines. Review these exclusions before treating an audit as complete for a high-assurance use case.

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
mise use --global rust@1.96 'cargo:https://github.com/Corgea/Sighthound[bin=sighthound,locked=true]@rev:c4608eb2b6ca256daf4dbd1e74aadc3570343685'
```

Sighthound remains optional and source-built. The install pins immutable commit `c4608eb2b6ca256daf4dbd1e74aadc3570343685`, uses `--locked`, and builds only `bin=sighthound`; `oy doctor --install-sighthound` performs the same pinned build. Routine `oy doctor --install-missing` does not build it. Sighthound scans Python, JavaScript/TypeScript, Java, PHP, C#, Go, Ruby, HTML, and Django templates; it does not scan Rust or C/C++. `OY_TOKEI`, `OY_CTAGS`, and `OY_SIGHTHOUND` can select absolute helper paths. Automatic discovery rejects relative `PATH` entries.

## Setup and launcher behavior

`oy setup` writes native OpenCode 2 JSON plus seven generated agents and three canonical skills. Global setup uses `OPENCODE_CONFIG_DIR` when set, otherwise `~/.config/opencode/`; workspace setup uses `.opencode/`. In either directory, an existing `opencode.jsonc` is selected before `opencode.json`. Use `--dry-run` to preview or `--remove` to remove oy's current generated files and owned config entries.

Setup and removal stage one multi-file batch and roll back mutations already committed if a later mutation fails. This is in-process rollback, not crash journaling or durable recovery. Setup migrates exact legacy command/MCP conversions and fails closed on ambiguous fields. JSON and JSONC are still pretty-reserialized, so comments and formatting are not preserved. Removal deletes owned current values; it does not restore values that existed before setup.

Launch, model, and workflow commands only validate that a complete global or workspace integration exists; they never auto-refresh it. Run `oy setup` explicitly after generated assets change (`oy upgrade` does this as an explicit post-upgrade step). All selected OpenCode runner and managed-API processes use `OY_ROOT` as their working directory.

oy defaults to the `opencode2` executable. This release supports exactly beta `0.0.0-next-15323` and tagged OpenCode 2.x; other prereleases and major versions fail closed until tested. `OY_OPENCODE` remains an executable override. `oy run`, `audit`, `review`, and `enhance` use OpenCode 2's noninteractive `run` command; `oy model` uses its managed API because the beta has no model-list command. `oy run` supports `--continue-session`, `--resume`, and mode-selected agents; set `OY_OPENCODE_MODEL=provider/model#variant` to override the noninteractive workflow model. JSON mode forwards OpenCode's JSON event stream.

`oy`, `oy open`, and `oy chat` launch the OpenCode 2 TUI. Session continuation/resume is supported, but the beta TUI cannot select an agent or oy mode per launch, so select the desired agent inside the TUI; use `oy run` when mode selection is required.

CLI audit/review/enhance runs inherit a typed context binding the run ID, session, model, scope, output, format, and maximum chunks. Review targets are resolved to a commit OID before launch. MCP enforces those values, stable input, chunk count/order/completion, and renderer output metadata; noninteractive workflow session titles include `oy:<run-id>` for correlation.

OpenCode's noninteractive runner cannot pause for unresolved `ask` permissions. Use `plan` for read-only work, `edit` when file edits are intentionally pre-approved, and `auto` only in trusted workspaces where both edits and shell are pre-approved.

## Command map

| Command | Purpose |
|---|---|
| `oy audit [focus]` | Write `ISSUES.md` or SARIF from deterministic-input audit coverage |
| `oy review [target]` | Write `REVIEW.md` for a workspace or target diff |
| `oy enhance [--interactive] [focus]` | Fix one finding; `--interactive` delegates to OpenCode `mini` |
| `oy setup [--workspace] [--dry-run] [--remove]` | Install, preview, or remove generated OpenCode integration |
| `oy doctor [--check]` | Show local status; `--check` validates effective OpenCode runtime integration |
| `oy` / `oy open ...` / `oy chat` | Launch or pass arguments to the OpenCode 2 TUI |
| `oy run`, `model`, `modes` | Noninteractive task, model-list, and safety-mode conveniences |
| `oy upgrade` | Upgrade mise-managed `oy` and OpenCode together |
| `oy mcp` | Serve the local stdio MCP integration; normally started by opencode |

The full CLI and MCP inventory are in the [reference](https://oy.adonm.dev/reference.html).

## Safety

`oy` is not a sandbox. Repository text returned by MCP may be sent to the model provider selected in OpenCode. Native oy can read collected workspace text, run fixed read-only helper processes, write requested reports inside the workspace, update integration config, launch OpenCode's noninteractive runner, and query its managed model API through the CLI. oy does not store provider credentials; general edits, shell, web, authentication, and provider traffic remain governed by OpenCode.

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
