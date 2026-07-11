#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
tmp=${TMPDIR:-/tmp}/oy-install-test.$$
trap 'rm -rf "$tmp"' EXIT HUP INT TERM
mkdir -p "$tmp/bin" "$tmp/home"

cat >"$tmp/bin/mise" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$OY_INSTALL_TEST_LOG"
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
  : >"$log"
  PATH="$tmp/bin:/usr/bin:/bin" \
    HOME="$tmp/home" \
    OY_INSTALL_TEST_LOG="$log" \
    OY_INSTALL_SIGHTHOUND="$install_sighthound" \
    OY_SKIP_SETUP=1 \
    sh "$repo_root/docs/install.sh" >/dev/null
}

default_log="$tmp/default.log"
run_install "$default_log" 0
default=$(cat "$default_log")
assert_contains "$default" "use --global --yes node@24 rust@1.96"
assert_contains "$default" "npm:@opencode-ai/cli@0.0.0-next-15323"
assert_not_contains "$default" "Corgea/Sighthound"

sighthound_log="$tmp/sighthound.log"
run_install "$sighthound_log" 1
sighthound=$(cat "$sighthound_log")
assert_contains "$sighthound" "rust@1.96"
assert_contains "$sighthound" "bin=sighthound,locked=true"
assert_contains "$sighthound" "rev:c4608eb2b6ca256daf4dbd1e74aadc3570343685"
assert_contains "$sighthound" "exec -- sighthound --version"

printf 'installer smoke passed\n'
