# justfile for oy-cli — OpenCode launcher and oy MCP helpers
#
# Run `just` or `just --list` to see available recipes.
#
# Quick start:
#   just dev            # fast checks (fmt + cargo check)
#   just check          # standard local checks using only stable Cargo
#   just fix            # auto-fix formatting and clippy lints, then check
#   just run -- --help
#
# Requires: cargo, rustc >= 1.96, and just. Optional CI-parity recipes below
# require cargo-nextest and/or nightly Miri.

_default:
    @just --list

# === Development checks ===

# Fast development check: format + cargo check (no recompilation across subcommands).
dev: _fmt-check
    cargo check --locked

# Standard local check suite. Uses stable Cargo only so it works after `mise install`.
check: _fmt-check _clippy _test _rustdoc _help-smoke
    @echo "✓ local checks passed"

# Optional CI-parity suite. Requires cargo-nextest and nightly Miri.
ci: _fmt-check _clippy _nextest _miri _rustdoc _help-smoke
    @echo "✓ CI-parity checks passed"

# Auto-format, apply clippy suggestions, update lockfile, then run the local suite.
fix: _fmt _clippy-fix
    cargo update --workspace
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

# Run all non-doc tests with stable Cargo, then run rustdoc examples/tests.
_test:
    cargo test --all-targets --locked
    cargo test --doc --locked

# Run all non-doc tests with nextest, matching CI's test runner.
_nextest:
    cargo nextest run --all-targets --locked --profile ci

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
    cargo run --locked -- review --help
    cargo run --locked -- model --help
    cargo run --locked -- doctor --help

# === Release preparation ===

# Verify the crate can be packaged for publishing.
package:
    cargo package --locked

# === Run the binary ===

# Run oy with arguments. Example: just run -- mcp
run *args:
    cargo run --locked -- {{args}}
