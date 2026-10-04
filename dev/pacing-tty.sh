#!/usr/bin/env bash
#
# Frame pacing on the hardware: Phase 0's "before" and "after" numbers (FX-S1).
#
#   dev/pacing-tty.sh <protocol> <monitor>
#
#   before   fourteen runs, about 21 minutes:
#              s1-a s1-b      the S1 scene twice: its numbers, and their noise
#              s1-untraced    S1 with neither SOLIUM_TRACE nor SOLIUM_PACING:
#                             the exit totals alone, to see what measuring costs
#              s1-noclocks    S1 with the clock sampler off
#              fence-on-a fence-off-a fence-on-b fence-off-b
#                             S1 with a capture's CPU wait on, off, on, off
#              tfence-on-a tfence-off-a tfence-on-b tfence-off-b
#                             the same on S1 tilted (the player held at 6 degrees)
#              tilt-a tilt-b  dev/pacing/tilt.lua: two tilted windows, one idle
#   after    nine runs, about 14 minutes: anchor (S1 on the build
#            PACING_ANCHOR names, run 1's, in this sitting), s1-a, s1-b, the
#            four tfence runs, tilt-a and tilt-b
#   fence    tfence-on-a and tfence-off-a, about 3 minutes: the fence check
#            on a later build
#   one      one s1 run, to try it out, about 2 minutes
#
# <monitor> is the connector the windows go on, as `solium --probe` names it.
#
# Run it on a free virtual terminal, logged in, from the top of the worktree
# the measured binary was built in, and keep your hands off the keyboard and
# the mouse until it says "done". Every run is capped by a timeout.
# Ctrl+Alt+Backspace stops the compositor at any moment, and the script then
# stops and says where the logs of what ran are. Do not switch VT during a
# run: that takes input away from the session being measured.
#
# What it writes, under $XDG_STATE_HOME/solium/pacing/ (or
# ~/.local/state/solium/pacing/), in <date>-<protocol>/: a directory per run
# with meta.txt, stderr.log, the compositor's own state/solium/session.log,
# trace.jsonl and summary.txt, and results.txt with every run's summary.
#
#   PACING_BINARY=<path>   run this binary instead of target/debug/solium:
#                          a build frozen for a measurement
#   PACING_ANCHOR=<path>   `after` only, and required there: the build run 1
#                          measured, run once more as this sitting's anchor
#   PACING_CLIP=<video>    what mpv plays; without mpv, or without a clip,
#                          ffplay plays a 1280x720 test pattern at 60 fps
#   PACING_WARMUP, PACING_SECONDS   10 and 60 by default
#   PACING_EXPECT_CAPTURES=<n>      the captures a pass must mostly have, in
#                          every run, instead of each run's own (below)
set -uo pipefail

protocol="${1:-}"
monitor="${2:-}"
case "$protocol" in
    before) runs=(s1-a s1-b s1-untraced s1-noclocks fence-on-a fence-off-a fence-on-b fence-off-b
                  tfence-on-a tfence-off-a tfence-on-b tfence-off-b tilt-a tilt-b) ;;
    after) runs=(anchor s1-a s1-b tfence-on-a tfence-off-a tfence-on-b tfence-off-b tilt-a tilt-b) ;;
    fence) runs=(tfence-on-a tfence-off-a) ;;
    one) runs=(s1-a) ;;
    *) echo "usage: dev/pacing-tty.sh before|after|fence|one <monitor>" >&2; exit 2 ;;
esac
[[ -n "$monitor" ]] || { echo "name the monitor, as solium --probe does: dev/pacing-tty.sh $protocol DP-1" >&2; exit 2; }

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=dev/pacing/tty-lib.sh
source "$root/dev/pacing/tty-lib.sh"
absolute() { if [[ "$1" = /* ]]; then echo "$1"; else echo "$root/$1"; fi; }
binary="$(absolute "${PACING_BINARY:-target/debug/solium}")"
[[ -x "$binary" ]] || { echo "not built: $binary" >&2; exit 1; }
anchor=""
if [[ "$protocol" = after ]]; then
    [[ -n "${PACING_ANCHOR:-}" ]] || { echo "after needs PACING_ANCHOR=<run 1's frozen binary>" >&2; exit 2; }
    anchor="$(absolute "$PACING_ANCHOR")"
    [[ -x "$anchor" ]] || { echo "not built: $anchor" >&2; exit 1; }
fi
tty_refuse
command -v kitty >/dev/null || { echo "needs kitty" >&2; exit 1; }
command -v python3 >/dev/null || { echo "needs python3" >&2; exit 1; }
tty_player || exit 1

warm="${PACING_WARMUP:-10}"
seconds="${PACING_SECONDS:-60}"
cap=$((warm + seconds + 60))
out="${XDG_STATE_HOME:-$HOME/.local/state}/solium/pacing/$(date +%Y%m%d-%H%M%S)-$protocol"
mkdir -p "$out"
pids="$out/pids"
: >"$pids"
trap 'tty_stop; echo "stopped; logs so far: $out" >&2; exit 130' INT TERM
trap tty_stop EXIT

# One run. $1 the label, $2 the scene (s1 or tilt), $3 the captures a pass
# must mostly have (`-` for an untraced run, which has no trace to
# summarise), $4 the binary, the rest extra environment. Returns 1 when the
# run was stopped, which stops the protocol.
run() {
    local label="$1" scene="$2" want="$3" use="$4"
    shift 4
    local dir="$out/$label"
    mkdir -p "$dir"
    local trace="$XDG_RUNTIME_DIR/solium-pacing-$$-$label.jsonl"
    local measure=(SOLIUM_TRACE="$trace")
    [[ "$want" = - ]] && measure=()
    local style=rounded
    [[ "$scene" = tilt ]] && style=none
    {
        echo "label=$label protocol=$protocol scene=$scene monitor=$monitor environment=${*:-none}"
        echo "binary=$use sha256=$(sha256sum "$use" | cut -c1-16) rev=$(cat "$use.rev" 2>/dev/null || git -C "$root" rev-parse --short HEAD)"
        echo "player=$playing"
    } >"$dir/meta.txt"
    tty_meta "$dir/meta.txt"
    tty_start "$dir" "$cap" "$use" "$@" "${measure[@]}" SOLIUM_PANE="$style" \
        SOLIUM_LUA_INIT="$root/dev/pacing/$scene.lua" PACING_MONITOR="$monitor" || return 1
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
                -o confirm_os_window_close=0 sh -c "printf '\033[?25lS1 idle window $index\n'; exec sleep 3600" \
                >/dev/null 2>&1 &
            echo "$!" >>"$pids"
            sleep 1.5
        done
        env -u DISPLAY WAYLAND_DISPLAY="$socket" "${player[@]}" >"$dir/player.log" 2>&1 &
        echo "$!" >>"$pids"
    fi
    sleep "$warm"
    local from to
    from="$(python3 -c 'import time; print(time.monotonic_ns())')"
    for _ in $(seq 1 "$seconds"); do
        kill -0 "$solium" 2>/dev/null || { echo "$label: solium stopped mid-run (Ctrl+Alt+Backspace?)" >&2; return 1; }
        sleep 1
    done
    to="$(python3 -c 'import time; print(time.monotonic_ns())')"
    tty_stop
    grep -E 'PACING|pacing:' "$dir/stderr.log" >"$dir/pacing.log" || true
    if [[ "$want" = - ]]; then
        # Untraced: the exit totals are the whole result.
        grep -E 'pacing:.*passes=' "$dir/stderr.log" >"$dir/summary.txt" || echo "no totals line" >"$dir/summary.txt"
        echo "summary exit=0" >>"$dir/summary.txt"
    else
        mv "$trace" "$dir/trace.jsonl" 2>/dev/null || true
        python3 "$root/dev/pacing-summary.py" "$dir/trace.jsonl" --from "$from" --to "$to" \
            --expect-captures "${PACING_EXPECT_CAPTURES:-$want}" --totals "$dir/stderr.log" >"$dir/summary.txt" 2>&1
        echo "summary exit=$?" >>"$dir/summary.txt"
    fi
    { echo "== $label"; cat "$dir/meta.txt" "$dir/summary.txt"; echo; } >>"$out/results.txt"
    return 0
}

# What a pass of each scene mostly captures. Before captures are kept and
# rounding is drawn inline (run 1's build): S1, its five rounded windows; S1
# tilted, its four rounded windows and the player's warp (a deformed window
# takes no client pass); tilt, both warps. After (run 2's build, and any
# later one the `fence` protocol runs): S1 nothing; S1 tilted, the player's
# warp; tilt, the ticking window's warp. Task 10's nested dry run checks the
# first three numbers on the frozen build, Task 23b's the last three.
case "$protocol" in
    after | fence) s1_want=0; tilted_want=1; tilt_want=1 ;;
    *) s1_want=5; tilted_want=5; tilt_want=2 ;;
esac
for label in "${runs[@]}"; do
    case "$label" in
        anchor) run "$label" s1 5 "$anchor" ;;
        s1-untraced) run "$label" s1 - "$binary" ;;
        s1-noclocks) run "$label" s1 "$s1_want" "$binary" SOLIUM_PACING_CLOCKS=off ;;
        fence-on-*) run "$label" s1 "$s1_want" "$binary" SOLIUM_FENCE_WAIT=on ;;
        fence-off-*) run "$label" s1 "$s1_want" "$binary" SOLIUM_FENCE_WAIT=off ;;
        tfence-on-*) run "$label" s1 "$tilted_want" "$binary" PACING_TILT=6 SOLIUM_FENCE_WAIT=on ;;
        tfence-off-*) run "$label" s1 "$tilted_want" "$binary" PACING_TILT=6 SOLIUM_FENCE_WAIT=off ;;
        tilt-*) run "$label" tilt "$tilt_want" "$binary" ;;
        *) run "$label" s1 "$s1_want" "$binary" ;;
    esac || { echo "stopped at $label; logs: $out" >&2; exit 1; }
    sleep 3
done
echo "done: $out/results.txt"
