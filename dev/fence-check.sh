#!/usr/bin/env bash
#
# Does skipping a capture's CPU fence wait change a single pixel?
#
# Nested. dev/fence-check/scene.lua draws two kitty windows of fixed content
# in the rounded style, the first tilted, so both captures run on every frame:
# the client's (for its corners) and the warp's. Five frames from each of
# four runs:
#
#   on-1    the default: a capture waits on the CPU
#   on-2    the same again: the determinism baseline
#   off     SOLIUM_FENCE_WAIT=off
#   moved   the default, tilted 9 degrees instead of 8: the control that
#           proves the comparison sees a change
#
# on-2 and off must equal on-1 byte for byte; moved must not. That shows there
# is no systematic difference. A race that shows once in several hundred
# frames is wirecheck's to catch (cases 11c and 11d), not this.
#
#   SOLIUM_CHECK_DIR=/somewhere   keep the captures and the logs
set -uo pipefail

if [[ -z "${WAYLAND_DISPLAY:-}" ]]; then
    echo "refusing to start: WAYLAND_DISPLAY is empty or unset." >&2
    exit 1
fi
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
binary="$root/target/debug/solium"
here="$root/dev/fence-check"
[[ -x "$binary" ]] || { echo "not built: $binary  (see dev/README.md)" >&2; exit 1; }
command -v kitty >/dev/null || { echo "fence-check: needs kitty as a client" >&2; exit 1; }

out="${SOLIUM_CHECK_DIR:-$(mktemp -d -t solium-fence-XXXXXX)}"
mkdir -p "$out"
pids="$out/pids"
: >"$pids"
# Only what this script started is ever killed: a developer may have a
# nested Solium of their own running.
stop() {
    while read -r pid; do [[ -n "$pid" ]] && kill "$pid" 2>/dev/null; done <"$pids"
    sleep 0.4
    while read -r pid; do [[ -n "$pid" ]] && kill -9 "$pid" 2>/dev/null; done <"$pids"
    : >"$pids"
}
trap stop EXIT

FRAMES=5
TILT_AT=6400
CAPTURE_AT=7400

# $1 name, rest: extra environment.
shoot() {
    local name="$1"
    shift
    local dir="$out/$name"
    mkdir -p "$dir"
    rm -f "$dir"/f-*
    env "$@" SOLIUM_LUA_INIT="$here/scene.lua" SOLIUM_QML=software \
        SOLIUM_TRIGGER_AT="$TILT_AT:super+t" SOLIUM_CAPTURE="$dir/f" \
        SOLIUM_CAPTURE_AT="$CAPTURE_AT" SOLIUM_CAPTURE_FRAMES="$FRAMES" SOLIUM_CAPTURE_INTERVAL=100 \
        "$binary" >"$dir/log" 2>&1 &
    local solium=$!
    echo "$solium" >>"$pids"
    local socket=""
    for _ in $(seq 1 200); do
        kill -0 "$solium" 2>/dev/null || { echo "  $name: solium exited early"; tail -5 "$dir/log"; return 1; }
        socket="$(grep -oE 'socket=wayland-[0-9]+' "$dir/log" | tail -1 | cut -d= -f2)"
        [[ -n "$socket" ]] && break
        sleep 0.1
    done
    [[ -n "$socket" ]] || { echo "  $name: no socket"; return 1; }
    for colour in 3a6ea5 a5513a; do
        WAYLAND_DISPLAY="$socket" kitty --config NONE -o "background=#$colour" \
            -o cursor_blink_interval=0 -o confirm_os_window_close=0 -o font_size=14 \
            sh -c "printf '\033[?25lfence-check $colour\n'; sleep 600" >/dev/null 2>&1 &
        echo "$!" >>"$pids"
        sleep 1.8
    done
    for _ in $(seq 1 200); do
        [[ "$(ls "$dir"/f-* 2>/dev/null | wc -l)" -ge "$FRAMES" ]] && break
        sleep 0.1
    done
    stop
    [[ "$(ls "$dir"/f-* 2>/dev/null | wc -l)" -ge "$FRAMES" ]] || { echo "  $name: frames missing"; return 1; }
}

failures=0
shoot on-1 SOLIUM_FENCE_WAIT=on || failures=$((failures + 1))
shoot on-2 SOLIUM_FENCE_WAIT=on || failures=$((failures + 1))
shoot off SOLIUM_FENCE_WAIT=off || failures=$((failures + 1))
shoot moved SOLIUM_FENCE_WAIT=on FENCE_CHECK_ANGLE=9 || failures=$((failures + 1))

moved_differs=0
for frame in "$out"/on-1/f-*; do
    name="$(basename "$frame")"
    cmp -s "$frame" "$out/on-2/$name" || { echo "  FAIL: on-2/$name differs from on-1: not deterministic"; failures=$((failures + 1)); }
    cmp -s "$frame" "$out/off/$name" || { echo "  FAIL: off/$name differs from on-1"; failures=$((failures + 1)); }
    cmp -s "$frame" "$out/moved/$name" || moved_differs=1
done
[[ "$moved_differs" = 1 ]] || { echo "  FAIL: the 9-degree control equals the 8-degree run: the comparison sees nothing"; failures=$((failures + 1)); }
echo "fence-check: $failures failure(s); frames in $out"
exit "$((failures > 0))"
