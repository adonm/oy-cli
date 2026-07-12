#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
tmp=$repo_root/.tmp/install-test.$$
trap 'rm -rf "$tmp"' EXIT HUP INT TERM
mkdir -p "$tmp/bin" "$tmp/home"

cat >"$tmp/bin/mise" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$OY_INSTALL_TEST_LOG"
case "$*" in
"exec -- oy --version") printf '%s\n' 'oy-cli 0.13.5' ;;
*"exec -- opencode2 api v2.plugin.list"*)
  count=$(cat "$OY_INSTALL_TEST_PLUGIN_COUNT")
  count=$((count + 1))
  printf '%s\n' "$count" >"$OY_INSTALL_TEST_PLUGIN_COUNT"
  if [ "$count" -ge 4 ]; then
    printf '%s\n' '{"data":[{"id":"oy"}]}'
  else
    printf '%s\n' '{"data":[]}'
  fi
  ;;
"exec -- opencode2 --version") printf '%s\n' 'opencode2 v0.0.0-next-15353' ;;
esac
exit 0
EOF
chmod +x "$tmp/bin/mise"

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
  : >"$log"
  printf '%s\n' 0 >"$tmp/plugin-count"
  PATH="$tmp/bin:/usr/bin:/bin" \
    HOME="$tmp/home" \
    XDG_CONFIG_HOME="$tmp/home/.config" \
    MISE_CONFIG_DIR= \
    MISE_GLOBAL_CONFIG_FILE= \
    SHELL=/bin/bash \
    OY_INSTALL_TEST_LOG="$log" \
    OY_INSTALL_TEST_PLUGIN_COUNT="$tmp/plugin-count" \
    OY_SKIP_SETUP="$skip_setup" \
    sh "$repo_root/docs/install.sh" >/dev/null
}

default_log="$tmp/default.log"
run_install "$default_log" 1
default=$(cat "$default_log")
assert_contains "$default" "use --global --yes node@24 rust@1.96"
assert_contains "$default" "cargo:oy-cli@0.13.5"
assert_contains "$default" "npm:@opencode-ai/cli@0.0.0-next-15353"
assert_contains "$default" "exec -- oy --version"
assert_contains "$default" "exec -- opencode2 --version"
assert_contains "$default" "prune --yes --tools cargo:oy-cli npm:@opencode-ai/cli"
assert_contains "$default" "config set --cd $tmp/home --file $tmp/home/.config/mise/config.toml --type bool bootstrap.mise_shell_activate.bash true"
assert_contains "$default" "bootstrap mise-shell-activate apply --cd $tmp/home --yes"
assert_contains "$default" "cargo:tokei"
assert_contains "$default" "universal-ctags"

setup_log="$tmp/setup.log"
run_install "$setup_log" 0
setup=$(cat "$setup_log")
assert_not_contains "$setup" "exec -- oy setup --remove"
assert_contains "$setup" "exec -- oy setup"
assert_contains "$setup" "exec -- opencode2 service start"
assert_contains "$setup" "exec -- opencode2 api v2.plugin.list"

printf 'installer smoke passed\n'
