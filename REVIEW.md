# Code Quality Review

> Generated with [oy-cli](https://github.com/wagov-dtt/oy-cli): `OY_MODEL=opencode-go/deepseek-v4-pro oy review` · 2026-06-04

## Verdict
Needs work

## Findings summary

- **Medium** `src/tools/network.rs` / `src/llm/route/auth.rs` — Duplicate public‑IP classification drifts between webfetch and credential‑transport checks, risking inconsistent security decisions.
- **Medium** `src/review.rs` — Duplicate chunking and manifest logic re‑implements audit input abstractions; merged shared helpers would delete ~200 lines.
- **Low** `src/tools/preview.rs` — 862‑line file mixes per‑tool preview functions; splitting by tool category would make ownership clearer.
- **Low** `src/cli/config/paths.rs` — Inline Windows‑specific ACL code (unsafe) could be extracted to a dedicated platform module for safer maintenance.
- **Low** `src/agent/model/tests.rs` — 841‑line test file mixes live integration tests with unit tests; live tests could move to their own file for faster focused test runs.

## Detailed findings

### 1. Duplicate public‑IP classification (`src/tools/network.rs` and `src/llm/route/auth.rs`)

**Severity:** Medium  
**Category:** design  
**Locations:** `src/tools/network.rs::is_public_ip`, `src/llm/route/auth.rs::is_loopback_or_private_ip`

The webfetch tool classifies addresses as public via `ip_rfc::global` plus multicast/site‑local checks (`src/tools/network.rs:252‑259`), while the credential‑transport guard uses `IpAddr::is_loopback()`, `is_private()`, `is_link_local()` (`src/llm/route/auth.rs:63‑68`). Although the policies differ (webfetch blocks private; credential transport allows them), the underlying classification diverges: `auth.rs` does not handle IPv4‑mapped‑IPv6 or site‑local ranges, and the network tool does not handle unique‑local addresses in the same way. A future change to one classification could inadvertently relax the other.

**Concrete simplification:** Extract a single `is_public_ip()` helper (using `ip_rfc` as the webfetch tool already does) into a shared crate‑private module and use it in both places. Keep the separate policy layers at the call sites (webfetch denies non‑public, credential transport permits loopback/private). This removes the drift risk and makes the security boundary easier to audit.

### 2. Review module duplicates audit input chunking (`src/review.rs`)

**Severity:** Medium  
**Category:** design  
**Locations:** `src/review.rs::split_git_diff_items`, `src/review.rs::chunk_diff_items`, `src/review.rs::ensure_chunks_fit`, `src/review.rs::diff_manifest`, `src/review.rs::workspace_size_index`

The `oy review` command collects and chunks review input (workspace files or git diffs) in `prepare_workspace_input` and `prepare_diff_input` (`src/review.rs:124‑229`). This logic mirrors the audit module’s `src/audit/input::collect_files`, `chunk_files`, `build_manifest`, and `build_security_index` but with duplicate chunk‑construction, token‑counting, and oversize‑check loops. The review module also defines its own `ReviewChunk` and `DiffItem` structs instead of reusing `AuditChunk` and `AuditFile`.

The duplicated chunking machinery (~200 lines) makes it harder to change chunk‑budget semantics (e.g., deriving context limits from model metadata) for both commands simultaneously. Any future improvement to audit input handling must be replicated manually.

**Concrete simplification:** Merge review input collection into the audit input abstraction: introduce a single `ChunkInput` trait or shared functions that accept a file/diff source. The audit module’s `collect_files`, `chunk_files`, and `build_manifest` already cover whole‑workspace collection; extend them with a git‑diff source that produces `AuditFile`‑like items. Delete `ReviewChunk`, `DiffItem`, and the duplicated chunking/validation functions from `src/review.rs`.

### 3. Large preview file (`src/tools/preview.rs`)

**Severity:** Low  
**Category:** maintainability  
**Location:** `src/tools/preview.rs` (862 lines)

The preview module contains one standalone summary/output function per tool plus shared helpers like `append_search_hits` and `compact_kvs`. All preview logic for 15+ tools is mixed in a single file, making it harder to locate the rendering for a specific tool and to keep the file under the common 1,000‑line decomposition threshold.

**Concrete simplification:** Split the file into sub‑modules grouped by tool category (e.g., `workspace_previews`, `network_previews`, `process_previews`) or into per‑tool files. Keep the shared formatting helpers (`preview_value`, `with_verbose`, `append_preview_lines`) in a common sibling module. This keeps the registry‑driven dispatch intact while shrinking the source unit.

### 4. Windows ACL code in‑line (`src/cli/config/paths.rs`)

**Severity:** Low  
**Category:** maintainability  
**Location:** `src/cli/config/paths.rs::restrict_to_owner` (lines 266‑345)

The `restrict_to_owner` function (Windows only) is a long block of `unsafe` Windows API calls that constructs an ACL and sets file security. It lives in the middle of a module that also handles home‑directory expansion, workspace output validations, and private file writes.

**Concrete simplification:** Move the entire function to a dedicated `src/cli/config/platform/windows.rs` (conditional compilation) so the unsafe, platform‑specific security code can be reviewed in isolation. The public API in `paths.rs` would remain the same.

### 5. Test file mixes live integration tests (`src/agent/model/tests.rs`)

**Severity:** Low  
**Category:** maintainability  
**Location:** `src/agent/model/tests.rs` (841 lines)

The test file contains many fast unit tests for model routing and reasoning, as well as several `#[ignore]` live integration tests that require network access and OpenCode metadata. Running unit tests now also compiles the live‑test scaffolding (helper functions, imports, test definitions), which adds noise.

**Concrete simplification:** Move the live integration tests (functions starting with `live_`) into a separate file `src/agent/model/live_tests.rs` gated behind a feature flag or kept ignored. This keeps the unit test file focused and makes it trivial to run only fast tests.

## Machine-readable findings

```json oy-findings
[
  {
    "source": "review",
    "severity": "Medium",
    "title": "Duplicate public‑IP classification between webfetch and credential‑transport",
    "locations": [
      { "path": "src/tools/network.rs", "line": 252 },
      { "path": "src/llm/route/auth.rs", "line": 63 }
    ],
    "evidence": "is_public_ip() uses ip_rfc::global plus multicast checks; is_loopback_or_private_ip() uses IpAddr methods only. The two classifiers can drift and produce inconsistent decisions for edge‑case addresses.",
    "body": "Extract a single `is_public_ip()` helper (using `ip_rfc`) into a shared crate‑private module and use it in both locations. Keep policy differences at the call sites.",
    "category": "design"
  },
  {
    "source": "review",
    "severity": "Medium",
    "title": "Review module duplicates audit input chunking logic",
    "locations": [
      { "path": "src/review.rs", "symbol": "split_git_diff_items" },
      { "path": "src/review.rs", "symbol": "chunk_diff_items" }
    ],
    "evidence": "Functions `prepare_workspace_input` and `prepare_diff_input` recreate chunk construction, token counting, and oversize checks already present in `src/audit/input`. The review module defines its own `ReviewChunk` and `DiffItem` structs instead of reusing `AuditChunk`/`AuditFile`.",
    "body": "Merge review input collection into the audit input abstraction. Extend audit input with git‑diff source support, delete the duplicated chunking and validation functions, and reuse the existing `AuditChunk` and `AuditFile` types.",
    "category": "design"
  },
  {
    "source": "review",
    "severity": "Low",
    "title": "Large preview file mixes all tool preview functions",
    "locations": [
      { "path": "src/tools/preview.rs", "line": 1 }
    ],
    "evidence": "862 lines in a single file with per‑tool summary and output functions. No clear separation between workspace tools, network tools, and shell tools.",
    "body": "Split the file into sub‑modules grouped by tool category (workspace, network, process, etc.) or into per‑tool files. Keep shared formatting helpers in a common sibling module.",
    "category": "maintainability"
  },
  {
    "source": "review",
    "severity": "Low",
    "title": "Inline Windows‑specific unsafe ACL code",
    "locations": [
      { "path": "src/cli/config/paths.rs", "line": 266 }
    ],
    "evidence": "`restrict_to_owner` contains a long block of unsafe Windows API calls inside a general‑purpose paths module, mixing platform‑specific safety concerns with path resolution and file writes.",
    "body": "Extract the function to a dedicated `src/cli/config/platform/windows.rs` file (conditional compilation) so it can be maintained in isolation.",
    "category": "maintainability"
  },
  {
    "source": "review",
    "severity": "Low",
    "title": "Model tests file mixes live integration tests with unit tests",
    "locations": [
      { "path": "src/agent/model/tests.rs", "line": 1 }
    ],
    "evidence": "841‑line test file contains many fast unit tests and also several `#[ignore]` live integration tests. Unit test runs still compile the live test scaffolding.",
    "body": "Move live integration tests to a separate file (`live_tests.rs`) to keep unit tests focused and enable faster isolated test compilation.",
    "category": "maintainability"
  }
]
```
