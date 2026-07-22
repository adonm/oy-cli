#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
tmp=$repo_root/.tmp/install-test.$$
trap 'rm -rf "$tmp"' EXIT HUP INT TERM
mkdir -p "$tmp/bin"

cat >"$tmp/mise-mock" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$OY_INSTALL_TEST_LOG"
case "$*" in
*"-- oy --version") printf '%s\n' 'oy-cli 0.13.7' ;;
*"-- opencode2 api v2.plugin.list"*)
  count=$(cat "$OY_INSTALL_TEST_PLUGIN_COUNT")
  count=$((count + 1))
  printf '%s\n' "$count" >"$OY_INSTALL_TEST_PLUGIN_COUNT"
  if [ "$count" -ge 4 ]; then
    printf '%s\n' '{"data":[{"id":"oy"}]}'
  else
    printf '%s\n' '{"data":[]}'
  fi
  ;;
*"-- opencode2 --version") printf '%s\n' 'opencode2 v0.0.0-next-15353' ;;
esac
exit 0
EOF
chmod +x "$tmp/mise-mock"

cat >"$tmp/bin/curl" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$OY_INSTALL_TEST_CURL_LOG"
cat <<'INSTALL'
mkdir -p "$HOME/.local/bin"
cp "$OY_INSTALL_TEST_MISE_SOURCE" "$HOME/.local/bin/mise"
chmod +x "$HOME/.local/bin/mise"
INSTALL
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
  log=$1
  skip_setup=$2
  with_mise=$3
  home=$4
  : >"$log"
  : >"$log.curl"
  printf '%s\n' 0 >"$tmp/plugin-count"
  mkdir -p "$home"
  if [ "$with_mise" -eq 1 ]; then
    cp "$tmp/mise-mock" "$tmp/bin/mise"
  else
    rm -f "$tmp/bin/mise"
  fi
  PATH="$tmp/bin:/usr/bin:/bin" \
    HOME="$home" \
    XDG_CONFIG_HOME="$home/.config" \
    MISE_CONFIG_DIR= \
    MISE_GLOBAL_CONFIG_FILE= \
    SHELL=/bin/bash \
    OY_INSTALL_TEST_LOG="$log" \
    OY_INSTALL_TEST_CURL_LOG="$log.curl" \
    OY_INSTALL_TEST_MISE_SOURCE="$tmp/mise-mock" \
    OY_INSTALL_TEST_PLUGIN_COUNT="$tmp/plugin-count" \
    OY_SKIP_SETUP="$skip_setup" \
    sh "$repo_root/docs/install.sh" >/dev/null
}

default_log="$tmp/default.log"
run_install "$default_log" 1 1 "$tmp/home-default"
default=$(cat "$default_log")
assert_contains "$default" "use --global --yes --minimum-release-age 0 github:adonm/oy-cli@0.13.7 node@latest"
assert_contains "$default" "exec node@latest -- npm install -g @opencode-ai/cli@next"
assert_contains "$default" "exec github:adonm/oy-cli@0.13.7 -- oy --version"
assert_contains "$default" "exec node@latest -- opencode2 --version"
assert_contains "$default" "unuse --global --yes --no-prune cargo:oy-cli npm:@opencode-ai/cli cargo:tokei github:universal-ctags/ctags"
assert_contains "$default" "prune --yes --tools github:adonm/oy-cli cargo:oy-cli npm:@opencode-ai/cli cargo:tokei github:universal-ctags/ctags"
assert_contains "$default" "aqua:XAMPPRocky/tokei@12.1.2"
assert_contains "$default" "github:universal-ctags/ctags-nightly-build[matching=.release.tar.gz]"
assert_not_contains "$default" "rust@"
assert_not_contains "$default" "cargo-binstall"
assert_not_contains "$default" "bootstrap mise-shell-activate"

setup_log="$tmp/setup.log"
run_install "$setup_log" 0 1 "$tmp/home-setup"
setup=$(cat "$setup_log")
assert_not_contains "$setup" "exec -- oy setup --remove"
assert_contains "$setup" "exec github:adonm/oy-cli@0.13.7 node@latest -- oy setup"
assert_contains "$setup" "exec node@latest -- opencode2 service start"
assert_contains "$setup" "exec node@latest -- opencode2 api v2.plugin.list"

bootstrap_log="$tmp/bootstrap.log"
run_install "$bootstrap_log" 1 0 "$tmp/home-bootstrap"
bootstrap_curl=$(cat "$bootstrap_log.curl")
assert_contains "$bootstrap_curl" "-fsSL https://mise.run/bash"
[ -x "$tmp/home-bootstrap/.local/bin/mise" ] || {
  printf 'shell-specific mise bootstrap did not install mise\n' >&2
  exit 1
}

printf 'installer smoke passed\n'
