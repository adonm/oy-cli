#!/bin/sh
set -eu

# Deliberate empty CDPATH for portable cd.
# shellcheck disable=SC1007
repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
oy_version=$(awk -F '"' '/^version = "/ { print $2; exit }' "$repo_root/Cargo.toml")
[ -n "$oy_version" ] || {
  printf '%s\n' "could not read package version from Cargo.toml" >&2
  exit 1
}
tmp=$repo_root/.tmp/install-test.$$
trap 'rm -rf "$tmp"' EXIT HUP INT TERM
mkdir -p "$tmp/bin"

cat >"$tmp/mise-mock" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$OY_INSTALL_TEST_LOG"
case "$*" in
*"-- oy --version") printf 'oy-cli %s\n' "$OY_INSTALL_TEST_VERSION" ;;
esac
exit 0
EOF
chmod +x "$tmp/mise-mock"

cat >"$tmp/bin/curl" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$OY_INSTALL_TEST_CURL_LOG"
output=
take_output=0
for arg in "$@"; do
  if [ "$take_output" -eq 1 ]; then
    output=$arg
    take_output=0
  elif [ "$arg" = "-o" ]; then
    take_output=1
  fi
done

cat_mise_installer() {
  cat <<'INSTALL'
mkdir -p "$HOME/.local/bin"
cp "$OY_INSTALL_TEST_MISE_SOURCE" "$HOME/.local/bin/mise"
chmod +x "$HOME/.local/bin/mise"
INSTALL
}

if [ -n "$output" ]; then
  cat_mise_installer >"$output"
else
  cat_mise_installer
fi
EOF
chmod +x "$tmp/bin/curl"

cat >"$tmp/bin/sleep" <<'EOF'
#!/bin/sh
exit 0
EOF
chmod +x "$tmp/bin/sleep"

assert_contains() {
  case "$1" in
  *"$2"*) ;;
  *)
    printf 'missing installer invocation: %s\n' "$2" >&2
    exit 1
    ;;
  esac
}

assert_not_contains() {
  case "$1" in
  *"$2"*)
    printf 'unexpected installer invocation: %s\n' "$2" >&2
    exit 1
    ;;
  *) ;;
  esac
}

run_install() {
  log_file=$1
  skip_setup=$2
  with_mise=$3
  home=$4
  scope=$5
  shift 5
  : >"$log_file"
  : >"$log_file.curl"
  mkdir -p "$home"
  if [ "$with_mise" -eq 1 ]; then
    cp "$tmp/mise-mock" "$tmp/bin/mise"
  else
    rm -f "$tmp/bin/mise"
  fi
  # Exported overrides so the installer starts from a clean state.
  # shellcheck disable=SC2034,SC1007
  MISE_CONFIG_DIR= MISE_GLOBAL_CONFIG_FILE= XDG_CONFIG_HOME="$home/.config"
  PATH="$tmp/bin:/usr/bin:/bin" \
    HOME="$home" \
    SHELL=/bin/bash \
    OY_INSTALL_TEST_LOG="$log_file" \
    OY_INSTALL_TEST_CURL_LOG="$log_file.curl" \
    OY_INSTALL_TEST_MISE_SOURCE="$tmp/mise-mock" \
    OY_INSTALL_TEST_VERSION="$oy_version" \
    OY_INSTALL_SCOPE="$scope" \
    OY_SKIP_SETUP="$skip_setup" \
    sh "$repo_root/docs/install.sh" "$@" >/dev/null
}

default_log="$tmp/default.log"
run_install "$default_log" 1 1 "$tmp/home-default" ""
default=$(cat "$default_log")
assert_contains "$default" "use --global --yes --minimum-release-age 0 github:adonm/oy-cli@$oy_version"
assert_contains "$default" "use --global --yes --minimum-release-age 0 aqua:XAMPPRocky/tokei@12.1.2 github:universal-ctags/ctags-nightly-build[matching=.release.tar.gz]"
assert_contains "$default" "exec github:adonm/oy-cli@$oy_version -- oy --version"
assert_contains "$default" "unuse --global --yes --no-prune cargo:oy-cli cargo:tokei github:universal-ctags/ctags"
assert_contains "$default" "reshim"
assert_contains "$default" "prune --yes --tools github:adonm/oy-cli cargo:oy-cli cargo:tokei github:universal-ctags/ctags"
assert_not_contains "$default" "npm install -g"
assert_not_contains "$default" "node@latest"
assert_not_contains "$default" "npm:@opencode-ai/cli"
assert_not_contains "$default" "config ls"
assert_not_contains "$default" "allow_builds"
assert_not_contains "$default" "opencode2"
assert_not_contains "$(cat "$default_log.curl")" "https://cursor.com/install"

workspace_log="$tmp/workspace.log"
run_install "$workspace_log" 1 1 "$tmp/home-workspace" "" --workspace
workspace=$(cat "$workspace_log")
assert_contains "$workspace" "use --yes --minimum-release-age 0 github:adonm/oy-cli@$oy_version"
assert_not_contains "$workspace" "use --global"
assert_contains "$workspace" "unuse --yes --no-prune cargo:oy-cli cargo:tokei github:universal-ctags/ctags"
assert_not_contains "$workspace" "unuse --global"

env_workspace_log="$tmp/env-workspace.log"
run_install "$env_workspace_log" 1 1 "$tmp/home-env-workspace" workspace
env_workspace=$(cat "$env_workspace_log")
assert_contains "$env_workspace" "use --yes --minimum-release-age 0 github:adonm/oy-cli@$oy_version"
assert_not_contains "$env_workspace" "use --global"

setup_log="$tmp/setup.log"
run_install "$setup_log" 0 1 "$tmp/home-setup" ""
setup=$(cat "$setup_log")
assert_contains "$setup" "exec github:adonm/oy-cli@$oy_version -- oy setup"

bootstrap_log="$tmp/bootstrap.log"
run_install "$bootstrap_log" 1 0 "$tmp/home-bootstrap" ""
bootstrap_curl=$(cat "$bootstrap_log.curl")
assert_contains "$bootstrap_curl" "-fsSL https://mise.run/bash"
[ -x "$tmp/home-bootstrap/.local/bin/mise" ] || {
  printf 'shell-specific mise bootstrap did not install mise\n' >&2
  exit 1
}

help=$(sh "$repo_root/docs/install.sh" --help)
assert_contains "$help" "--global"
assert_contains "$help" "--workspace"
assert_contains "$help" "--yes"
assert_not_contains "$help" "cursor"
assert_not_contains "$help" "opencode"
assert_not_contains "$help" "both"
if sh "$repo_root/docs/install.sh" --global --workspace >/dev/null 2>&1; then
  printf 'installer accepted conflicting scopes\n' >&2
  exit 1
fi

printf 'installer smoke passed\n'
