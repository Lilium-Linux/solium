#!/usr/bin/env bash
#
# Run Solium nested under continuous scripted use, and watch what it keeps.
#
#   dev/soak.sh 48        # minutes
#
# A compositor leaks quietly. Nothing crashes, nothing logs, and the session
# is simply worse after a day than it was after a minute -- so the only honest
# test is churn plus time plus a number sampled throughout.
#
# What churns, on a 20-second cycle: a terminal opens, the clipboard makes a
# round trip, a window is tilted and then sucked into a slot (which is the
# expensive path -- it captures the window to a texture every frame), the mode
# changes, the workspace changes, and the window closes again. An X11 client
# joins in periodically, since XWayland is a second surface lifecycle with its
# own bookkeeping.
#
# What is sampled every 20 seconds: resident memory, thread count, open file
# descriptors, and total CPU. Descriptors matter as much as memory here: a
# compositor that forgets one per window survives about a day.
set -uo pipefail

minutes="${1:-48}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
binary="$root/target/debug/solium"
out="${SOLIUM_SOAK_DIR:-/tmp/solium-soak}"
mkdir -p "$out"
csv="$out/samples.csv"
log="$out/compositor.log"

[[ -x "$binary" ]] || { echo "not built: $binary" >&2; exit 1; }
if [[ -z "${WAYLAND_DISPLAY:-}" ]]; then
    echo "refusing to start: WAYLAND_DISPLAY is empty or unset." >&2
    exit 1
fi

# The scripted half of the workload, generated up front: triggers are relative
# to start-up, so an hour of use is an hour of entries.
cycle=(super+return super+g super+m super+space super+ctrl+right super+q)
triggers=""
at=4000
while (( at < minutes * 60000 )); do
    for key in "${cycle[@]}"; do
        triggers+="${at}:${key},"
        at=$(( at + 3300 ))
    done
done
triggers="${triggers%,}"

SOLIUM_TRIGGER_AT="$triggers" nice -n 5 "$binary" >"$log" 2>&1 &
solium=$!

clients=()
cleanup() {
    for pid in "${clients[@]:-}"; do [[ -n "$pid" ]] && kill "$pid" 2>/dev/null; done
    kill "$solium" 2>/dev/null
    pkill -x glxgears 2>/dev/null
    wait "$solium" 2>/dev/null
}
trap cleanup EXIT

socket=""
for _ in $(seq 1 150); do
    kill -0 "$solium" 2>/dev/null || { echo "solium exited early" >&2; tail -5 "$log" >&2; exit 1; }
    socket="$(grep -oE 'socket=wayland-[0-9]+' "$log" | tail -1 | cut -d= -f2)"
    [[ -n "$socket" ]] && break
    sleep 0.1
done
[[ -n "$socket" ]] || { echo "no socket" >&2; exit 1; }

x_display=""
for _ in $(seq 1 60); do
    number="$(grep -oE 'XWayland is up display=[0-9]+' "$log" | tail -1 | cut -d= -f2)"
    [[ -n "$number" ]] && { x_display=":$number"; break; }
    sleep 0.1
done

echo "soak: ${minutes} minutes, socket=$socket, x11=${x_display:-none}"
echo "samples: $csv"

kwin="$(pgrep -x kwin_wayland | head -1)"
echo "elapsed_s,rss_kb,threads,fds,cpu_s,clients,kwin_rss_kb,kwin_cpu_s" >"$csv"

sample() {
    local pid=$1
    [[ -r /proc/$pid/status ]] || { echo ",,,"; return; }
    local rss threads fds cpu
    rss=$(awk '/^VmRSS:/{print $2}' "/proc/$pid/status")
    threads=$(awk '/^Threads:/{print $2}' "/proc/$pid/status")
    fds=$(ls "/proc/$pid/fd" 2>/dev/null | wc -l)
    cpu=$(awk '{print ($14+$15)/'"$(getconf CLK_TCK)"'}' "/proc/$pid/stat")
    echo "$rss,$threads,$fds,$cpu"
}

start=$(date +%s)
deadline=$(( start + minutes * 60 ))
tick=0
while (( $(date +%s) < deadline )); do
    kill -0 "$solium" 2>/dev/null || { echo "COMPOSITOR DIED at $(( $(date +%s) - start ))s" | tee -a "$csv"; break; }

    # Client churn, on its own slower rhythm than the keyboard scripting.
    if (( tick % 6 == 0 )); then
        env -u LD_LIBRARY_PATH -u DISPLAY WAYLAND_DISPLAY="$socket" kitty >/dev/null 2>&1 &
        clients+=($!)
    fi
    if (( tick % 9 == 4 )) && [[ -n "$x_display" ]]; then
        # An X11 client, in bursts: the point is the surface lifecycle, and
        # glxgears left running is just heat.
        ( env -u LD_LIBRARY_PATH DISPLAY="$x_display" WAYLAND_DISPLAY="$socket" \
            timeout 25 glxgears >/dev/null 2>&1 ) &
        clients+=($!)
    fi
    if (( tick % 3 == 1 )); then
        env -u LD_LIBRARY_PATH -u DISPLAY WAYLAND_DISPLAY="$socket" \
            wl-copy "soak $tick" >/dev/null 2>&1 &
        sleep 0.4
        got="$(env -u LD_LIBRARY_PATH -u DISPLAY WAYLAND_DISPLAY="$socket" timeout 4 wl-paste 2>/dev/null)"
        [[ "$got" == "soak $tick" ]] || echo "clipboard round trip failed at tick $tick" >>"$out/failures.log"
    fi
    # Keep at most a handful alive, oldest first.
    while (( ${#clients[@]} > 4 )); do
        kill "${clients[0]}" 2>/dev/null
        clients=("${clients[@]:1}")
    done

    solium_row="$(sample "$solium")"
    # A stall is not idleness. Nested, the compositor blocks inside
    # eglSwapBuffers waiting on the host, and if the host stops servicing the
    # window -- a screen lock, a blank, an occluded surface -- it waits
    # forever, burning no CPU at all. Sampling that quietly produces a
    # beautiful flat memory graph of a compositor that is not running.
    cpu_now="$(echo "$solium_row" | cut -d, -f4)"
    if [[ "$cpu_now" == "${cpu_prev:-}" ]]; then
        stalls=$(( ${stalls:-0} + 1 ))
        (( stalls >= 2 )) && echo "STALLED: no CPU consumed by $(( $(date +%s) - start ))s" >>"$out/failures.log"
    else
        stalls=0
    fi
    cpu_prev="$cpu_now"
    kwin_row=",,,"
    [[ -n "$kwin" ]] && kwin_row="$(sample "$kwin")"
    kwin_rss="$(echo "$kwin_row" | cut -d, -f1)"
    kwin_cpu="$(echo "$kwin_row" | cut -d, -f4)"
    echo "$(( $(date +%s) - start )),$solium_row,${#clients[@]},$kwin_rss,$kwin_cpu" >>"$csv"

    tick=$(( tick + 1 ))
    sleep 20
done

echo "soak finished at $(( $(date +%s) - start ))s"
