#!/usr/bin/env bash
#
# Does dev/install.sh install, check and uninstall cleanly?
#
# A full install into a DESTDIR under /tmp, with --session-dir under /tmp too,
# then every file it put there, the sudo lines it printed (run without sudo,
# since they point into /tmp), `--check` from the staged copy, and an uninstall
# that leaves nothing behind. Everything it writes is under one directory of
# its own in /tmp, removed when every check passes and kept when one fails.
#
#   dev/install-check.sh [--no-build]
#
# --no-build is handed to install.sh, to install the release binary already in
# target/install instead of building it first.
set -uo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
install_sh="$root/dev/install.sh"
build_args=()
case "${1:-}" in
    "") ;;
    --no-build) build_args=(--no-build) ;;
    *) echo "usage: dev/install-check.sh [--no-build]" >&2; exit 2 ;;
esac

work="$(mktemp -d /tmp/solium-install-check.XXXXXX)"
destdir="$work/destdir"
sessions="$work/sessions"
config="$work/config"
mkdir -p "$config"
# The default prefix, so this checks what a bare `dev/install.sh` does.
prefix="$HOME/.local"
dest="$destdir$prefix"
share="$dest/share/solium"
staged="$share/solium.desktop"

failures=0
pass() { echo "  ok    $*"; }
fail() { echo "  FAIL  $*"; failures=$((failures + 1)); }
check() {
    local what="$1"
    shift
    if "$@"; then pass "$what"; else fail "$what"; fi
}
count_files() { find "$@" -type f | wc -l; }

echo "work directory: $work"

echo "refusals"
DESTDIR="$work/refused" "$install_sh" --no-build --prefix "/tmp/with space" \
    >"$work/refused-space.log" 2>&1
check "a prefix with a space is refused" [ $? -ne 0 ]
check "  and says why" grep -q "may only contain" "$work/refused-space.log"
DESTDIR="$work/refused" "$install_sh" --no-build --prefix relative/path \
    >"$work/refused-relative.log" 2>&1
check "a relative prefix is refused" [ $? -ne 0 ]
check "  and nothing was written" [ ! -e "$work/refused" ]

echo "install"
# What an earlier install of an older checkout would leave: a file this
# checkout no longer has. Reinstalling has to take it away, or it goes on being
# found.
mkdir -p "$share/qml"
touch "$share/qml/removed-since.qml"
DESTDIR="$destdir" "$install_sh" "${build_args[@]}" --session-dir "$sessions" \
    >"$work/install.log" 2>&1
status=$?
check "dev/install.sh exits 0" [ "$status" -eq 0 ]
if [[ "$status" -ne 0 ]]; then
    tail -20 "$work/install.log"
    echo "install-check: FAILED, install did not complete; kept $work"
    exit 1
fi

echo "files"
check "bin/solium is executable" [ -x "$dest/bin/solium" ]
check "bin/solium is the release build" cmp -s "$dest/bin/solium" "$root/target/install/release/solium"
check "share/solium/qml is a copy of crates/solium/qml" diff -r "$root/crates/solium/qml" "$share/qml"
check "share/solium/lua is a copy of crates/solium/lua" diff -r "$root/crates/solium/lua" "$share/lua"
check "a file left by an earlier install is gone" [ ! -e "$share/qml/removed-since.qml" ]
check "nothing staged is a symlink" [ -z "$(find "$destdir" -type l)" ]
expected_files=$(($(count_files "$root/crates/solium/qml" "$root/crates/solium/lua") + 2))
check "exactly those files and the session file ($expected_files)" \
    [ "$(count_files "$destdir")" -eq "$expected_files" ]
check "the session file is mode 644" [ "$(stat -c %a "$staged")" = 644 ]
check "Exec is absolute: Exec=$prefix/bin/solium --tty" \
    grep -qx "Exec=$prefix/bin/solium --tty" "$staged"
check "  and is the only line that differs from dev/session/solium.desktop" \
    diff <(grep -v '^Exec=' "$root/dev/session/solium.desktop") <(grep -v '^Exec=' "$staged")
check "install.sh wrote nothing into --session-dir itself" [ ! -e "$sessions" ]

echo "the printed sudo line"
mapfile -t lines < <(grep -E '^  sudo install -Dm644 ' "$work/install.log")
check "exactly one sudo line is printed" [ "${#lines[@]}" -eq 1 ]
check "  and it is install -Dm644 <staged> <session-dir>/solium.desktop" \
    [ "${lines[0]:-}" = "  sudo install -Dm644 $staged $sessions/solium.desktop" ]
read -ra command <<<"${lines[0]#  sudo }"
"${command[@]}"
check "run without sudo, it installs the session file" cmp -s "$staged" "$sessions/solium.desktop"
check "  mode 644" [ "$(stat -c %a "$sessions/solium.desktop")" = 644 ]

echo "--check from the staged copy"
XDG_CONFIG_HOME="$config" RUST_LOG="info,solium::assets=debug" "$dest/bin/solium" --check \
    >"$work/check.out" 2>&1
check "exits 0" [ $? -eq 0 ]
check "reads the staged init.lua" grep -qx "checking $share/lua/init.lua" "$work/check.out"
check "resolves its assets to its own share/solium" \
    grep -q "shipped assets root=$share\$" "$work/check.out"
check "loads the bindings" grep -q '^  ok: [0-9]* binding(s)' "$work/check.out"
check "install.sh ran the same check and said so" grep -q -- "--check passed" "$work/install.log"
check "install.sh names the session log" grep -q "solium/session.log" "$work/install.log"
check "install.sh names the uninstall command" \
    grep -q "DESTDIR=$destdir dev/install.sh --uninstall --session-dir $sessions" "$work/install.log"

echo "refusing to touch a running Solium"
# A stand-in that runs from the path a binary would be installed at: a copy of
# bash, which stays the process while its loop runs.
busy="$work/busy"
fake="$busy$prefix/bin/solium"
mkdir -p "$(dirname "$fake")"
cp /usr/bin/bash "$fake"
"$fake" -c 'while :; do sleep 0.2; done' &
pid=$!
sleep 0.3
DESTDIR="$busy" "$install_sh" --no-build --session-dir "$sessions" >"$work/busy-install.log" 2>&1
check "install refuses" [ $? -ne 0 ]
check "  naming the pid" grep -q "pid $pid" "$work/busy-install.log"
DESTDIR="$busy" "$install_sh" --uninstall --session-dir "$sessions" >"$work/busy-uninstall.log" 2>&1
check "uninstall refuses" [ $? -ne 0 ]
check "the running binary was left alone" cmp -s /usr/bin/bash "$fake"
check "nothing was installed beside it" [ ! -e "$busy$prefix/share" ]
check "and the process is still running" kill -0 "$pid"
kill "$pid" 2>/dev/null
wait "$pid" 2>/dev/null

echo "uninstall"
DESTDIR="$destdir" "$install_sh" --uninstall --session-dir "$sessions" >"$work/uninstall.log" 2>&1
check "exits 0" [ $? -eq 0 ]
check "no file is left under DESTDIR" [ "$(count_files "$destdir")" -eq 0 ]
check "share/solium is gone" [ ! -e "$share" ]
mapfile -t lines < <(grep -E '^  sudo ' "$work/uninstall.log")
check "exactly one sudo line: sudo rm -f <session-dir>/solium.desktop" \
    [ "${#lines[@]}:${lines[0]:-}" = "1:  sudo rm -f $sessions/solium.desktop" ]
read -ra command <<<"${lines[0]#  sudo }"
"${command[@]}"
check "run without sudo, it removes the session file" [ ! -e "$sessions/solium.desktop" ]
DESTDIR="$destdir" "$install_sh" --uninstall --session-dir "$sessions" >"$work/uninstall-again.log" 2>&1
check "a second uninstall exits 0" [ $? -eq 0 ]
check "  says there is nothing installed" grep -q "nothing installed" "$work/uninstall-again.log"
check "  and prints no sudo line" [ -z "$(grep -E '^  sudo ' "$work/uninstall-again.log")" ]

if [[ "$failures" -ne 0 ]]; then
    echo "install-check: FAILED, $failures check(s); kept $work"
    exit 1
fi
rm -rf "$work"
echo "install-check: passed"
