#!/bin/sh
set -eu

# Install or upgrade oy and its optional context helpers through mise, install
# opencode2 when it is missing, then write the oy agent skills into the
# standard cross-agent skills directory.
#
# Intended curl usage:
#   curl -fsSL https://oy.adonm.dev/install.sh | sh
#   curl -fsSL https://oy.adonm.dev/install.sh | sh -s -- --workspace
#
# Environment knobs:
#   OY_INSTALL_SCOPE  global or workspace; an explicit flag wins
#   OY_SKIP_SETUP     1/true to skip `oy setup`

oy_version="0.15.4"
oy_tool="github:adonm/oy-cli@$oy_version"
tokei_tool="aqua:XAMPPRocky/tokei@12.1.2"
ctags_tool="github:universal-ctags/ctags-nightly-build[matching=.release.tar.gz]"
opencode2_tool="npm:@opencode-ai/cli@beta"

log() {
  printf '%s\n' "$*" >&2
}

die() {
  log "error: $*"
  exit 1
}

show_help() {
  cat <<'EOF'
Install oy, its agent skills, and optional context helpers with mise,
and opencode2 when it is not already installed.

Usage:
  install.sh [--global|--workspace] [--yes]

Scope:
  --global     Add tools to mise's global config
  --workspace  Add tools to mise.toml in the current directory
  --yes        Skip the scope prompt; defaults to global when no scope is given

Environment:
  OY_INSTALL_SCOPE  global or workspace; an explicit flag wins
  OY_SKIP_SETUP     1/true skips the agent skills setup
EOF
}

scope_flag=
assume_yes=0
while [ "$#" -gt 0 ]; do
  case "$1" in
  --global) requested_scope=global ;;
  --workspace) requested_scope=workspace ;;
  --yes | -y) assume_yes=1 ;;
  -h | --help)
    show_help
    exit 0
    ;;
  *) die "unknown installer option: $1" ;;
  esac
  if [ -n "${requested_scope:-}" ] && [ -n "$scope_flag" ]; then
    die "choose only one of --global or --workspace"
  fi
  if [ -n "${requested_scope:-}" ]; then
    scope_flag=$requested_scope
    requested_scope=
  fi
  shift
done

tty_available() {
  [ "$assume_yes" -eq 0 ] || return 1
  [ -t 1 ] || return 1
  [ -r /dev/tty ] && [ -w /dev/tty ] || return 1
  ( : </dev/tty ) 2>/dev/null
}

scope=${scope_flag:-${OY_INSTALL_SCOPE:-}}
case "$scope" in
global | workspace) ;;
"")
  if tty_available; then
    printf 'Install mise-managed tools globally or in the current workspace? [G/w] ' >/dev/tty
    read -r scope_reply </dev/tty || scope_reply=
    case "$(printf '%s' "$scope_reply" | tr '[:upper:]' '[:lower:]')" in
    w | workspace) scope=workspace ;;
    "" | g | global) scope=global ;;
    *) die "choose global or workspace" ;;
    esac
  else
    scope=global
  fi
  ;;
*) die "OY_INSTALL_SCOPE must be global or workspace" ;;
esac

case "$(uname -s)" in
Linux) ;;
*) die "oy supports Linux only; use WSL2 elsewhere, or build from source" ;;
esac

find_mise() {
  if command -v mise >/dev/null 2>&1; then
    command -v mise
  elif [ -n "${MISE_INSTALL_PATH:-}" ] && [ -x "$MISE_INSTALL_PATH" ]; then
    printf '%s\n' "$MISE_INSTALL_PATH"
  elif [ -x "$HOME/.local/bin/mise" ]; then
    printf '%s\n' "$HOME/.local/bin/mise"
  else
    return 1
  fi
}

case "${SHELL:-}" in
*/bash | bash) shell_target=bash ;;
*/zsh | zsh) shell_target=zsh ;;
*/fish | fish) shell_target=fish ;;
*) shell_target= ;;
esac

install_mise() {
  if [ -n "$shell_target" ]; then
    mise_url="https://mise.run/$shell_target"
    log "Installing mise and configuring $shell_target activation..."
  else
    mise_url="https://mise.run"
    log "Installing mise (shell activation skipped because SHELL=${SHELL:-unset})..."
  fi
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$mise_url" | sh
  elif command -v wget >/dev/null 2>&1; then
    wget -qO- "$mise_url" | sh
  else
    die "mise is not installed and neither curl nor wget is available"
  fi
}

mise_bin="$(find_mise || true)"
installed_mise=0
if [ -z "$mise_bin" ]; then
  install_mise
  installed_mise=1
  mise_bin="$(find_mise || true)"
fi
[ -n "$mise_bin" ] || die "mise installed, but no mise executable was found on PATH, at MISE_INSTALL_PATH, or at ~/.local/bin/mise"

if [ "$installed_mise" -eq 0 ]; then
  log "Updating mise itself when supported..."
  if "$mise_bin" self-update --yes; then
    mise_bin="$(find_mise || true)"
    [ -n "$mise_bin" ] || die "mise self-update completed, but no mise executable was found"
  else
    log "Skipping mise self-update; this is normal for package-manager installs."
  fi
fi

mise_use() {
  if [ "$scope" = "global" ]; then
    "$mise_bin" use --global --yes --minimum-release-age 0 "$@"
  else
    "$mise_bin" use --yes --minimum-release-age 0 "$@"
  fi
}

mise_unuse() {
  if [ "$scope" = "global" ]; then
    "$mise_bin" unuse --global --yes --no-prune "$@"
  else
    "$mise_bin" unuse --yes --no-prune "$@"
  fi
}

log "Mise scope: $scope"
log "Installing/upgrading oy with mise..."
mise_use "$oy_tool"

log "Installing optional prebuilt context helpers..."
if ! mise_use "$tokei_tool" "$ctags_tool"; then
  log "Warning: optional context helpers could not be installed; rerun this installer later."
fi

if command -v opencode2 >/dev/null 2>&1; then
  log "opencode2 is already installed; skipping."
else
  log "Installing opencode2 with mise..."
  mise_use "$opencode2_tool"
  if [ "$scope" = "global" ]; then
    opencode2_config="${MISE_GLOBAL_CONFIG_FILE:-${XDG_CONFIG_HOME:-$HOME/.config}/mise/config.toml}"
  else
    opencode2_config="mise.toml"
  fi
  log "To allow non-interactive npm builds for opencode2, add this to $opencode2_config:"
  log '  [tools."npm:@opencode-ai/cli"]'
  log '  version = "beta"'
  log '  allow_builds = ["@opencode-ai/cli"]'
fi

log "Removing superseded source/package-manager tool entries..."
mise_unuse \
  cargo:oy-cli \
  cargo:tokei \
  github:universal-ctags/ctags
"$mise_bin" reshim

installed_oy_version=$("$mise_bin" exec "$oy_tool" -- oy --version 2>/dev/null) \
  || die "oy installed, but oy --version failed"
case "$installed_oy_version" in
*"$oy_version"*) ;;
*) die "expected oy $oy_version after install, got: $installed_oy_version" ;;
esac

log "Pruning unreferenced old tool versions..."
prune_status=0
"$mise_bin" prune --yes --tools \
  github:adonm/oy-cli \
  cargo:oy-cli \
  cargo:tokei \
  github:universal-ctags/ctags || prune_status=$?
if [ "$prune_status" -ne 0 ]; then
  log "Warning: mise could not prune old versions; the newly installed versions remain active."
fi

case "${OY_SKIP_SETUP:-}" in
1 | true | TRUE | yes | YES)
  log "Skipping oy setup because OY_SKIP_SETUP is set."
  ;;
*)
  log "Installing the oy agent skills with oy setup..."
  "$mise_bin" exec "$oy_tool" -- oy setup
  ;;
esac

log "Done."
if [ "$installed_mise" -eq 1 ] && [ -n "$shell_target" ]; then
  log "Restart your shell to load the mise activation configured by https://mise.run/$shell_target."
fi
if [ "$scope" = "workspace" ]; then
  log "Mise tools were added to the current workspace."
else
  log "Mise tools were added to the global config."
fi
log "Then run: oy doctor --check"
log "Then ask your agent to run the oy-setup skill to finish setup."
