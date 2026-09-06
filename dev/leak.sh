#!/usr/bin/env bash
#
# Open and close windows in cycles, and check the compositor gives it back.
#
#   dev/leak.sh 3 10        # 3 cycles of 10 windows
#
# A soak tells you memory grew; it cannot tell you whether that was a leak or
# a warm cache, because clients are still alive at the end. This settles it: a
# cycle ends with nothing running, so whatever the compositor is still holding
# it has no excuse for. Growth that persists across settled cycles, and grows
# with each one, is a leak; a single step up and then flat is a cache.
#
# File descriptors are watched as closely as memory. One forgotten per window
# is invisible for an afternoon and fatal after a week.
set -uo pipefail

cycles="${1:-3}"
per_cycle="${2:-10}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="${SOLIUM_LEAK_DIR:-/tmp/solium-leak}"
mkdir -p "$out"
log="$out/compositor.log"

[[ -n "${WAYLAND_DISPLAY:-}" ]] || { echo "WAYLAND_DISPLAY is empty" >&2; exit 1; }

# Deforms during the churn, because the warp path builds a texture per window
# per frame and that is the most expensive thing here to get wrong.
triggers=""
at=6000
while (( at < (cycles * per_cycle * 2500) + 60000 )); do
    triggers+="${at}:super+g,$(( at + 1200 )):super+m,"
    at=$(( at + 9000 ))
done

SOLIUM_TRIGGER_AT="${triggers%,}" nice -n 5 "$root/target/debug/solium" >"$log" 2>&1 &
comp=$!
trap 'kill "$comp" 2>/dev/null; pkill -x glxgears 2>/dev/null' EXIT

socket=""
for _ in $(seq 1 150); do
    kill -0 "$comp" 2>/dev/null || { echo "compositor exited early" >&2; tail -3 "$log" >&2; exit 1; }
    socket="$(grep -oE 'socket=wayland-[0-9]+' "$log" | tail -1 | cut -d= -f2)"
    [[ -n "$socket" ]] && break
    sleep 0.1
done
[[ -n "$socket" ]] || { echo "no socket" >&2; exit 1; }
x_display=""
for _ in $(seq 1 50); do
    n="$(grep -oE 'XWayland is up display=[0-9]+' "$log" | tail -1 | cut -d= -f2)"
    [[ -n "$n" ]] && { x_display=":$n"; break; }
    sleep 0.2
done

rss() { awk '/^VmRSS:/{print $2}' "/proc/$comp/status" 2>/dev/null; }
fds() { ls "/proc/$comp/fd" 2>/dev/null | wc -l; }
cpu() { awk '{print ($14+$15)/'"$(getconf CLK_TCK)"'}' "/proc/$comp/stat" 2>/dev/null; }
stalled() { [[ "$(cpu)" == "$1" ]]; }

sleep 6
base_rss="$(rss)"; base_fds="$(fds)"
echo "socket=$socket x11=${x_display:-none}"
printf 'baseline (nothing running): rss=%sMB fds=%s\n' "$(( base_rss / 1024 ))" "$base_fds"

for cycle in $(seq 1 "$cycles"); do
    before_cpu="$(cpu)"
    for _ in $(seq 1 "$per_cycle"); do
        env -u LD_LIBRARY_PATH -u DISPLAY WAYLAND_DISPLAY="$socket" kitty >/dev/null 2>&1 &
        client=$!
        sleep 1.4
        kill "$client" 2>/dev/null
        sleep 0.7
    done
    if [[ -n "$x_display" ]]; then
        ( env -u LD_LIBRARY_PATH DISPLAY="$x_display" WAYLAND_DISPLAY="$socket" \
            timeout 6 glxgears >/dev/null 2>&1 ) &
        sleep 8
    fi
    env -u LD_LIBRARY_PATH -u DISPLAY WAYLAND_DISPLAY="$socket" wl-copy "leak $cycle" >/dev/null 2>&1 &
    sleep 5   # settle: every client from this cycle is gone

    if stalled "$before_cpu"; then
        echo "cycle $cycle: STALLED -- the host stopped servicing us, numbers are meaningless"
        exit 3
    fi
    printf 'cycle %s settled: rss=%sMB (%+dMB vs baseline) fds=%s (%+d)\n' \
        "$cycle" "$(( $(rss) / 1024 ))" "$(( ($(rss) - base_rss) / 1024 ))" \
        "$(fds)" "$(( $(fds) - base_fds ))"
done
