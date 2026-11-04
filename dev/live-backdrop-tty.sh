#!/usr/bin/env bash
# Run by a person logged into a free VT. Never switches VTs itself.
#
# The live backdrop on real monitors (#209): the shipped desktop with the
# bar's glass reading what is beneath it (`source = "live"`), for you to drag
# windows and a video under. Nested cannot show three things this checks: the
# GPU path, an element scanned out on an overlay plane (none may be, on a
# monitor showing a live cut), and a monitor turned 90 degrees.
#
#   ROTATE=DP-2 dev/live-backdrop-tty.sh      that monitor at 90 for the run
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/dev/pacing/tty-lib.sh"
tty_refuse
binary="${SOLIUM_BINARY:-$root/target/debug/solium}"
[[ -x "$binary" ]] || { echo "build Solium first: $binary" >&2; exit 1; }
cap="${LIVE_CHECK_SECONDS:-180}"
dir="$(mktemp -d /tmp/solium-live-backdrop-XXXXXX)"
pids="$dir/pids"
: > "$pids"
trap tty_stop EXIT
printf 'Evidence: %s\n' "$dir"
tty_meta "$dir/meta.txt"
mkdir -p "$dir/config"
cp -a "$root/crates/solium/default-config" "$dir/config/solium"
sed -i 's/            effect = { "glass", soften_passes = 5,/            source = "live",\n            effect = { "glass", soften_passes = 5,/' "$dir/config/solium/looks.lua"
grep -q 'source = "live"' "$dir/config/solium/looks.lua" || { echo "could not set the bar to live" >&2; exit 1; }
if [[ -n "${ROTATE:-}" ]]; then
    sed -i "0,/^return {/s//return {\n    monitors = { { name = \"$ROTATE\", transform = \"90\" } },/" "$dir/config/solium/user.lua"
fi
sed -i "0,/^return {/s//return {\n    session = { systemd = false, autostart = false },/" "$dir/config/solium/user.lua"
tty_start "$dir" "$cap" "$binary" RUST_LOG=solium=debug
if tty_player; then
    WAYLAND_DISPLAY="$socket" "${player[@]}" >"$dir/player.log" 2>&1 &
    printf '%s\n' "$!" >> "$pids"
    printf 'Playing %s: drag it under the bar too.\n' "$playing"
fi
cat <<'TEXT'

Look for, on every monitor (and the turned one, if ROTATE was given):
  1. a window dragged under the bar shows through its glass, bent at its
     edges, and follows the drag with no lag or smear;
  2. the bar over the video (if one is playing) moves with the video;
  3. with nothing moving under it, nothing flickers;
  4. on the turned monitor, what shows through is the window under it, not
     a mirror or a rotated copy of it.
Quit Solium with Super+Shift+Q when done.
TEXT
set +e
wait "$solium"
result=$?
set -e
tty_stop
printf 'solium_exit=%s\n' "$result" >> "$dir/meta.txt"
grep -iE 'GL error|effects problem|panicked|refus' "$dir/stderr.log" > "$dir/problems.log" || true
printf '\nProblems the log names, if any: %s/problems.log (%s lines).\n' "$dir" "$(wc -l < "$dir/problems.log")"
printf 'Say what you saw for 1 to 4; the evidence directory holds the log.\n'
