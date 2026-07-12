#!/bin/sh
set -eu

# Install or upgrade oy, OpenCode, and compact context helpers with mise.
#
# Intended curl usage:
#   curl -fsSL https://oy.adonm.dev/install.sh | sh
#
# Environment knobs:
#   OY_MISE_MINIMUM_RELEASE_AGE  mise age filter; default 0 for freshest releases
#   OY_SKIP_SETUP                set to 1/true to skip `oy setup`

minimum_release_age="${OY_MISE_MINIMUM_RELEASE_AGE:-0}"
oy_version="0.13.4"
oy_tool="cargo:oy-cli@$oy_version"
opencode_version="0.0.0-next-15353"
opencode_tool="npm:@opencode-ai/cli@$opencode_version"

log() {
  printf '%s\n' "$*" >&2
}

die() {
  log "error: $*"
  exit 1
}

case "$(uname -s)" in
Linux | Darwin) ;;
*) die "oy supports Linux and macOS only; Windows users should run the installer in WSL2" ;;
esac

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

log "Updating mise itself when supported..."
if "$mise_bin" self-update --yes; then
  mise_bin="$(find_mise || true)"
  [ -n "$mise_bin" ] || die "mise self-update completed, but no mise executable was found"
else
  log "Skipping mise self-update; this is normal for package-manager installs."
fi

log "Installing/upgrading oy toolchain with mise (minimum release age: $minimum_release_age)..."

# OpenCode's beta package uses npm; oy and tokei use Cargo.
"$mise_bin" use --global --yes node@24 rust@1.96

# Install cargo-binstall first so cargo-backed tools can use prebuilt binaries when available.
"$mise_bin" use --global --yes --minimum-release-age "$minimum_release_age" cargo-binstall

"$mise_bin" use --global --yes --minimum-release-age "$minimum_release_age" \
  "$oy_tool" \
  "$opencode_tool" \
  cargo:tokei \
  github:universal-ctags/ctags

case "${SHELL:-}" in
*/bash | bash) shell_target=bash ;;
*/zsh | zsh) shell_target=zsh ;;
*/fish | fish) shell_target=fish ;;
*) shell_target= ;;
esac

if [ -n "$shell_target" ]; then
  if [ -n "${MISE_GLOBAL_CONFIG_FILE:-}" ]; then
    mise_global_config_file=$MISE_GLOBAL_CONFIG_FILE
  elif [ -n "${MISE_CONFIG_DIR:-}" ]; then
    mise_global_config_file=$MISE_CONFIG_DIR/config.toml
  elif [ -n "${XDG_CONFIG_HOME:-}" ]; then
    mise_global_config_file=$XDG_CONFIG_HOME/mise/config.toml
  else
    mise_global_config_file=$HOME/.config/mise/config.toml
  fi

  log "Configuring $shell_target activation with mise bootstrap..."
  "$mise_bin" config set --cd "$HOME" --file "$mise_global_config_file" --type bool \
    "bootstrap.mise_shell_activate.$shell_target" true
  MISE_GLOBAL_CONFIG_FILE="$mise_global_config_file" \
    "$mise_bin" bootstrap mise-shell-activate apply --cd "$HOME" --yes
else
  log "Skipping shell activation: mise bootstrap supports bash, zsh, and fish; SHELL=${SHELL:-unset}."
fi

"$mise_bin" reshim

installed_oy_version=$("$mise_bin" exec -- oy --version 2>/dev/null) \
  || die "oy installed, but oy --version failed"
case "$installed_oy_version" in
*"$oy_version"*) ;;
*) die "expected oy $oy_version after install, got: $installed_oy_version" ;;
esac

installed_opencode_version=$("$mise_bin" exec -- opencode2 --version 2>/dev/null) \
  || die "OpenCode 2 installed, but opencode2 --version failed"
case "$installed_opencode_version" in
*"$opencode_version"*) ;;
*) die "expected OpenCode $opencode_version after install, got: $installed_opencode_version" ;;
esac

log "Stopping any older OpenCode background service..."
if ! "$mise_bin" exec -- opencode2 service stop >/dev/null 2>&1; then
  log "No running OpenCode service needed stopping."
fi

log "Pruning unreferenced old oy and OpenCode versions..."
if ! "$mise_bin" prune --yes --tools cargo:oy-cli "npm:@opencode-ai/cli"; then
  log "Warning: mise could not prune old versions; the newly installed versions remain active."
fi

case "${OY_SKIP_SETUP:-}" in
1 | true | TRUE | yes | YES)
  log "Skipping oy setup because OY_SKIP_SETUP is set."
  ;;
*)
  log "Installing the OpenCode integration with oy setup..."
  "$mise_bin" exec -- oy setup
  log "Starting OpenCode so it can install the version-matched oy plugin..."
  "$mise_bin" exec -- opencode2 service start >/dev/null \
    || die "OpenCode could not start after oy setup"
  workspace=$(pwd)
  log "Waiting for OpenCode to resolve and load the oy plugin..."
  plugin_loaded=0
  attempts=0
  while [ "$attempts" -lt 60 ]; do
    loaded_plugins=$("$mise_bin" exec -- opencode2 api v2.plugin.list \
      --param "location[directory]=$workspace" 2>/dev/null || true)
    case "$loaded_plugins" in
    *'"id":"oy"'* | *'"id": "oy"'*)
      plugin_loaded=1
      break
      ;;
    esac
    attempts=$((attempts + 1))
    sleep 2
  done
  [ "$plugin_loaded" -eq 1 ] \
    || die "OpenCode started, but the oy plugin did not load within 120 seconds; run 'oy doctor --check' for details"
  log "Verified OpenCode loaded the oy plugin."
  ;;
esac

log "Done."
log "Restart your shell to load the mise activation configured by mise bootstrap."
log "Then run: oy doctor"
