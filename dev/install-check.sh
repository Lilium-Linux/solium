#!/usr/bin/env bash
#
# Does dev/install.sh install, check and uninstall cleanly?
#
# A full install into a DESTDIR under /tmp, with --session-dir under /tmp too,
# then every file it put there, the sudo lines it printed (run without sudo,
# since they point into /tmp), `--check` from the staged copy, and an uninstall
# that leaves nothing behind. Before and around that, the refusals: paths it
# cannot print safely, a link it would delete through, a Solium running from
# the binary (one started during the build included), and a check that fails
# after the files are in place. Everything it writes is under one directory of
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
DESTDIR="$work/refused-root" "$install_sh" --no-build --prefix / --session-dir "$sessions" \
    >"$work/refused-root.log" 2>&1
check "--prefix / is refused" [ $? -ne 0 ]
check "  and nothing was written" [ ! -e "$work/refused-root" ]
DESTDIR="$work/refused-root" "$install_sh" --no-build --session-dir // \
    >"$work/refused-root-sessions.log" 2>&1
check "--session-dir // is refused" [ $? -ne 0 ]
check "  and nothing was written" [ ! -e "$work/refused-root" ]
DESTDIR="$work/with space" "$install_sh" --no-build --session-dir "$sessions" \
    >"$work/refused-destdir.log" 2>&1
check "a DESTDIR with a space is refused" [ $? -ne 0 ]
check "  and says why" grep -q "may only contain" "$work/refused-destdir.log"
check "  and nothing was written" [ ! -e "$work/with space" ]

echo "refusing to delete through a link"
# Install replaces share/solium/{qml,lua} and uninstall removes them. Through a
# link, that deletes whatever the link points at: a share/solium left linked to
# a checkout would lose its QML and Lua, uncommitted work included. Each case
# plants files where the link leads, and both install and uninstall have to
# refuse and leave them there.
wip() {
    mkdir -p "$1/qml" "$1/lua"
    echo wip >"$1/qml/Mine.qml"
    echo wip >"$1/lua/my-wip.lua"
}
refuses_through_link() {
    local name="$1" what="$2" destdir="$3" kept="$4" says="$5" script="$6" mode status
    for mode in install uninstall; do
        wip "$kept"
        if [[ "$mode" == install ]]; then
            DESTDIR="$destdir" "$script" --no-build --session-dir "$sessions" \
                >"$work/$name-$mode.log" 2>&1
        else
            DESTDIR="$destdir" "$script" --uninstall --session-dir "$sessions" \
                >"$work/$name-$mode.log" 2>&1
        fi
        status=$?
        check "$mode refuses $what" [ "$status" -ne 0 ]
        check "  saying \"$says\"" grep -q -- "$says" "$work/$name-$mode.log"
        check "  and what the link leads to keeps its files" \
            [ -f "$kept/qml/Mine.qml" -a -f "$kept/lua/my-wip.lua" ]
    done
    check "  and nothing was installed" [ ! -e "$destdir$prefix/bin" ]
}
# share/solium itself a link, as linking a checkout into ~/.local leaves it.
mkdir -p "$work/link-a$prefix/share"
ln -s "$work/link-a-target" "$work/link-a$prefix/share/solium"
refuses_through_link link-a "a linked share/solium" "$work/link-a" \
    "$work/link-a-target" "is a symlink" "$install_sh"
# share a link, as a ~/.local/share moved to another disk leaves it: an
# ordinary setup, so install goes through it, and uninstall removes only what
# install put there. Another application's data beside it survives both.
mkdir -p "$work/link-b$prefix" "$work/link-b-target/other-app"
echo keep >"$work/link-b-target/other-app/data"
ln -s "$work/link-b-target" "$work/link-b$prefix/share"
DESTDIR="$work/link-b" "$install_sh" --no-build --session-dir "$sessions" >"$work/link-b-install.log" 2>&1
check "install goes through a linked share" [ $? -eq 0 ]
check "  and lands where the link leads" [ -d "$work/link-b-target/solium/qml" -a -d "$work/link-b-target/solium/lua" ]
DESTDIR="$work/link-b" "$install_sh" --uninstall --session-dir "$sessions" >"$work/link-b-uninstall.log" 2>&1
check "uninstall goes through a linked share" [ $? -eq 0 ]
check "  and removes what install put there" [ ! -e "$work/link-b-target/solium/qml" -a ! -e "$work/link-b-target/solium/lua" ]
check "  and another application's data survives" grep -qx keep "$work/link-b-target/other-app/data"
check "  and the link itself is left alone" [ -L "$work/link-b$prefix/share" ]
# A staged session file that is a link: writing it must replace the link, never
# write through it into what it points at.
mkdir -p "$work/link-d$prefix/share/solium"
echo planted >"$work/link-d-planted.desktop"
ln -s "$work/link-d-planted.desktop" "$work/link-d$prefix/share/solium/solium.desktop"
DESTDIR="$work/link-d" "$install_sh" --no-build --session-dir "$sessions" >"$work/link-d-install.log" 2>&1
check "a linked staged session file is replaced" [ $? -eq 0 -a ! -L "$work/link-d$prefix/share/solium/solium.desktop" ]
check "  and what it pointed at is untouched" grep -qx planted "$work/link-d-planted.desktop"
check "  and the new one has the absolute Exec" grep -qx "Exec=$prefix/bin/solium --tty" "$work/link-d$prefix/share/solium/solium.desktop"
DESTDIR="$work/link-d" "$install_sh" --uninstall --session-dir "$sessions" >/dev/null 2>&1
# The prefix a link into the checkout, so no link sits where the two cases
# above look, and share/solium still resolves into the checkout. A stand-in
# checkout under $work, since the real one is what a failure would delete.
fake_root="$work/checkout"
mkdir -p "$fake_root/dev/session" "$fake_root/target/install/release" "$fake_root/inside"
cp "$install_sh" "$fake_root/dev/install.sh"
cp "$root/dev/session/solium.desktop" "$fake_root/dev/session/"
printf '#!/bin/sh\nexit 1\n' >"$fake_root/target/install/release/solium"
chmod +x "$fake_root/target/install/release/solium"
mkdir -p "$(dirname "$work/link-c$prefix")"
ln -s "$fake_root/inside" "$work/link-c$prefix"
refuses_through_link link-c "a prefix that resolves into the checkout" "$work/link-c" \
    "$fake_root/inside/share/solium" "inside this checkout" "$fake_root/dev/install.sh"

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
expected="  sudo install -Dm644 $staged $sessions/solium.desktop"
check "exactly one sudo line is printed" [ "${#lines[@]}" -eq 1 ]
check "  and it is install -Dm644 <staged> <session-dir>/solium.desktop" \
    [ "${lines[0]:-}" = "$expected" ]
# Only the line just checked is run: any other could name any path.
if [[ "${lines[0]:-}" == "$expected" ]]; then
    read -ra command <<<"${lines[0]#  sudo }"
    "${command[@]}"
fi
check "run without sudo, it installs the session file" cmp -s "$staged" "$sessions/solium.desktop"
check "  mode 644" [ "$(stat -c %a "$sessions/solium.desktop")" = 644 ]

echo "--check from the staged copy"
XDG_CONFIG_HOME="$config" RUST_LOG="info,solium::assets=debug" "$dest/bin/solium" --check \
    >"$work/check.out" 2>&1
check "exits 0" [ $? -eq 0 ]
# Resolved, as install.sh compares it: the binary finds its prefix through
# /proc/self/exe, so where $HOME or /tmp is reached through a link, the root it
# logs is the resolved one.
real_share="$(realpath "$share")"
check "reads the staged init.lua" grep -qx "checking $real_share/lua/init.lua" "$work/check.out"
check "resolves its assets to its own share/solium" \
    grep -q "shipped assets root=$real_share\$" "$work/check.out"
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

echo "refusing a Solium started during the build"
# A session can start from the old install while the build runs. A stand-in
# podman plays that: its `run`, the build, starts a process from the path the
# binary is about to be installed at.
mid="$work/mid-build"
mid_bin="$mid$prefix/bin/solium"
mkdir -p "$(dirname "$mid_bin")" "$work/fakebin"
cp /usr/bin/bash "$mid_bin"
cat >"$work/fakebin/podman" <<EOF
#!/bin/sh
case "\$1" in
    image) exit 0 ;;
    run)
        "$mid_bin" -c 'while :; do sleep 0.2; done' </dev/null >/dev/null 2>&1 &
        echo \$! >"$work/mid-build.pid"
        sleep 0.3
        exit 0 ;;
esac
exit 1
EOF
chmod +x "$work/fakebin/podman"
PATH="$work/fakebin:$PATH" SOLIUM_BUILD_LOCK="$work/build.lock" DESTDIR="$mid" \
    "$install_sh" --session-dir "$sessions" >"$work/mid-build.log" 2>&1
check "install refuses" [ $? -ne 0 ]
mid_pid="$(cat "$work/mid-build.pid" 2>/dev/null || true)"
check "  naming the pid" grep -q "pid ${mid_pid:-none}" "$work/mid-build.log"
check "  the running binary was left alone" cmp -s /usr/bin/bash "$mid_bin"
check "  and nothing was installed beside it" [ ! -e "$mid$prefix/share" ]
[[ -n "$mid_pid" ]] && kill "$mid_pid" 2>/dev/null

echo "a check that fails after installing"
# The files are in place by the time the check runs, and a session file from
# an earlier install would start them at the next login, so the failure has
# to say so. A configuration that raises an error fails the check.
broken="$work/broken-config"
mkdir -p "$broken/solium"
echo 'error("planted by dev/install-check.sh")' >"$broken/solium/init.lua"
XDG_CONFIG_HOME="$broken" DESTDIR="$work/failed" "$install_sh" --no-build \
    --session-dir "$sessions" >"$work/failed.log" 2>&1
check "install exits non-zero" [ $? -ne 0 ]
check "  saying the new files are already in place" grep -q "already in place" "$work/failed.log"
check "  and how to take them out" \
    grep -q "DESTDIR=$work/failed dev/install.sh --uninstall --session-dir $sessions" "$work/failed.log"
DESTDIR="$work/failed" "$install_sh" --uninstall --session-dir "$sessions" \
    >"$work/failed-uninstall.log" 2>&1
check "  which removes them" [ "$(count_files "$work/failed")" -eq 0 ]

echo "uninstall"
DESTDIR="$destdir" "$install_sh" --uninstall --session-dir "$sessions" >"$work/uninstall.log" 2>&1
check "exits 0" [ $? -eq 0 ]
check "no file is left under DESTDIR" [ "$(count_files "$destdir")" -eq 0 ]
check "share/solium is gone" [ ! -e "$share" ]
mapfile -t lines < <(grep -E '^  sudo ' "$work/uninstall.log")
expected="  sudo rm -f $sessions/solium.desktop"
check "exactly one sudo line: sudo rm -f <session-dir>/solium.desktop" \
    [ "${#lines[@]}:${lines[0]:-}" = "1:$expected" ]
if [[ "${#lines[@]}:${lines[0]:-}" == "1:$expected" ]]; then
    read -ra command <<<"${lines[0]#  sudo }"
    "${command[@]}"
fi
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
