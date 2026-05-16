# Good first issues

These are limited-scope, self-contained tasks suitable for new contributors.
Each includes the file(s) to touch, the expected outcome, and the relevant
local check (`just check` covers all of them).

## 1. Add module-level doc comments to `src/tools/`

**Files:** `src/tools/workspace.rs`, `src/tools/network.rs`, `src/tools/shell.rs`,
`src/tools/preview.rs`, `src/tools/todo.rs`, `src/tools/registry.rs`,
`src/tools/schema.rs`, `src/tools/policy.rs`, `src/tools/output.rs`,
`src/tools/args.rs`

**What:** Each file is missing a `//!` module doc comment. Add a 2-5 line
comment describing what the module owns and its key entry points. Follow the
style in `src/tools.rs` (which has a good top-of-file comment block).

**Why:** New contributors (and future you) should be able to open any file and
immediately understand its role without reading the full source.

**Check:** `just check` (rustdoc uses `-D warnings`, so missing docs on public
items will fail if you add `#![warn(missing_docs)]` — keep it to module-level
only for this task).

## 2. Address `#[allow(dead_code)]` on `wrap_line` in `src/cli/ui/text.rs`

**Files:** `src/cli/ui/text.rs`

**What:** The function `wrap_line` has `#[allow(dead_code)]`. Either:
- Wire it into the markdown/diff rendering path in `src/cli/ui/render.rs` where
  line-wrapping is needed, or
- Remove it and add a short comment explaining why soft-wrap is handled
  differently.

**Why:** Dead code accumulates and confuses readers. The function exists and is
testable; it should either earn its keep or be cut.

**Check:** `just check` — clippy will flag unused functions if the allow is
removed without adding a call site.

## 3. Add unit tests for `ToolPolicy` approval matrix

**Files:** `src/tools/policy.rs` (add a `#[cfg(test)] mod tests` block)

**What:** `ToolPolicy::approval()` maps tool names to `Approval` values based on
the policy's `files` and `shell` fields. This is pure logic with 4 modes × 5+
tool names. Add exhaustive table-driven tests covering:
- Every `SafetyMode` → `ToolPolicy` conversion
- `approval("bash")` for each policy
- `approval("replace")` for each policy
- `approval("todo")` / `approval("todo_persist")` for each policy

**Why:** The approval matrix is a trust boundary. It must be correct for every
mode, and table-driven tests are the easiest way to prove it.

**Check:** `just check` (includes `cargo nextest run --all-targets --locked --profile ci`)

## 4. Add regression tests for `webfetch` URL/IP classification

**Files:** `src/tools/network.rs` (add a `#[cfg(test)] mod tests` block)

**What:** `webfetch` must reject non-public URLs (localhost, private, link-local,
reserved, multicast). Add tests that exercise the URL/IP validation path with:
- Allowed: public IPv4, public IPv6, public hostname
- Denied: 127.0.0.1, ::1, 10.x, 172.16-31.x, 192.168.x, 169.254.x, 224-239.x
  (multicast), 0.0.0.0, IPv4-mapped IPv6 loopback

**Why:** This is explicitly called out in `docs/tool-safety.md` and
`docs/architecture.md` as needing regression coverage. The validation is a
security boundary.

**Check:** `just check`

## 5. Add snapshot test for `oy doctor` help output

**Files:** `src/cli/app/doctor_cmd.rs` (or wherever snapshot tests live)

**What:** The CI smoke-test runs `oy doctor --help` but there's no snapshot
coverage. Add an `insta` snapshot test similar to the existing chat-help and
tool-preview snapshots.

**Why:** Doctor output changes should be visible in diff form during review.
Snapshot coverage catches accidental changes to the setup/status UX.

**Check:** `just check` (includes `cargo nextest run --all-targets --locked --profile ci`)

---

## How to pick one

1. Comment on the issue (or the tracking issue) that you're working on it.
2. Follow the flow in `CONTRIBUTING.md`:
   - Inspect the relevant code first.
   - Make the smallest targeted change.
   - Add or update focused tests.
   - Run `just check`.
3. Update docs/help text if user-visible behaviour changes.
