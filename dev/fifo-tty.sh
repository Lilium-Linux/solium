#!/usr/bin/env bash
# Run by a person logged into a free VT. Never switches VTs itself.
#
# fifo and commit timing on real monitors (#76). Nested has no vblank, so
# there a refresh is a guess and a presentation time is when a frame was
# handed to the host; here both are the kernel's. wl-probe queues frames with
# fifo barriers and stamps one with a time, three times over, and vkcube, whose
# Mesa swapchain speaks both protocols, spins for ten seconds with every
# request and event written down. Nothing to watch: the cube should spin
# smoothly, and the numbers at the end say whether it did.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/dev/pacing/tty-lib.sh"
tty_refuse
binary="${SOLIUM_BINARY:-$root/target/debug/solium}"
probe="${WL_PROBE:-$root/target/debug/wl-probe}"
[[ -x "$binary" && -x "$probe" ]] || { echo "build Solium and wl-probe first: cargo build --workspace" >&2; exit 1; }
command -v vkcube >/dev/null || { echo "needs vkcube (vulkan-tools)" >&2; exit 1; }
dir="$(mktemp -d /tmp/solium-fifo-XXXXXX)"
pids="$dir/pids"
: > "$pids"
trap tty_stop EXIT
printf 'Evidence: %s\n' "$dir"
tty_meta "$dir/meta.txt"
mkdir -p "$dir/config/solium"
cat > "$dir/config/solium/user.lua" <<'LUA'
return { session = { systemd = false, autostart = false } }
LUA
tty_start "$dir" 120 "$binary"
sleep 3
for run in 1 2 3; do
    printf '== wl-probe %s\n' "$run" | tee -a "$dir/probe.log"
    WAYLAND_DISPLAY="$socket" timeout 60 "$probe" >>"$dir/probe.log" 2>&1 || true
done
WAYLAND_DEBUG=1 WAYLAND_DISPLAY="$socket" timeout -s INT 10 vkcube --wsi wayland \
    >"$dir/vkcube.out" 2>"$dir/vkcube.wire" || true
tty_stop
{
    grep -E '^(fifo|commit timing|wl-probe):|presentation clock' "$dir/probe.log"
    printf 'vkcube: %s barriers set, %s commits timed, %s frames presented, %s discarded\n' \
        "$(grep -c 'wp_fifo_v1#[0-9]*\.set_barrier' "$dir/vkcube.wire")" \
        "$(grep -c 'wp_commit_timer_v1#[0-9]*\.set_timestamp' "$dir/vkcube.wire")" \
        "$(grep -c 'wp_presentation_feedback#[0-9]*\.presented' "$dir/vkcube.wire")" \
        "$(grep -c 'wp_presentation_feedback#[0-9]*\.discarded' "$dir/vkcube.wire")"
    echo 'vkcube: milliseconds between presented frames, most common first'
    grep -oE 'wp_presentation_feedback#[0-9]+\.presented\([0-9]+, [0-9]+, [0-9]+' "$dir/vkcube.wire" \
        | awk -F', ' '{ t = $2 + $3 / 1e9; if (p) printf "%.1f\n", (t - p) * 1000; p = t }' \
        | sort -n | uniq -c | sort -rn | head -8
} | tee "$dir/summary.txt"
printf '\nDone. The summary is in %s/summary.txt.\n' "$dir"
printf 'Expected: every fifo line "presented: 12 discarded: 0"; every timed frame shown after its time, by\n'
printf 'less than a refresh; vkcube with barriers and timed commits, nothing discarded, and its frames one\n'
printf 'refresh of the monitor apart.\n'
