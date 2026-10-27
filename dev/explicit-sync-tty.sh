#!/usr/bin/env bash
# Run by a person logged into a free VT. Never switches VTs itself.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/dev/pacing/tty-lib.sh"
tty_refuse
[[ $# -gt 0 ]] || { echo "usage: $0 <native Wayland Vulkan client> [args...]" >&2; exit 2; }
binary="${SOLIUM_BINARY:-$root/target/debug/solium}"
[[ -x "$binary" ]] || { echo "build Solium first: $binary" >&2; exit 1; }
cap="${SYNC_CHECK_SECONDS:-120}"
[[ "$cap" =~ ^[0-9]+$ && "$cap" -ge 30 ]] || { echo 'SYNC_CHECK_SECONDS must be at least 30' >&2; exit 2; }
dir="$(mktemp -d /tmp/solium-explicit-sync-XXXXXX)"
pids="$dir/pids"
: > "$pids"
trap tty_stop EXIT
printf 'Evidence: %s\n' "$dir"
tty_meta "$dir/meta.txt"
printf 'client=' >> "$dir/meta.txt"
printf '%q ' "$@" >> "$dir/meta.txt"
printf '\n' >> "$dir/meta.txt"
# Isolated configuration and session services are off; the live desktop's
# environment is never imported or changed by this diagnostic session.
mkdir -p "$dir/config/solium"
cat > "$dir/config/solium/user.lua" <<'LUA'
return { session = { systemd = false, autostart = false } }
LUA
tty_start "$dir" "$cap" "$binary" RUST_LOG=solium=debug
WAYLAND_DISPLAY="$socket" WAYLAND_DEBUG=client timeout -k 5 -s TERM "$cap" "$@" >"$dir/client.stdout" 2>"$dir/client.log" &
client=$!
printf '%s\n' "$client" >> "$pids"
printf 'Exercise redraw, fullscreen and VT away/back. Quit Solium with Super+Shift+Q.\n'
set +e
wait -n -p finished "$solium" "$client"
first_result=$?
if [[ "$finished" == "$client" ]]; then
    client_result=$first_result
    wait "$solium"
    result=$?
else
    result=$first_result
    # Its exit after the compositor ends is expected and not a client test.
    client_result=running
fi
set -e
tty_stop
printf 'solium_exit=%s\nclient_exit=%s\n' "$result" "$client_result" >> "$dir/meta.txt"
if [[ "$client_result" != running && "$client_result" != 0 && "$client_result" != 124 ]]; then
    echo "FAIL: Vulkan client exited $client_result; inspect $dir/client.log" >&2; exit 1
fi
if grep -Eqi 'protocol error|error in client communication' "$dir/client.log"; then
    echo "FAIL: client reported a protocol error; inspect $dir/client.log" >&2; exit 1
fi
if ! grep -q 'advertising wp_linux_drm_syncobj_manager_v1' "$dir/stderr.log"; then
    echo "FAIL: explicit sync unavailable; inspect $dir/stderr.log" >&2; exit 1
fi
if ! grep -q 'wp_linux_drm_syncobj_surface_v1.*set_acquire_point' "$dir/client.log" ||
   ! grep -q 'wp_linux_drm_syncobj_surface_v1.*set_release_point' "$dir/client.log"; then
    echo "FAIL: the chosen client did not use syncobj acquire/release; inspect $dir/client.log" >&2; exit 1
fi
if [[ "$result" != 0 && "$result" != 124 ]]; then
    echo "FAIL: Solium exited $result; inspect $dir/stderr.log" >&2; exit 1
fi
printf 'Protocol use recorded. Review %s for errors and report visual/pacing observations.\n' "$dir"
printf 'This records protocol use and session survival; it does not certify NVIDIA, scanout or suspend/resume correctness.\n'
