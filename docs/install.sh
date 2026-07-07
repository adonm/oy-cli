#!/bin/sh
set -eu

# Install or upgrade oy and its optional local helpers with mise.
#
# Intended curl usage:
#   curl -fsSL https://wagov-dtt.github.io/oy-cli/install.sh | sh
#
# Environment knobs:
#   OY_MISE_MINIMUM_RELEASE_AGE  mise age filter; default 0 for freshest releases
#   OY_SKIP_SETUP                set to 1/true to skip `oy setup`

minimum_release_age="${OY_MISE_MINIMUM_RELEASE_AGE:-0}"

log() {
  printf '%s\n' "$*" >&2
}

die() {
  log "error: $*"
  exit 1
}

find_mise() {
  if command -v mise >/dev/null 2>&1; then
    command -v mise
  elif [ -x "$HOME/.local/bin/mise" ]; then
    printf '%s\n' "$HOME/.local/bin/mise"
  else
    return 1
  fi
}

install_mise() {
  log "Installing mise..."
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL https://mise.run | sh
  elif command -v wget >/dev/null 2>&1; then
    wget -qO- https://mise.run | sh
  else
    die "mise is not installed and neither curl nor wget is available"
  fi
}

mise_bin="$(find_mise || true)"
if [ -z "$mise_bin" ]; then
  install_mise
  mise_bin="$(find_mise || true)"
fi
[ -n "$mise_bin" ] || die "mise installed, but no mise executable was found on PATH or at ~/.local/bin/mise"

mise_bin_dir="$(dirname "$mise_bin")"
mise_shims_dir="${MISE_DATA_DIR:-$HOME/.local/share/mise}/shims"
PATH="$mise_bin_dir:$mise_shims_dir:$PATH"
export PATH

log "Updating mise itself when supported..."
if "$mise_bin" self-update --yes; then
  mise_bin="$(find_mise || true)"
  [ -n "$mise_bin" ] || die "mise self-update completed, but no mise executable was found"
else
  log "Skipping mise self-update; this is normal for package-manager installs."
fi

log "Installing/upgrading oy toolchain with mise (minimum release age: $minimum_release_age)..."

# Install cargo-binstall first so cargo-backed tools can use prebuilt binaries when available.
"$mise_bin" use --global --yes --minimum-release-age "$minimum_release_age" cargo-binstall

"$mise_bin" use --global --yes --minimum-release-age "$minimum_release_age" \
  cargo:oy-cli \
  opencode \
  cargo:tokei \
  github:universal-ctags/ctags

"$mise_bin" reshim

case "${OY_SKIP_SETUP:-}" in
1 | true | TRUE | yes | YES)
  log "Skipping oy setup because OY_SKIP_SETUP is set."
  ;;
*)
  log "Installing/updating opencode integration with oy setup..."
  "$mise_bin" exec -- oy setup
  ;;
esac

log "Done."
log "Restart your shell, or add mise shims to this session now:"
log "  export PATH=\"$mise_shims_dir:\$PATH\""
log "Then run: oy doctor"
