# justfile for oy-cli — deterministic audit/review evidence preparation and agent skills
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
# Requires: cargo, rustc >= 1.96, just, and mdbook. `mise install`
# provides them. The extended suite also requires cargo-nextest and nightly Miri.

_default:
    @just --list

# === Development checks ===

# Fast development check: format + cargo check (no recompilation across subcommands).
dev: _fmt-check
    cargo check --locked

# Local install from checkout (no mise/XDG isolation): build, install to ~/.cargo/bin, refresh skills.
install:
    cargo install --path . --locked
    mkdir -p ~/.local/bin && ln -sf ~/.cargo/bin/oy ~/.local/bin/oy
    oy setup
    oy doctor --check
    @echo "Installed oy $$(~/.cargo/bin/oy --version) — skills at ~/.agents/skills — run 'oy doctor --check' and ask your agent to run the oy-setup skill"

# Standard local check suite. Uses stable Cargo only so it works after `mise install`.
check: _version-check _fmt-check _clippy _test _rustdoc _book _help-smoke _installer-smoke _shellcheck
    @echo "✓ local checks passed"

# Extended local suite using CI's nextest and Miri runners.
ci: _version-check _fmt-check _clippy _nextest _miri _rustdoc _book _help-smoke _installer-smoke _shellcheck
    @echo "✓ extended checks passed"

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

# Run the same tasks across model lanes. Repeat --model label=provider/model#variant.
eval-matrix *args:
    python3 scripts/eval_runner.py matrix {{args}}

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

# Run non-doc tests with nextest, then rustdoc tests.
_nextest:
    cargo nextest run --all-targets --locked --profile ci
    cargo test --doc --locked

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
    cargo run --locked -- audit --help
    cargo run --locked -- audit prepare --help
    cargo run --locked -- audit finalize --help
    cargo run --locked -- review --help
    cargo run --locked -- review prepare --help
    cargo run --locked -- review finalize --help
    cargo run --locked -- doctor --help
    cargo run --locked -- upgrade --help

# Exercise installer sequencing and pins with a fake mise executable.
_installer-smoke:
    sh scripts/test_install.sh

# Lint the shell scripts with shellcheck.
_shellcheck:
    shellcheck docs/install.sh scripts/test_install.sh

# Check release-facing version pins against Cargo.toml.
_version-check:
    python3 scripts/check_versions.py

# === Release preparation ===

# Bump every release-facing version pin to a new version, then verify alignment.
# Usage: just release 0.14.10
release version:
    @python3 scripts/bump_version.py {{version}}
    @python3 scripts/check_versions.py

# Verify the crate can be packaged for publishing.
package:
    cargo package --locked

# === Run the binary ===

# Run oy with arguments. Example: just run -- doctor
run *args:
    cargo run --locked -- {{args}}
