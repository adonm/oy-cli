# justfile for oy-cli — autonomous OpenCode agent and deterministic repository workflows
#
# Run `just` or `just --list` to see available recipes.
#
# Quick start:
#   just dev            # fast checks (fmt + cargo check)
#   just check          # standard local checks plus the mdBook site
#   just docs           # build the mdBook site into book/
#   just fix            # auto-fix formatting and clippy lints, then check
#   just run -- --help
#
# Requires: cargo, rustc >= 1.96, just, and mdbook. `mise install` provides
# them. Optional CI-parity recipes below require cargo-nextest and/or nightly Miri.

_default:
    @just --list

# === Development checks ===

# Fast development check: format + cargo check (no recompilation across subcommands).
dev: _fmt-check
    cargo check --locked

# Standard local check suite. Uses stable Cargo only so it works after `mise install`.
check: _fmt-check _clippy _test _rustdoc _book _help-smoke _installer-smoke
    @echo "✓ local checks passed"

# Optional CI-parity suite. Requires cargo-nextest and nightly Miri.
ci: _fmt-check _clippy _nextest _miri _rustdoc _book _help-smoke
    @echo "✓ CI-parity checks passed"

# Auto-format, apply clippy suggestions, update lockfile, then run the local suite.
fix: _fmt _clippy-fix
    cargo update --workspace
    @just check

# Validate the local LLM evaluation corpus without provider/model calls.
eval:
    python3 scripts/eval_runner.py validate

# Build user/contributor documentation into book/.
docs: _book

# Run local prompt evaluations. Example: just eval-run --dry-run --task zuko-remote-pty-precision-audit
eval-run *args:
    python3 scripts/eval_runner.py run {{args}}

# Compare two completed eval runs. Example: just eval-compare .tmp/eval/runs/base .tmp/eval/runs/new
eval-compare baseline candidate:
    python3 scripts/eval_runner.py compare {{baseline}} {{candidate}}

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

# Build the mdBook user/contributor site.
_book:
    rm -rf book
    mdbook build
    cp docs/install.sh book/install.sh
    sh -n book/install.sh
    test -f book/index.html
    test -f book/getting-started.html
    test -f book/reference.html

# Smoke-test the CLI help output.
_help-smoke:
    cargo run --locked -- --help
    cargo run --locked -- setup --help
    cargo run --locked -- open --help
    cargo run --locked -- mcp --help
    cargo run --locked -- run --help
    cargo run --locked -- audit prepare --help
    cargo run --locked -- audit finalize --help
    cargo run --locked -- review prepare --help
    cargo run --locked -- review finalize --help
    cargo run --locked -- chat --help
    cargo run --locked -- audit --help
    cargo run --locked -- review --help
    cargo run --locked -- enhance --help
    cargo run --locked -- recover --help
    cargo run --locked -- model --help
    cargo run --locked -- doctor --help
    cargo run --locked -- upgrade --help

# Exercise installer sequencing and pins with a fake mise executable.
_installer-smoke:
    sh scripts/test_install.sh

# === Release preparation ===

# Verify the crate can be packaged for publishing.
package:
    cargo package --locked

# === Run the binary ===

# Run oy with arguments. Example: just run -- mcp
run *args:
    cargo run --locked -- {{args}}
