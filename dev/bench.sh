#!/usr/bin/env bash
#
# Measure a nested compositor under a fixed client load.
#
#   dev/bench.sh solium 120
#   dev/bench.sh sway 120
#
# The load is identical for both: two terminals and an X11 client through
# XWayland. What is measured is what the compositor costs to composite it --
# CPU seconds per wall second and resident memory -- plus what the client
# achieves through it, since a compositor can look cheap by simply presenting
# less.
#
# Run each one twice, alternating, before believing the difference: the host
# session this nests inside is not a controlled environment.
set -uo pipefail

which="${1:?solium or sway}"
seconds="${2:-120}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="${SOLIUM_BENCH_DIR:-/tmp/solium-bench}"
mkdir -p "$out"
log="$out/$which.log"
gears="$out/$which.gears.log"

[[ -n "${WAYLAND_DISPLAY:-}" ]] || { echo "WAYLAND_DISPLAY is empty" >&2; exit 1; }

case "$which" in
    solium)
        nice -n 5 "$root/target/debug/solium" >"$log" 2>&1 &
        comp=$!
        pattern='socket=wayland-[0-9]+'
        ;;
    sway)
        sway_root="${SWAY_ROOT:?set SWAY_ROOT to the extracted sway tree}"
        LD_LIBRARY_PATH="$sway_root/usr/lib64" WLR_BACKENDS=wayland \
            nice -n 5 "$sway_root/usr/bin/sway" --unsupported-gpu -c "${SWAY_CONFIG:?}" >"$log" 2>&1 &
        comp=$!
        pattern='WAYLAND_DISPLAY=wayland-[0-9]+'
        ;;
    *) echo "unknown compositor: $which" >&2; exit 2 ;;
esac

clients=()
cleanup() {
    for pid in "${clients[@]:-}"; do [[ -n "$pid" ]] && kill "$pid" 2>/dev/null; done
    kill "$comp" 2>/dev/null
    pkill -x glxgears 2>/dev/null
    wait "$comp" 2>/dev/null
}
trap cleanup EXIT

socket=""
for _ in $(seq 1 200); do
    kill -0 "$comp" 2>/dev/null || { echo "$which exited early" >&2; tail -3 "$log" >&2; exit 1; }
    socket="$(grep -oE "$pattern" "$log" | tail -1 | cut -d= -f2)"
    [[ -n "$socket" ]] && break
    sleep 0.1
done
[[ -n "$socket" ]] || { echo "$which never reported a socket" >&2; exit 1; }

# Both compositors start XWayland lazily, so the display number is read the
# same way from each: by asking, once a client needs it.
x_display=":$(( 20 + RANDOM % 20 ))"
for _ in $(seq 1 60); do
    case "$which" in
        solium) n="$(grep -oE 'XWayland is up display=[0-9]+' "$log" | tail -1 | cut -d= -f2)";;
        sway)   n="$(grep -oE 'xwayland.*DISPLAY=:[0-9]+' "$log" | grep -oE ':[0-9]+' | tail -1 | tr -d ':')";;
    esac
    [[ -n "${n:-}" ]] && { x_display=":$n"; break; }
    sleep 0.2
done

for _ in 1 2; do
    env -u LD_LIBRARY_PATH -u DISPLAY WAYLAND_DISPLAY="$socket" kitty >/dev/null 2>&1 &
    clients+=($!)
    sleep 1
done
env -u LD_LIBRARY_PATH DISPLAY="$x_display" WAYLAND_DISPLAY="$socket" glxgears >"$gears" 2>&1 &
clients+=($!)
sleep 6

read_cpu() { awk '{print ($14+$15)/'"$(getconf CLK_TCK)"'}' "/proc/$comp/stat" 2>/dev/null; }
read_rss() { awk '/^VmRSS:/{print $2}' "/proc/$comp/status" 2>/dev/null; }

cpu_start="$(read_cpu)"; rss_start="$(read_rss)"
sleep "$seconds"
cpu_end="$(read_cpu)"; rss_end="$(read_rss)"

fps="$(grep -oE '= [0-9.]+ FPS' "$gears" | grep -oE '[0-9.]+' | awk '{n++; s+=$1} END {if (n) printf "%.0f", s/n; else print "n/a"}')"
cpu="$(echo "${cpu_end:-0} - ${cpu_start:-0}" | bc)"
printf '%-8s cpu=%ss over %ss (%.1f%% of a core)  rss=%sMB->%sMB  client_fps=%s\n' \
    "$which" "$cpu" "$seconds" \
    "$(echo "scale=4; $cpu / $seconds * 100" | bc)" \
    "$(( ${rss_start:-0} / 1024 ))" "$(( ${rss_end:-0} / 1024 ))" "$fps"
