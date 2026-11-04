#!/usr/bin/env bash
# Run by a person logged into a free VT. Never switches VTs itself.
#
# wlr-output-management on real monitors (#51): the first monitor is given a
# new scale, a quarter turn, another of its modes and variable refresh, each
# through the protocol as a program would ask, and put back as it was. Nested
# cannot show a rotation, a mode or vrr; this is where they are proven. Watch
# the monitor at each step: the script says what it should be doing.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/dev/pacing/tty-lib.sh"
tty_refuse
binary="${SOLIUM_BINARY:-$root/target/debug/solium}"
probe="${WL_PROBE:-$root/target/debug/wl-probe}"
[[ -x "$binary" && -x "$probe" ]] || { echo "build Solium and wl-probe first: cargo build --workspace" >&2; exit 1; }
hold="${STEP_SECONDS:-5}"
dir="$(mktemp -d /tmp/solium-output-management-XXXXXX)"
pids="$dir/pids"
: > "$pids"
trap tty_stop EXIT
printf 'Evidence: %s\n' "$dir"
tty_meta "$dir/meta.txt"
mkdir -p "$dir/config/solium"
cat > "$dir/config/solium/user.lua" <<'LUA'
return { session = { systemd = false, autostart = false } }
LUA
tty_start "$dir" 120 "$binary" RUST_LOG=solium=debug
sleep 3
ask() {
    local label="$1"
    shift
    printf '== %s\n' "$label" | tee -a "$dir/steps.log"
    WAYLAND_DISPLAY="$socket" env "$@" "$probe" 2>&1 | tee -a "$dir/steps.log"
    command -v grim >/dev/null && WAYLAND_DISPLAY="$socket" grim "$dir/${label// /-}.png" 2>/dev/null || true
    sleep "$hold"
}
ask "before" WL_PROBE_OUTPUTS=1
first="$(grep -m1 -P '^\S+\ton\t' "$dir/steps.log" | cut -f1)"
current="$(grep -m1 -P '^\S+\ton\t' "$dir/steps.log" | cut -f3)"
scale="$(grep -m1 -P '^\S+\ton\t' "$dir/steps.log" | cut -f5 | cut -d' ' -f2)"
other="$(grep -m1 '^modes' "$dir/steps.log" | cut -f2 | tr ' ' '\n' | grep -v -x "$current" | grep -m1 -E '^(1920x1080|1280x720|2560x1440)@' || true)"
printf 'first monitor %s, driven at %s, scale %s; another mode: %s\n' "$first" "$current" "$scale" "${other:-none}" | tee -a "$dir/steps.log"
printf '\n[watch] everything grows a quarter larger\n'
ask "scale 1.25" WL_PROBE_ARRANGE="scale=1.25"
printf '\n[watch] the monitor goes dark for a moment and comes back turned a quarter\n'
ask "transform 90" WL_PROBE_ARRANGE="transform=90 scale=$scale"
ask "transform normal" WL_PROBE_ARRANGE="transform=normal"
if [[ -n "$other" ]]; then
    printf '\n[watch] the monitor goes dark for a moment and comes back at %s\n' "$other"
    ask "mode $other" WL_PROBE_ARRANGE="mode=$other"
    ask "mode back" WL_PROBE_ARRANGE="mode=$current"
fi
printf '\n[watch] vrr on, then off; the monitor may blank for each\n'
ask "vrr on" WL_PROBE_ARRANGE="vrr=on"
ask "vrr off" WL_PROBE_ARRANGE="vrr=off"
ask "test with nothing" WL_PROBE_ARRANGE_TEST=1 WL_PROBE_ARRANGE="scale=0.1"
ask "after" WL_PROBE_OUTPUTS=1
tty_stop
grep -iE 'lit again|monitor arrived|monitor gone|no free CRTC|could not|panicked' "$dir/stderr.log" > "$dir/monitor-lines.log" || true
printf '\nDone. Each answer is in %s/steps.log, and what the compositor said of each monitor in %s/monitor-lines.log.\n' "$dir" "$dir"
printf 'Expected: every apply "succeeded", the test "failed", each step read back as asked, and the last read the same as the first.\n'
