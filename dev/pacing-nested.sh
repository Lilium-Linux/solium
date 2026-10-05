#!/usr/bin/env bash
#
# The TTY scenes, nested: dev/pacing-tty.sh's twin, for proving the
# instrumentation end to end before a TTY run is asked for, and for a
# smoke-level before and after. Its numbers are not S1's: QML is software
# nested (#148), the deadline is the host monitor's, there are no flips, and
# only the captures are GPU-timed (dev/README.md says why).
#
#   dev/pacing-nested.sh <label> [seconds=20]
#
#   PACING_SCENE=s1|tilt     the scene, s1 by default; PACING_TILT=<degrees>
#                            holds S1's player tilted, as on the TTY
#   PACING_BINARY=<path>     a frozen build instead of target/debug/solium
#   PACING_EXPECT_CAPTURES   5 for s1 and 2 for tilt by default
#
# Extra environment passes through: SOLIUM_FENCE_WAIT=off dev/pacing-nested.sh off.
# Writes $XDG_STATE_HOME/solium/pacing/nested-<date>-<label>/ (or under
# ~/.local/state), and prints the summary.
set -uo pipefail

label="${1:?a label, such as dry-run}"
seconds="${2:-20}"
scene="${PACING_SCENE:-s1}"
case "$scene" in
    s1) expect="${PACING_EXPECT_CAPTURES:-5}"; style=rounded ;;
    tilt) expect="${PACING_EXPECT_CAPTURES:-2}"; style=none ;;
    *) echo "PACING_SCENE is s1 or tilt" >&2; exit 2 ;;
esac
[[ -n "${WAYLAND_DISPLAY:-}" ]] || { echo "refusing to start: WAYLAND_DISPLAY is empty or unset." >&2; exit 1; }
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
binary="${PACING_BINARY:-$root/target/debug/solium}"
[[ "$binary" = /* ]] || binary="$root/$binary"
[[ -x "$binary" ]] || { echo "not built: $binary" >&2; exit 1; }
command -v kitty >/dev/null || { echo "needs kitty" >&2; exit 1; }
command -v ffplay >/dev/null || { echo "needs ffplay" >&2; exit 1; }
out="${XDG_STATE_HOME:-$HOME/.local/state}/solium/pacing/nested-$(date +%Y%m%d-%H%M%S)-$label"
mkdir -p "$out/config"
pids="$out/pids"
: >"$pids"
stop() {
    while read -r pid; do [[ -n "$pid" ]] && kill "$pid" 2>/dev/null; done <"$pids"
    sleep 1
    while read -r pid; do [[ -n "$pid" ]] && kill -9 "$pid" 2>/dev/null; done <"$pids"
    : >"$pids"
}
trap stop EXIT
trace="${XDG_RUNTIME_DIR:-/tmp}/solium-pacing-nested-$$.jsonl"
SOLIUM_TRACE="$trace" SOLIUM_QML=software SOLIUM_PANE="$style" SOLIUM_LUA_INIT="$root/dev/pacing/$scene.lua" \
    XDG_CONFIG_HOME="$out/config" timeout -k 5 -s TERM $((seconds + 40)) "$binary" >"$out/stderr.log" 2>&1 &
solium=$!
echo "$solium" >>"$pids"
socket=""
for _ in $(seq 1 300); do
    kill -0 "$solium" 2>/dev/null || { echo "solium exited early; see $out/stderr.log" >&2; exit 1; }
    socket="$(grep -oE 'socket=wayland-[0-9]+' "$out/stderr.log" | tail -1 | cut -d= -f2)"
    [[ -n "$socket" ]] && break
    sleep 0.1
done
[[ -n "$socket" ]] || { echo "no socket" >&2; exit 1; }
if [[ "$scene" = tilt ]]; then
    env -u DISPLAY WAYLAND_DISPLAY="$socket" kitty --config NONE -o cursor_blink_interval=0 \
        -o confirm_os_window_close=0 sh -c "printf '\033[?25ltilt: idle\n'; exec sleep 3600" >/dev/null 2>&1 &
    echo "$!" >>"$pids"
    sleep 1.5
    env -u DISPLAY WAYLAND_DISPLAY="$socket" kitty --config NONE -o cursor_blink_interval=0 \
        -o confirm_os_window_close=0 \
        sh -c 'printf "\033[?25l"; while true; do printf "\rtilt: %s" "$(date +%T.%N | cut -c1-10)"; sleep 0.1; done' \
        >/dev/null 2>&1 &
    echo "$!" >>"$pids"
else
    for index in 1 2 3 4; do
        env -u DISPLAY WAYLAND_DISPLAY="$socket" kitty --config NONE -o cursor_blink_interval=0 \
            -o confirm_os_window_close=0 sh -c "printf '\033[?25lS1 idle window $index\n'; exec sleep 3600" >/dev/null 2>&1 &
        echo "$!" >>"$pids"
        sleep 1.5
    done
    env -u DISPLAY WAYLAND_DISPLAY="$socket" SDL_VIDEODRIVER=wayland ffplay -loglevel error -an -loop 0 \
        -f lavfi -i testsrc2=size=1280x720:rate=60 >"$out/player.log" 2>&1 &
    echo "$!" >>"$pids"
fi
sleep 5
from="$(python3 -c 'import time; print(time.monotonic_ns())')"
sleep "$seconds"
to="$(python3 -c 'import time; print(time.monotonic_ns())')"
stop
mv "$trace" "$out/trace.jsonl" 2>/dev/null || true
python3 "$root/dev/pacing-summary.py" "$out/trace.jsonl" --from "$from" --to "$to" \
    --expect-captures "$expect" --totals "$out/stderr.log" | tee "$out/summary.txt"
status=${PIPESTATUS[0]}
echo "results: $out"
exit "$status"
