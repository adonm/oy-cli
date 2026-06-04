# Code Quality Review

> Generated with [oy-cli](https://github.com/wagov-dtt/oy-cli): `OY_MODEL=opencode-go/deepseek-v4-pro oy review` · 2026-06-04

## Verdict
Needs work — the tree-sitter dependency footprint introduces measurable binary size, build-time, and supply-chain cost for a single tool. The workspace would benefit from optional feature gating. Other structures are sound.

## Findings summary
| Severity | Title | Reference |
|----------|-------|-----------|
| High | Excessive tree-sitter grammar dependencies bloat build and binary for a rarely‑used tool | `Cargo.toml:67-82`, `src/tools/outline.rs` |
| Medium | Two large test modules approach 1000 lines, losing local cohesion | `src/llm/test/executor.rs`, `src/tools/tests/workspace_tools.rs` |
| Low | `src/cli/config/paths.rs` mixes workspace‑validation and atomic‑file concerns | `src/cli/config/paths.rs::write_workspace_batch` |

## Detailed findings

### High – Excessive tree-sitter grammar dependencies bloat build and binary for a rarely‑used tool
**Severity:** High  
**Evidence:** `Cargo.toml` lists 16 tree‑sitter grammar crates (rust, python, javascript, typescript, go, java, c, cpp, c-sharp, ruby, php, swift, kotlin‑ng, bash, lua, dart), each pulling a compiled C parser. These are only consumed by `src/tools/outline.rs`.  
**Design impact:** Every build re‑compiles 16 C grammars, increasing CI time and developer iteration cost. The shipped binary includes all grammars even though most users will only encounter a handful of languages. This also widens the supply‑chain attack surface.  
**Simplify:** Move the outline tool (and all tree‑sitter deps) behind an opt‑in Cargo feature, e.g. `features = ["outline"]`. When the feature is disabled, `tool_outline` is not registered and no grammars are compiled. The default release (used via `cargo install` or binstall) can keep the feature enabled, but local development and lean deployments benefit from the smaller binary.

### Medium – Two large test modules approach 1000 lines, losing local cohesion
**Severity:** Medium  
**Evidence:** `src/llm/test/executor.rs` is 748 lines; `src/tools/tests/workspace_tools.rs` is 667 lines. Both mix tests for multiple protocol codepaths (`executor.rs` covers Chat, Responses, Anthropic, Gemini, Bedrock) and multiple tools (`workspace_tools.rs` covers patch, replace, search, sloc, list, read).  
**Design impact:** Despite being test code, long files still obstruct quick navigation and encourage duplicate scaffolding. A contributor adding a new tool is likely to append to an already‑long file instead of creating a focused module.  
**Simplify:** Split `executor.rs` into one file per protocol (e.g. `src/llm/test/openai_chat.rs`, `src/llm/test/openai_responses.rs`, etc.) and `workspace_tools.rs` into tool‑oriented files (patch, search, read) under `src/tools/tests/`. Both splits are mechanical and preserve all existing tests.

### Low – `src/cli/config/paths.rs` mixes workspace‑validation and atomic‑file concerns
**Severity:** Low  
**Evidence:** The module spans workspace root resolution, output‑path safety checks, symlink rejection, `write_workspace_batch` with backup‑and‑rollback, and private‑file writes. The backup/rollback logic (structs `PreparedWorkspaceWrite`, `CommittedWorkspaceWrite`) is unrelated to path resolution.  
**Design impact:** The interleaving makes it harder to understand the backup safety net independently of workspace rules, and adding a new safe‑file primitive would require modifying this already‑busy file.  
**Simplify:** Extract the atomic‑write helpers (`write_workspace_batch`, `prepare_workspace_write`, `commit_workspace_writes`, backup/rollback) into a sibling module like `src/cli/config/atomic_write.rs`. The public API (`write_workspace_file`, `write_workspace_batch`) remains unchanged in the facade.

## Machine-readable findings
```json oy-findings
[
  {
    "source": "whole workspace",
    "severity": "High",
    "title": "Excessive tree-sitter grammar dependencies bloat build and binary for a rarely‑used tool",
    "locations": [
      { "path": "Cargo.toml", "line": 67 },
      { "path": "Cargo.toml", "line": 82 }
    ],
    "evidence": "16 tree‑sitter grammar crates are unconditionally compiled; only used by src/tools/outline.rs.",
    "body": "Move outline tool behind an opt‑in Cargo feature (e.g. `features = [\"outline\"]`). When disabled, register no outline tool and drop all grammar deps. This reduces compile time, binary size, and supply‑chain surface, allowing lean builds for users who don't need structural outlines.",
    "category": "dependency-weight"
  },
  {
    "source": "whole workspace",
    "severity": "Medium",
    "title": "Two large test modules approach 1000 lines, losing local cohesion",
    "locations": [
      { "path": "src/llm/test/executor.rs", "line": 1 },
      { "path": "src/tools/tests/workspace_tools.rs", "line": 1 }
    ],
    "evidence": "executor.rs (748 lines) tests five protocols in one file; workspace_tools.rs (667 lines) tests seven tools in one file.",
    "body": "Mechanically split by protocol (executor) and by tool (workspace_tools). Each new file stays under 300 lines and makes it obvious where to add tests for a new protocol or tool.",
    "category": "decomposition"
  },
  {
    "source": "whole workspace",
    "severity": "Low",
    "title": "src/cli/config/paths.rs mixes workspace‑validation and atomic‑file concerns",
    "locations": [
      { "path": "src/cli/config/paths.rs", "symbol": "write_workspace_batch" }
    ],
    "evidence": "The file contains both path resolution helpers and the full backup‑and‑rollback atomic‑write pipeline (PreparedWorkspaceWrite, CommittedWorkspaceWrite).",
    "body": "Extract the atomic‑write implementation into a sibling module such as src/cli/config/atomic_write.rs. Keep the public API unchanged in the config facade.",
    "category": "decomposition"
  }
]
```
