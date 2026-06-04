# Roadmap — Q3 2026

> Three-month plan addressing the five findings in `REVIEW.md` (2026-06-04).
> Each item references its REVIEW.md finding. Work is ordered by impact: security boundary → duplication elimination → decomposition → hygiene.

## Month 1 — Security boundary & input pipeline consolidation ✅ COMPLETE (2026-07)

### Week 1–2: Extract shared public-IP helper (REVIEW #1) ✅ DONE

**Problem:** `is_public_ip` in `src/tools/network.rs` and `is_loopback_or_private_ip` in `src/llm/route/auth.rs` are two independent IP classifiers that have already drifted on edge cases (IPv4-mapped-IPv6, site-local, unique-local). A future change to one could inadvertently relax the other.

**Fix:** Extract a single `is_public_ip()` helper using `ip_rfc::global` into a shared crate-private module (`src/tools/network.rs` or a new `src/net.rs`). Keep the distinct policy layers at call sites — webfetch denies non-public; credential transport permits loopback/private.

**Acceptance:**
- One `is_public_ip` definition in the crate
- Both call sites use it
- Existing webfetch IP tests and route auth tests pass unchanged
- One new test covering IPv4-mapped-IPv6 and unique-local alignment between the two call sites

### Week 3–4: Merge review input collection into audit input abstraction (REVIEW #2) ✅ DONE

**Problem:** `src/review.rs` (~666 lines) duplicates audit's file collection, chunking, token counting, oversize checks, and manifest/index construction with its own `ReviewChunk`/`DiffItem` structs instead of reusing `AuditChunk`/`AuditFile`. ~200 lines of duplicated chunking machinery. Any future improvement to audit input handling must be replicated manually.

**Fix:** Introduce a shared input trait or set of functions that accept a file or git-diff source. Extend `src/audit/input.rs` with git-diff support producing `AuditFile`-like items. Delete `ReviewChunk`, `DiffItem`, `split_git_diff_items`, `chunk_diff_items`, `ensure_chunks_fit`, `diff_manifest`, and `workspace_size_index` from `src/review.rs`. The review module's `prepare_workspace_input` and `prepare_diff_input` become thin callers of the shared abstraction.

**Acceptance:**
- `src/review.rs` drops below 400 lines
- `src/audit/input.rs` gains a git-diff source (no behavior change for existing audit path)
- All 34 audit tests and all review tests pass unchanged
- Running `oy review` produces identical output to before the change

## Month 2 — Module decomposition ✅ COMPLETE (2026-07)

### Week 1–3: Split `src/tools/preview.rs` by tool category (REVIEW #3) ✅ DONE

**Problem:** 799-line file mixing per-tool preview functions for 15+ tools with no separation between workspace, network, and process tool previews. Hard to locate the rendering for a specific tool.

**Fix:** Split into sub-modules under `src/tools/preview/`:
- `common.rs` — shared helpers (`preview_value`, `with_verbose`, `append_preview_lines`, `append_search_hits`, `compact_kvs`)
- `workspace.rs` — list, read, read_multiple_files, search, sloc, outline, replace, patch
- `network.rs` — webfetch, repo_clone
- `process.rs` — bash
- `planning.rs` — todo, think, ask, snapshot (if still registered)

Keep the registry-driven dispatch in `src/tools.rs` intact. No behavior changes.

**Acceptance:**
- No file over 400 lines in `src/tools/preview/`
- All existing preview tests pass unchanged
- `just check` clean

### Week 4: Extract Windows ACL code to platform module (REVIEW #4) ✅ DONE

**Problem:** `restrict_to_owner` in `src/cli/config/paths.rs` (lines 282-345) is a long block of `unsafe` Windows API calls inside a general-purpose paths module that also handles home-directory expansion and workspace output validation.

**Fix:** Move `restrict_to_owner` to `src/cli/config/platform/windows.rs` behind `#[cfg(windows)]`. The public API in `paths.rs` remains unchanged.

**Acceptance:**
- `restrict_to_owner` lives in a dedicated platform module
- `#[cfg(windows)]` gating is correct
- `just check` passes on Linux; Windows build confirmed via CI

## Month 3 — Test hygiene & regression prevention ✅ COMPLETE (2026-07)

### Week 1–2: Split model live tests into separate file (REVIEW #5) ✅ DONE

**Problem:** `src/agent/model/tests.rs` (741 lines) mixes fast unit tests with `#[ignore]` live integration tests. Unit test runs still compile live-test scaffolding.

**Fix:** Move functions starting with `live_` to `src/agent/model/live_tests.rs`. Gate behind `#[cfg(feature = "live-tests")]` or keep `#[ignore]`. Unit tests stay focused.

**Acceptance:**
- `cargo test -p oy-cli --lib` runs only unit tests (no live test compilation)
- `cargo test -p oy-cli --lib --ignored` runs live tests
- All existing tests pass

### Week 3–4: Regression prevention & final review pass

**Goal:** Ensure the five fixes stick and no new duplication creeps back.

**Tasks:**
- Add a focused test that `is_public_ip` has exactly one definition in the crate (compile-fail or simple grep-based CI check)
- Add a focused test that review input collection goes through audit input abstractions (no `ReviewChunk` or `DiffItem` remaining)
- Run `oy review` on the final state; all prior REVIEW.md findings should be absent or marked fixed
- Update `REVIEW.md` with a "Fixed" section documenting each closed finding
- Update `CHANGELOG.md` with the month's changes

**Acceptance:**
- Fresh `oy review` run shows zero open findings matching the five from 2026-06-04
- `ISSUES.md` and `REVIEW.md` reflect current state
- `just check` clean

## Summary

| Month | Focus | REVIEW refs | Effort |
|---|---|---|---|
| 1 | Security boundary + input consolidation | #1, #2 | Medium-high |
| 2 | Module decomposition | #3, #4 | Medium |
| 3 | Test hygiene + regression prevention | #5 + regression | Low-medium |

The two medium-severity items (#1, #2) land in month 1 because they affect security boundaries and cross-module duplication. The three low-severity decomposition items (#3, #4, #5) spread across months 2–3. Month 3 closes with a fresh `oy review` to confirm zero regressions.
