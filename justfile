# justfile for oy-cli — a local AI coding CLI
#
# Run `just` or `just --list` to see available recipes.
#
# Quick start:
#   just dev            # fast checks (fmt + cargo check)
#   just check          # full CI suite (slow — uses multiple cargo subcommands)
#   just fix            # auto-fix formatting and clippy lints
#   just run -- "summarize this repo"
#
# Requires: cargo, cargo-nextest, rustc >= 1.91.1; `just check` also needs nightly Miri

_default:
    @just --list

# === Development checks ===

# Fast development check: format + cargo check (no recompilation across subcommands).
dev: _fmt-check
    cargo check --locked

# Full local CI suite. Slow because each cargo subcommand invalidates previous
# compilation artifacts. Requires nightly Miri. Use `just dev` for quick feedback.
check: _fmt-check _clippy _test _miri _rustdoc _help-smoke
    @echo "✓ all checks passed"

# Auto-format and apply clippy suggestions, then run the full suite.
fix: _fmt _clippy-fix
    @just check

# === Individual checks (available as standalone targets) ===

# Check formatting (no changes).
_fmt-check:
    cargo fmt --check

# Apply formatting in-place.
_fmt:
    cargo fmt

# Run clippy with deny-warnings.
_clippy:
    cargo clippy --all-targets --locked -- -D warnings

# Auto-apply clippy fixes.
_clippy-fix:
    cargo clippy --all-targets --locked --fix --allow-dirty --allow-staged

# Run all non-doc tests with nextest, then run rustdoc examples/tests.
_test:
    cargo nextest run --all-targets --locked --profile ci
    cargo test --doc --locked

# Run focused smoke tests under Miri on nightly to catch undefined behavior.
_miri:
    cargo +nightly miri test --locked miri_smoke

# Build docs with deny-warnings (no deps).
_rustdoc:
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked

# Smoke-test the CLI help output.
_help-smoke:
    cargo run --locked -- --help
    cargo run --locked -- run --help
    cargo run --locked -- chat --help
    cargo run --locked -- audit --help
    cargo run --locked -- model --help
    cargo run --locked -- doctor --help

# === Release preparation ===

# Verify the crate can be packaged for publishing.
package:
    cargo package --locked

# === Run the binary ===

# Run oy with arguments. Example: just run -- chat
run *args:
    cargo run --locked -- {{args}}
