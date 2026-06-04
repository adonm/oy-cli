# Good first issues

Small, self-contained tasks suitable for new contributors. Each lists the files to touch, expected outcome, and the local check.

## Starter tasks

### 1. Extract Windows ACL code to platform module

- **Files:** `src/cli/config/paths.rs` (lines 282-345) → new `src/cli/config/platform/windows.rs`
- **Outcome:** Move `restrict_to_owner` to a dedicated platform module behind `#[cfg(windows)]`. The public API in `paths.rs` stays the same.
- **Check:** `just check` passes on Linux; Windows build confirmed via CI.
- **See:** `REVIEW.md` finding #4.

### 2. Split model live tests into separate file

- **Files:** `src/agent/model/tests.rs` → `src/agent/model/live_tests.rs`
- **Outcome:** Move functions starting with `live_` into a separate file behind `#[cfg(feature = "live-tests")]` or kept `#[ignore]`. Unit tests stay focused.
- **Check:** `cargo test -p oy-cli --lib` runs only unit tests; `cargo test -p oy-cli --lib --ignored` runs live tests.
- **See:** `REVIEW.md` finding #5.

### 3. Split preview.rs by tool category

- **Files:** `src/tools/preview.rs` → `src/tools/preview/{workspace,network,process,common}.rs`
- **Outcome:** Move per-tool preview functions into sub-modules grouped by tool category. Keep shared helpers (`preview_value`, `with_verbose`, `append_preview_lines`) in a `common` sibling.
- **Check:** All existing preview tests pass; `just check` clean.
- **See:** `REVIEW.md` finding #3.

### 4. Extract shared public-IP helper

- **Files:** `src/tools/network.rs` (`is_public_ip`), `src/llm/route/auth.rs` (`is_loopback_or_private_ip`) → new shared helper in `src/tools/network.rs` or a new crate-private module.
- **Outcome:** Single `is_public_ip()` using `ip_rfc`. Policy differences (webfetch denies non-public; credential transport permits loopback/private) stay at call sites.
- **Check:** Existing webfetch IP tests and route auth tests pass; add one test for edge-case address alignment.
- **See:** `REVIEW.md` finding #1.

## Contributing

See `CONTRIBUTING.md` for the inspect → edit → verify flow and `just check` expectations. Each task should be a single focused PR.
