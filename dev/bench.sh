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

# Which socket a compositor ended up on is found by watching for one to
# appear, rather than by parsing its log: Hyprland does not print it at all,
# and every compositor spells it differently. A new socket in the runtime
# directory is unambiguous and needs no per-compositor knowledge.
sockets_now() { ls "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}" 2>/dev/null | grep -E '^wayland-[0-9]+$' | sort; }
x_sockets_now() { ls /tmp/.X11-unix 2>/dev/null | sort; }
before_sockets="$(sockets_now)"
before_x="$(x_sockets_now)"

case "$which" in
    solium)
        nice -n 5 "$root/target/debug/solium" >"$log" 2>&1 &
        comp=$!
        ;;
    sway)
        sway_root="${SWAY_ROOT:?set SWAY_ROOT to the extracted sway tree}"
        LD_LIBRARY_PATH="$sway_root/usr/lib64" WLR_BACKENDS=wayland \
            nice -n 5 "$sway_root/usr/bin/sway" --unsupported-gpu -c "${SWAY_CONFIG:?}" >"$log" 2>&1 &
        comp=$!
        ;;
    hyprland)
        hypr_root="${HYPR_ROOT:?set HYPR_ROOT to the extracted hyprland tree}"
        LD_LIBRARY_PATH="$hypr_root/usr/lib64" \
            nice -n 5 "$hypr_root/usr/bin/Hyprland" -c "${HYPR_CONFIG:?}" >"$log" 2>&1 &
        comp=$!
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

# Asking who listens, rather than which name is new: a compositor killed with
# a signal leaves its socket file behind, the next one happily reuses the same
# name, and a name diff then reports that nothing started.
socket_of() {
    ss -xlp 2>/dev/null | awk -v pid="$1" '
        $0 ~ ("pid=" pid ",") {
            for (i = 1; i <= NF; i++)
                if ($i ~ /wayland-[0-9]+$/) { n = split($i, parts, "/"); print parts[n]; exit }
        }'
}
socket=""
for _ in $(seq 1 300); do
    kill -0 "$comp" 2>/dev/null || { echo "$which exited early" >&2; tail -3 "$log" >&2; exit 1; }
    socket="$(socket_of "$comp")"
    [[ -z "$socket" ]] && socket="$(comm -13 <(echo "$before_sockets") <(sockets_now) | head -1)"
    [[ -n "$socket" ]] && break
    sleep 0.1
done
[[ -n "$socket" ]] || { echo "$which never opened a socket" >&2; exit 1; }
# Never the host's own socket. Detection that falls back to the host puts the
# benchmark's terminals on the developer's real desktop, which is both wrong
# and hard to notice afterwards.
if [[ "$socket" == "$WAYLAND_DISPLAY" ]]; then
    echo "$which: refusing to run -- detection landed on the host socket ($socket)" >&2
    exit 1
fi
echo "  $which on $socket"

# Both compositors start XWayland lazily, so the display number is read the
# same way from each: by asking, once a client needs it.
# Same trick for XWayland: a new socket under /tmp/.X11-unix is the display,
# whoever started it and however they logged it.
x_display=""
for _ in $(seq 1 100); do
    fresh="$(comm -13 <(echo "$before_x") <(x_sockets_now) | head -1)"
    [[ -n "$fresh" ]] && { x_display=":${fresh#X}"; break; }
    sleep 0.2
done
[[ -n "$x_display" ]] || echo "  ($which: no XWayland appeared; the X11 client will be skipped)" >&2

for _ in 1 2; do
    env -u LD_LIBRARY_PATH -u DISPLAY WAYLAND_DISPLAY="$socket" kitty >/dev/null 2>&1 &
    clients+=($!)
    sleep 1
done
if [[ -n "$x_display" ]]; then
    env -u LD_LIBRARY_PATH DISPLAY="$x_display" WAYLAND_DISPLAY="$socket" glxgears >"$gears" 2>&1 &
    clients+=($!)
fi
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
