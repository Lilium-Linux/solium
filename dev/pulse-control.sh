#!/usr/bin/env bash
#
# The known-animating control a nested capture is judged beside (spec §8.1,
# "Judging captures"): one terminal of fixed content, focused, in the `pulse`
# pane style, which animates while its window is focused. Two captures 150 ms
# apart must differ. If they do not, the renderer is frozen, and no capture
# taken beside this one proves anything.
#
#   dev/pulse-control.sh [extra environment for the compositor]
#
# Nested. Exit 0 when the two frames differ; 1 when they do not, or the run failed.
set -uo pipefail

[[ -n "${WAYLAND_DISPLAY:-}" ]] || { echo "refusing to start: WAYLAND_DISPLAY is empty or unset." >&2; exit 1; }
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
binary="${PULSE_BINARY:-$root/target/debug/solium}"
[[ -x "$binary" ]] || { echo "not built: $binary" >&2; exit 1; }
command -v kitty >/dev/null || { echo "pulse-control: needs kitty" >&2; exit 1; }
out="$(mktemp -d -t solium-pulse-XXXXXX)"
mkdir -p "$out/config"
# Nothing but the style: no wallpaper, no shell scene, nothing else that moves.
printf 'sol.pane("pulse")\n' >"$out/init.lua"
pids="$out/pids"
: >"$pids"
# Only what this script started is ever killed.
stop() {
    while read -r pid; do [[ -n "$pid" ]] && kill "$pid" 2>/dev/null; done <"$pids"
    sleep 0.5
    while read -r pid; do [[ -n "$pid" ]] && kill -9 "$pid" 2>/dev/null; done <"$pids"
    : >"$pids"
}
trap stop EXIT

# The extra environment comes last, so it can override any of these.
env SOLIUM_QML=software SOLIUM_LUA_INIT="$out/init.lua" XDG_CONFIG_HOME="$out/config" \
    SOLIUM_CAPTURE="$out/f" SOLIUM_CAPTURE_AT=6000 SOLIUM_CAPTURE_FRAMES=2 SOLIUM_CAPTURE_INTERVAL=150 "$@" \
    timeout -k 5 -s TERM 30 "$binary" >"$out/log" 2>&1 &
solium=$!
echo "$solium" >>"$pids"
socket=""
for _ in $(seq 1 200); do
    kill -0 "$solium" 2>/dev/null || { echo "pulse-control: solium exited early; see $out/log"; exit 1; }
    socket="$(grep -oE 'socket=wayland-[0-9]+' "$out/log" | tail -1 | cut -d= -f2)"
    [[ -n "$socket" ]] && break
    sleep 0.1
done
[[ -n "$socket" ]] || { echo "pulse-control: no socket; see $out/log"; exit 1; }
WAYLAND_DISPLAY="$socket" kitty --config NONE -o cursor_blink_interval=0 -o confirm_os_window_close=0 \
    sh -c "printf '\033[?25lpulse control\n'; exec sleep 600" >/dev/null 2>&1 &
echo "$!" >>"$pids"
for _ in $(seq 1 150); do
    [[ "$(ls "$out"/f-* 2>/dev/null | wc -l)" -ge 2 ]] && break
    sleep 0.1
done
stop
frames=("$out"/f-*)
[[ ${#frames[@]} -ge 2 && -f "${frames[0]}" && -f "${frames[1]}" ]] || { echo "pulse-control: frames missing; see $out/log"; exit 1; }
if cmp -s "${frames[0]}" "${frames[1]}"; then
    echo "pulse-control: FAIL: the pulse frame did not move in 150 ms, so the renderer is frozen; frames in $out"
    exit 1
fi
echo "pulse-control: the pulse frame moved; frames in $out"
