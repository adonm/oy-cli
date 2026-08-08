# justfile for oy-cli — autonomous OpenCode agent and deterministic repository workflows
#
# Run `just` or `just --list` to see available recipes.
#
# Quick start:
#   just dev            # fast checks (fmt + cargo check)
#   just opencode-dev   # launch OpenCode with the checkout plugin
#   just check          # standard local checks plus the mdBook site
#   just docs           # build the mdBook site into book/
#   just fix            # auto-fix formatting and clippy lints, then check
#   just run -- --help
#
# Requires: cargo, rustc >= 1.96, Node.js 24, just, and mdbook. `mise install`
# provides them. The extended suite also requires cargo-nextest and nightly Miri.

_default:
    @just --list

# === Development checks ===

# Fast development check: format + cargo check (no recompilation across subcommands).
dev: _fmt-check
    cargo check --locked

# Standard local check suite. Uses stable Cargo only so it works after `mise install`.
check: _version-check _fmt-check _clippy _test _rustdoc _book _help-smoke _installer-smoke _opencode-package
    @echo "✓ local checks passed"

# Extended local suite using CI's nextest and Miri runners.
ci: _version-check _fmt-check _clippy _nextest _miri _rustdoc _book _help-smoke _installer-smoke _opencode-package
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

# Launch a private OpenCode instance with the checkout's npm plugin.
# Extra arguments are passed to opencode2, for example `just opencode-dev models`.
opencode-dev *args:
    @set -eu; \
    test -d packages/opencode/node_modules/@stablekernel/opencode-cursor || npm ci --prefix packages/opencode --ignore-scripts; \
    root=$(mktemp -d "$(pwd)/.tmp/opencode-dev.XXXXXX"); \
    trap 'rm -rf "$root"' EXIT INT TERM; \
    mkdir -p "$root/config" "$root/config-home" "$root/home" "$root/cache" "$root/data" "$root/state"; \
    { \
      printf '%s\n' '{' '  "$schema": "https://opencode.ai/config.json",'; \
      printf '  "plugins": ["%s"]\n' "$(pwd)/packages/opencode/index.js"; \
      printf '%s\n' '}'; \
    } >"$root/config/opencode.json"; \
    if command -v opencode2 >/dev/null 2>&1; then \
      opencode_bin=$(command -v opencode2); \
    else \
      if ! MISE_CONFIG_FILE=/dev/null MISE_CONFIG_DIR=/dev/null MISE_GLOBAL_CONFIG_FILE=/dev/null mise exec node@latest -- opencode2 --version >/dev/null 2>&1; then \
        printf '%s\n' 'opencode2 is missing from node@latest; installing @opencode-ai/cli@next...' >&2; \
        MISE_CONFIG_FILE=/dev/null MISE_CONFIG_DIR=/dev/null MISE_GLOBAL_CONFIG_FILE=/dev/null mise exec node@latest -- npm install --global @opencode-ai/cli@next; \
      fi; \
      opencode_bin=$(MISE_CONFIG_FILE=/dev/null MISE_CONFIG_DIR=/dev/null MISE_GLOBAL_CONFIG_FILE=/dev/null mise exec node@latest -- sh -c 'command -v opencode2'); \
    fi; \
    export HOME="$root/home" OPENCODE_CONFIG_DIR="$root/config" XDG_CONFIG_HOME="$root/config-home" XDG_CACHE_HOME="$root/cache" XDG_DATA_HOME="$root/data" XDG_STATE_HOME="$root/state"; \
    "$opencode_bin" --standalone {{args}}

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
    cargo run --locked -- run --help
    cargo run --locked -- audit prepare --help
    cargo run --locked -- audit finalize --help
    cargo run --locked -- review prepare --help
    cargo run --locked -- review finalize --help
    cargo run --locked -- audit --help
    cargo run --locked -- review --help
    cargo run --locked -- enhance --help
    cargo run --locked -- recover --help
    cargo run --locked -- doctor --help
    cargo run --locked -- upgrade --help

# Exercise installer sequencing and pins with a fake mise executable.
_installer-smoke:
    sh scripts/test_install.sh

# Check release-facing version pins against Cargo.toml.
_version-check:
    python3 scripts/check_versions.py

# Build, test, audit, and inspect the publishable OpenCode package.
_opencode-package:
    npm ci --prefix packages/opencode --ignore-scripts
    npm --prefix packages/opencode run build
    npm --prefix packages/opencode test
    npm audit --prefix packages/opencode --omit=dev
    cd packages/opencode && npm pack --dry-run

# === Release preparation ===

# Verify the crate can be packaged for publishing.
package:
    cargo package --locked

# === Run the binary ===

# Run oy with arguments. Example: just run -- doctor
run *args:
    cargo run --locked -- {{args}}
