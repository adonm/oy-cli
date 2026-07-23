#!/bin/sh
set -eu

# Install or upgrade oy, an agent host, and compact context helpers.
#
# Intended curl usage:
#   curl -fsSL https://oy.adonm.dev/install.sh | sh
#   curl -fsSL https://oy.adonm.dev/install.sh | sh -s -- --cursor
#   curl -fsSL https://oy.adonm.dev/install.sh | sh -s -- --both
#
# Environment knobs:
#   OY_SKIP_SETUP  set to 1/true to skip `oy setup`
#   OY_INSTALL_TARGET  opencode (default), cursor, or both; a flag overrides it

oy_version="0.14.0"
oy_tool="github:adonm/oy-cli@$oy_version"
node_tool="node@latest"
opencode_package="@opencode-ai/cli@next"
cursor_install_url="https://cursor.com/install"
tokei_tool="aqua:XAMPPRocky/tokei@12.1.2"
ctags_tool="github:universal-ctags/ctags-nightly-build[matching=.release.tar.gz]"

log() {
  printf '%s\n' "$*" >&2
}

die() {
  log "error: $*"
  exit 1
}

show_help() {
  cat <<'EOF'
Install oy and an agent-host integration.

Usage:
  install.sh [--opencode|--cursor|--both]

Targets:
  --opencode  Install OpenCode 2 and its oy plugin (default)
  --cursor    Install Cursor CLI and Cursor oy assets
  --both      Install and configure both hosts

Environment:
  OY_INSTALL_TARGET  opencode, cursor, or both; an explicit flag wins
  OY_SKIP_SETUP      1/true skips integration setup and runtime load checks
EOF
}

target_flag=
while [ "$#" -gt 0 ]; do
  case "$1" in
  --opencode) requested_target=opencode ;;
  --cursor) requested_target=cursor ;;
  --both) requested_target=both ;;
  -h | --help)
    show_help
    exit 0
    ;;
  *) die "unknown installer option: $1" ;;
  esac
  [ -z "$target_flag" ] || die "choose only one of --opencode, --cursor, or --both"
  target_flag=$requested_target
  shift
done

target=${target_flag:-${OY_INSTALL_TARGET:-opencode}}
case "$target" in
opencode)
  install_opencode=1
  install_cursor=0
  ;;
cursor)
  install_opencode=0
  install_cursor=1
  ;;
both)
  install_opencode=1
  install_cursor=1
  ;;
*) die "OY_INSTALL_TARGET must be opencode, cursor, or both" ;;
esac

case "$(uname -s)" in
Linux | Darwin) ;;
*) die "oy supports Linux and macOS only; Windows users should run the installer in WSL2" ;;
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

log "Installing/upgrading oy with mise..."
if [ "$install_opencode" -eq 1 ]; then
  "$mise_bin" use --global --yes --minimum-release-age 0 \
    "$oy_tool" \
    "$node_tool"
else
  "$mise_bin" use --global --yes --minimum-release-age 0 \
    "$oy_tool"
fi

if [ "$install_opencode" -eq 1 ]; then
  log "Installing OpenCode 2 with npm as documented upstream..."
  "$mise_bin" exec "$node_tool" -- npm install -g "$opencode_package"
fi

find_cursor_agent() {
  if command -v agent >/dev/null 2>&1; then
    command -v agent
  elif [ -x "$HOME/.local/bin/agent" ]; then
    printf '%s\n' "$HOME/.local/bin/agent"
  else
    return 1
  fi
}

install_cursor_cli() {
  command -v bash >/dev/null 2>&1 \
    || die "Cursor's official CLI installer requires bash"
  cursor_installer=$(mktemp "${TMPDIR:-/tmp}/oy-cursor-install.XXXXXX") \
    || die "failed to create a temporary Cursor installer file"
  trap 'rm -f "$cursor_installer"' EXIT HUP INT TERM
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$cursor_install_url" -o "$cursor_installer" \
      || die "failed to download Cursor's official CLI installer"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$cursor_installer" "$cursor_install_url" \
      || die "failed to download Cursor's official CLI installer"
  else
    die "installing Cursor CLI requires curl or wget"
  fi
  bash "$cursor_installer" \
    || die "Cursor's official CLI installer failed"
  rm -f "$cursor_installer"
  trap - EXIT HUP INT TERM
}

if [ "$install_cursor" -eq 1 ]; then
  log "Installing/upgrading Cursor CLI with Cursor's official installer..."
  install_cursor_cli
fi

log "Installing optional prebuilt context helpers..."
if ! "$mise_bin" use --global --yes --minimum-release-age 0 \
  "$tokei_tool" \
  "$ctags_tool"; then
  log "Warning: optional context helpers could not be installed; rerun this installer later."
fi

log "Removing superseded source/package-manager tool entries..."
if [ "$install_opencode" -eq 1 ]; then
  "$mise_bin" unuse --global --yes --no-prune \
    cargo:oy-cli \
    "npm:@opencode-ai/cli" \
    cargo:tokei \
    github:universal-ctags/ctags
else
  "$mise_bin" unuse --global --yes --no-prune \
    cargo:oy-cli \
    cargo:tokei \
    github:universal-ctags/ctags
fi
"$mise_bin" reshim

installed_oy_version=$("$mise_bin" exec "$oy_tool" -- oy --version 2>/dev/null) \
  || die "oy installed, but oy --version failed"
case "$installed_oy_version" in
*"$oy_version"*) ;;
*) die "expected oy $oy_version after install, got: $installed_oy_version" ;;
esac

if [ "$install_opencode" -eq 1 ]; then
  installed_opencode_version=$("$mise_bin" exec "$node_tool" -- opencode2 --version 2>/dev/null) \
    || die "OpenCode 2 installed, but opencode2 --version failed"
  case "$installed_opencode_version" in
  *"0.0.0-next-"[0-9]*) ;;
  *) die "expected an OpenCode 2 next-channel build after install, got: $installed_opencode_version" ;;
  esac

  log "Stopping any older OpenCode background service..."
  if ! "$mise_bin" exec "$node_tool" -- opencode2 service stop >/dev/null 2>&1; then
    log "No running OpenCode service needed stopping."
  fi
fi

if [ "$install_cursor" -eq 1 ]; then
  cursor_agent_bin=$(find_cursor_agent || true)
  [ -n "$cursor_agent_bin" ] \
    || die "Cursor CLI installed, but no agent executable was found on PATH or at ~/.local/bin/agent"
  installed_cursor_version=$("$cursor_agent_bin" --version 2>/dev/null) \
    || die "Cursor CLI installed, but agent --version failed"
  [ -n "$installed_cursor_version" ] \
    || die "Cursor CLI returned an empty version"
fi

log "Pruning unreferenced old tool versions..."
if [ "$install_opencode" -eq 1 ]; then
  prune_status=0
  "$mise_bin" prune --yes --tools \
    github:adonm/oy-cli \
    cargo:oy-cli \
    "npm:@opencode-ai/cli" \
    cargo:tokei \
    github:universal-ctags/ctags || prune_status=$?
else
  prune_status=0
  "$mise_bin" prune --yes --tools \
    github:adonm/oy-cli \
    cargo:oy-cli \
    cargo:tokei \
    github:universal-ctags/ctags || prune_status=$?
fi
if [ "$prune_status" -ne 0 ]; then
  log "Warning: mise could not prune old versions; the newly installed versions remain active."
fi

case "${OY_SKIP_SETUP:-}" in
1 | true | TRUE | yes | YES)
  log "Skipping oy setup because OY_SKIP_SETUP is set."
  ;;
*)
  if [ "$install_opencode" -eq 1 ]; then
    log "Installing the OpenCode integration with oy setup..."
    "$mise_bin" exec "$oy_tool" "$node_tool" -- oy setup
    log "Starting OpenCode so it can install the version-matched oy plugin..."
    "$mise_bin" exec "$node_tool" -- opencode2 service start >/dev/null \
      || die "OpenCode could not start after oy setup"
    workspace=$(pwd)
    log "Waiting for OpenCode to resolve and load the oy plugin..."
    plugin_loaded=0
    attempts=0
    while [ "$attempts" -lt 60 ]; do
      loaded_plugins=$("$mise_bin" exec "$node_tool" -- opencode2 api v2.plugin.list \
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
  fi
  if [ "$install_cursor" -eq 1 ]; then
    log "Installing the Cursor integration with oy setup --cursor..."
    "$mise_bin" exec "$oy_tool" -- oy setup --cursor
  fi
  ;;
esac

log "Done."
if [ "$installed_mise" -eq 1 ] && [ -n "$shell_target" ]; then
  log "Restart your shell to load the mise activation configured by https://mise.run/$shell_target."
fi
case "$target" in
opencode) log "Then run: oy doctor" ;;
cursor) log "Then run: agent" ;;
both) log "Then run: oy doctor, or agent" ;;
esac
