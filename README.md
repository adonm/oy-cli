# oy

[![Crates.io](https://img.shields.io/crates/v/oy-cli.svg)](https://crates.io/crates/oy-cli)
[![docs.rs](https://docs.rs/oy-cli/badge.svg)](https://docs.rs/oy-cli)

**A concise autonomous OpenCode agent with repeatable repository audits and reviews.**

`oy` is for maintainers who already use [opencode](https://opencode.ai/) and want a concise autonomous coding agent plus bounded, reviewable security audit, code-quality review, and finding-remediation workflows.

opencode still owns the model, provider, UI, sessions, permissions, edits, shell, and web tools. `oy` adds:

- deterministic, gitignore-aware repository and diff collection,
- one autonomous `oy` agent aligned with OpenCode 2's inspect/implement/verify behavior,
- three canonical audit/review/enhance skills that run under the user's OpenCode permissions,
- file-backed deterministic evidence with private SHA-256-bound workflow state,
- an `@oy-cli/opencode` OpenCode V2 package for the agent, skills, and commands,
- Markdown and SARIF rendering with stable finding IDs and statuses,
- a one-finding-at-a-time remediation handoff.

The **inputs, ordering, limits, and report rendering** are deterministic. Model conclusions are not; model choice, prompt quality, and the user's OpenCode tool policy still affect outcomes. Oy does not maintain a second permission system.

## Quick start

Requirements: OpenCode 2 with a configured provider, plus `git` for diff reviews. oy 0.13.2 no longer supports OpenCode 1.

```bash
curl -fsSL https://oy.adonm.dev/install.sh | sh
# Restart or activate your shell as the installer prints, then:
oy doctor
oy audit
```

The installer uses [`mise`](https://mise.jdx.dev/) to install pinned oy 0.13.2, `@opencode-ai/cli@0.0.0-next-15353`, `tokei`, and Universal Ctags. It verifies both primary versions, stops stale OpenCode services, prunes unreferenced old mise versions, and resets the integration before `oy setup` registers `@oy-cli/opencode@0.13.2`. OpenCode installs the package into its isolated cache and the installer waits up to 120 seconds to verify that plugin ID `oy` loaded. Set `OY_RESET_SETUP=0` to preserve the generated setup in place or `OY_SKIP_SETUP=1` to skip setup. Source-built Sighthound is opt-in with `OY_INSTALL_SIGHTHOUND=1`.

For a minimal manual install:

```bash
mise use --global node@24 cargo-binstall cargo:oy-cli@0.13.2 npm:@opencode-ai/cli@0.0.0-next-15353
oy setup
oy doctor
```

Configure authentication and models with OpenCode's [provider guide](https://v2.opencode.ai/providers). See the [getting-started guide](https://oy.adonm.dev/getting-started.html) for install behavior, supported host versions, and workspace-local setup.

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

Reports include stable IDs and statuses. `oy enhance` selects one actionable finding, makes a focused change under the user's effective OpenCode permissions, and verifies it when possible. Rerunning an audit or review reads the previous generated report once and carries forward only findings that remain current.

See the [workflow guide](https://oy.adonm.dev/workflows.html) for report semantics, scope behavior, failure limits, and practical examples.

## Why not just prompt opencode?

A free-form agent can choose what to inspect and may silently sample a large repository. The oy audit/review protocol instead:

1. inventories the requested scope,
2. creates ordered chunks or target-diff chunks,
3. fails closed when the configured chunk budget is exceeded,
4. exposes every bounded chunk in one versioned index for native OpenCode reads,
5. rejects changed input, artifact tampering, concurrent output changes, and malformed findings,
6. normalizes the final report for reruns and remediation.

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

## Agent, package, and setup

The OpenCode V2 package lives at `packages/opencode` and publishes as `@oy-cli/opencode`. It registers one `oy` primary agent, the three canonical skills, and their slash commands without permission overrides. `oy setup` pins the package version matching the binary in OpenCode's `plugins` array and removes superseded direct-file copies.

`oy setup` is package-first: it adds the exact `@oy-cli/opencode` version matching the binary and removes exact legacy agent, skill, command, MCP, and output-budget entries. Global setup uses `OPENCODE_CONFIG_DIR` when set, otherwise `~/.config/opencode/`; workspace setup uses `.opencode/`. Use `--dry-run` to preview or `--remove` to remove the owned plugin entry and legacy generated files.

The `oy` agent has no permission overrides. Its short system prompt carries the useful OpenCode 2 defaults that a custom prompt would otherwise replace: inspect before editing, follow repository conventions, make the smallest correct change, persist through verification, preserve unrelated worktree changes, avoid destructive Git operations, and report concisely. OpenCode and the user remain authoritative for permissions and approvals.

Setup and removal stage one multi-file batch and roll back mutations already committed if a later mutation fails. This is in-process rollback, not crash journaling or durable recovery. JSON and JSONC are still pretty-reserialized, so comments and formatting are not preserved. Removal deletes owned current values; it does not restore values that existed before setup.

Launch, model, and workflow commands only validate that a complete global or workspace integration exists; they never auto-refresh it. Run `oy setup` explicitly after upgrading (`oy upgrade` does this as an explicit post-upgrade step). All selected OpenCode runner and managed-API processes use `OY_ROOT` as their working directory.

oy defaults to the `opencode2` executable. This release supports exactly beta `0.0.0-next-15353` and tagged OpenCode 2.x; other prereleases and major versions fail closed until tested. `OY_OPENCODE` remains an executable override. `oy run`, `audit`, `review`, and `enhance` use the single `oy` agent. `oy run --auto` asks OpenCode to approve pending requests once while preserving explicit denies; without it, the user's normal policy applies. Set `OY_OPENCODE_MODEL=provider/model#variant` to override the noninteractive workflow model.

`oy`, `oy open`, and `oy chat` launch the OpenCode 2 TUI. Select `oy` in the TUI when its concise autonomous prompt is useful; use OpenCode's built-in Plan agent when planning is wanted.

Audit and review use `prepare → native OpenCode reads/edits → finalize`. Preparation writes a small index, manifest, prior report, and bounded chunks under `.oy/runs/<run-id>/`. Private platform-state metadata binds their hashes, canonical workspace, scope or target OID, output, and format. Finalization verifies those bindings and separate candidate Markdown/findings JSON before writing the report. `oy mcp` remains only as a temporary compatibility adapter and is not registered by default.

## Command map

| Command | Purpose |
|---|---|
| `oy audit [focus]` | Write `ISSUES.md` or SARIF from deterministic-input audit coverage |
| `oy audit prepare` / `oy audit finalize --run ID` | Prepare audit artifacts and finalize a verified candidate |
| `oy review [target]` | Write `REVIEW.md` for a workspace or target diff |
| `oy review prepare [target]` / `oy review finalize --run ID` | Prepare review artifacts and finalize a verified candidate |
| `oy enhance [--interactive] [focus]` | Fix one finding; `--interactive` delegates to OpenCode `mini` |
| `oy setup [--workspace] [--dry-run] [--remove]` | Install, preview, or remove generated OpenCode integration |
| `oy doctor [--check]` | Show local status; `--check` validates effective OpenCode runtime integration |
| `oy` / `oy open ...` / `oy chat` | Launch or pass arguments to the OpenCode 2 TUI |
| `oy run [--auto]`, `model` | Noninteractive task with the `oy` agent, and transitional model-list convenience |
| `oy upgrade` | Upgrade mise-managed `oy` and OpenCode together |
| `oy mcp` | Serve the temporary stdio MCP compatibility adapter |

The full CLI and MCP inventory are in the [reference](https://oy.adonm.dev/reference.html).

## Safety

`oy` is not a sandbox. Repository text read from prepared artifacts or returned by compatibility MCP may be sent to the model provider selected in OpenCode. Native oy can read collected workspace text, run fixed read-only helper processes, write requested reports inside the workspace, update integration config, launch OpenCode's noninteractive runner, and query its managed model API through the CLI. oy does not store provider credentials; general edits, shell, web, authentication, and provider traffic remain governed by OpenCode.

Configure OpenCode [permissions](https://v2.opencode.ai/permissions) for your trust boundary and use a disposable environment for untrusted repositories. Read [SECURITY.md](SECURITY.md) and the [tool safety notes](docs/tool-safety.md) before high-risk use.

## Project direction and development

The product contract is intentionally narrow: improve the audit → review → remediate loop without becoming another model client or general agent framework. See [ROADMAP.md](ROADMAP.md) for current outcomes and explicit non-goals.

```bash
just dev
just check
just eval
just docs
```

Contributor references: [architecture](docs/architecture.md), [evaluation](docs/evaluation.md), [contributing](CONTRIBUTING.md), and [docs.rs API](https://docs.rs/oy-cli).
