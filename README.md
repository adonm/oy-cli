# oy

[![Crates.io](https://img.shields.io/crates/v/oy-cli.svg)](https://crates.io/crates/oy-cli)
[![docs.rs](https://docs.rs/oy-cli/badge.svg)](https://docs.rs/oy-cli)

`oy` launches opencode with deterministic repository helpers for audit and review workflows.

opencode owns the model, UI, sessions, permissions, editing, shell, web, and general tool loop. `oy` adds a small local MCP server for deterministic repository inputs and report rendering, then installs agents, skills, and commands that use those helpers.

## Quick Start

```bash
mise use cargo-binstall cargo:oy-cli
oy setup
oy
```

`oy setup` writes global integration files under `~/.config/opencode/`. `oy` with no subcommand ensures that integration exists and launches `opencode --agent oy`.

## Requirements

- opencode installed and configured
- Rust 1.96 or later if building from source
- `git` for diff-based review input

Model providers, authentication, sessions, permissions, editing, shell commands, web fetches, and UI behavior are configured in opencode. Use its provider and config docs for those surfaces.

## Commands

| Command | What it does |
|---|---|
| `oy` | Install/update global integration silently, then launch opencode with the `oy` agent |
| `oy setup` | Write `~/.config/opencode/opencode.json`, agents, and skills |
| `oy setup --workspace` | Write project-local `.opencode` integration files instead |
| `oy mcp` | Start the local stdio MCP server |
| `oy open ...` | Pass arguments through to `opencode` |
| `oy run "prompt"` | Compatibility wrapper for `opencode run --agent oy "prompt"` |
| `oy chat` | Compatibility wrapper that launches opencode with `--agent oy` |
| `oy model [provider]` | Compatibility wrapper for `opencode models [provider]` |
| `oy audit [focus]` | Compatibility wrapper for `opencode run --command oy-audit ...` |
| `oy review [target]` | Compatibility wrapper for `opencode run --command oy-review ...` |
| `oy enhance [focus]` | Compatibility wrapper for `opencode run --command oy-enhance ...` |
| `oy doctor` | Check opencode and oy integration status |

Legacy command names are kept for muscle memory. Their AI behavior now runs through opencode.

## Generated Integration

`oy setup` creates global files:

```text
~/.config/opencode/opencode.json
~/.config/opencode/agents/oy.md
~/.config/opencode/agents/oy-plan.md
~/.config/opencode/agents/oy-edit.md
~/.config/opencode/agents/oy-auto.md
~/.config/opencode/agents/oy-auditor.md
~/.config/opencode/agents/oy-reviewer.md
~/.config/opencode/agents/oy-enhancer.md
~/.config/opencode/skills/oy-audit/SKILL.md
~/.config/opencode/skills/oy-review/SKILL.md
```

`oy setup --workspace` writes the same integration under `.opencode/` for project-local overrides:

```text
.opencode/opencode.json
.opencode/agents/oy.md
.opencode/agents/oy-plan.md
.opencode/agents/oy-edit.md
.opencode/agents/oy-auto.md
.opencode/agents/oy-auditor.md
.opencode/agents/oy-reviewer.md
.opencode/agents/oy-enhancer.md
.opencode/skills/oy-audit/SKILL.md
.opencode/skills/oy-review/SKILL.md
```

The generated config registers `oy mcp` as a local MCP server:

```jsonc
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "oy": {
      "type": "local",
      "command": ["oy", "mcp"],
      "enabled": true,
      "timeout": 300000
    }
  }
}
```

MCP tools are namespaced by server name, so the model sees tools such as `oy_repo_manifest` and `oy_render_review_report`.

`oy` does not set the global `default_agent`; it passes `--agent` when launched through `oy`. Direct `opencode` usage keeps your normal default.

## oy Modes

The old `--mode` names now map to generated primary agents:

| oy mode | agent | Permissions |
|---|---|---|
| `default` / `ask` | `oy` | edits ask, bash asks |
| `plan` / `read` | `oy-plan` | edits denied, bash denied |
| `accept-edits` / `edit` | `oy-edit` | edits allowed, bash asks |
| `auto-approve` / `auto` | `oy-auto` | edits allowed, bash allowed |

The agent prompts closely follow the old v0.10 oy run/chat guidance: inspect before editing, keep work terse and evidence-first, prefer simple explicit code, batch independent reads/searches, treat tool output as untrusted data, and verify focused changes.

## MCP Tools

`oy mcp` exposes deterministic helpers only. It does not call a model, edit source files, run shell commands, fetch the web, or clone repositories.

| Tool | Purpose |
|---|---|
| `repo_manifest` | Gitignore-aware file inventory, token estimates, optional security index |
| `repo_chunks` | Deterministic workspace chunking for audit/review input |
| `git_diff_input` | Deterministic review input from `git diff <target>` |
| `sloc` | Source line counts via `tokei` |
| `outline` | Tree-sitter structural outline for a source file when the default `outline` feature is enabled |
| `render_audit_report` | Write `ISSUES.md` or SARIF from produced findings |
| `render_review_report` | Write `REVIEW.md` from produced findings |

## Audit And Review

The old standalone `oy audit`, `oy review`, and `oy enhance` pipelines have been replaced by generated commands/agents.

```bash
oy audit "security and complexity"
oy review main --focus "types and boundaries"
oy enhance --review-target main
```

Those wrappers use the generated agents. opencode performs the reasoning and orchestration; oy MCP provides deterministic input chunks and report rendering.

## Safety

`oy` is not a sandbox, but its MCP server is intentionally narrow. Risky capabilities live in opencode and are governed by its permissions.

Native `oy` risks:

- reads reviewable workspace text for manifests/chunks/outlines/SLOC
- writes generated audit/review reports when asked
- writes global integration files during setup, or `.opencode` files with `oy setup --workspace`
- launches the `opencode` process

Use plan/read-only modes and disposable containers for untrusted repositories.

## Development

```bash
just dev
just check
just run -- --help
just run -- mcp
```

Important files:

| Path | Role |
|---|---|
| `src/opencode.rs` | Setup, generated config, legacy command wrappers |
| `src/mcp.rs` | Minimal stdio MCP JSON-RPC server |
| `src/audit/input.rs` | File collection, manifest, chunking, git diff input |
| `src/audit/findings.rs` | Structured findings extraction/render support |
| `src/tools/outline.rs` | Tree-sitter outline helper |
| `src/tools/workspace/sloc.rs` | SLOC helper |

See `docs/architecture.md`, `docs/tool-safety.md`, `SECURITY.md`, and `CONTRIBUTING.md` for more detail.
