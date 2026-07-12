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
"exec -- oy --version") printf '%s\n' 'oy-cli 0.13.1' ;;
*"exec -- opencode2 api v2.plugin.list"*) printf '%s\n' '{"data":[{"id":"oy"}]}' ;;
"exec -- opencode2 --version") printf '%s\n' 'opencode2 v0.0.0-next-15353' ;;
"exec -- sighthound --version") printf '%s\n' 'sighthound 1.0' ;;
esac
exit 0
EOF
chmod +x "$tmp/bin/mise"

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
  install_sighthound=$2
  skip_setup=$3
  : >"$log"
  PATH="$tmp/bin:/usr/bin:/bin" \
    HOME="$tmp/home" \
    XDG_CONFIG_HOME="$tmp/home/.config" \
    MISE_CONFIG_DIR= \
    MISE_GLOBAL_CONFIG_FILE= \
    SHELL=/bin/bash \
    OY_INSTALL_TEST_LOG="$log" \
    OY_INSTALL_SIGHTHOUND="$install_sighthound" \
    OY_SKIP_SETUP="$skip_setup" \
    sh "$repo_root/docs/install.sh" >/dev/null
}

default_log="$tmp/default.log"
run_install "$default_log" 0 1
default=$(cat "$default_log")
assert_contains "$default" "use --global --yes node@24 rust@1.96"
assert_contains "$default" "cargo:oy-cli@0.13.1"
assert_contains "$default" "npm:@opencode-ai/cli@0.0.0-next-15353"
assert_contains "$default" "exec -- oy --version"
assert_contains "$default" "exec -- opencode2 --version"
assert_contains "$default" "prune --yes --tools cargo:oy-cli npm:@opencode-ai/cli"
assert_contains "$default" "config set --cd $tmp/home --file $tmp/home/.config/mise/config.toml --type bool bootstrap.mise_shell_activate.bash true"
assert_contains "$default" "bootstrap mise-shell-activate apply --cd $tmp/home --yes"
assert_not_contains "$default" "Corgea/Sighthound"

sighthound_log="$tmp/sighthound.log"
run_install "$sighthound_log" 1 1
sighthound=$(cat "$sighthound_log")
assert_contains "$sighthound" "rust@1.96"
assert_contains "$sighthound" "bin=sighthound,locked=true"
assert_contains "$sighthound" "rev:c4608eb2b6ca256daf4dbd1e74aadc3570343685"
assert_contains "$sighthound" "exec -- sighthound --version"

reset_log="$tmp/reset.log"
run_install "$reset_log" 0 0
reset=$(cat "$reset_log")
assert_contains "$reset" "exec -- oy setup --remove"
assert_contains "$reset" "exec -- oy setup"
assert_contains "$reset" "exec -- opencode2 service start"
assert_contains "$reset" "exec -- opencode2 api v2.plugin.list"

printf 'installer smoke passed\n'
