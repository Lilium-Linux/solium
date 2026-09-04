#!/usr/bin/env bash
#
# Run Solium as a nested window on the host compositor, optionally with a
# client inside it.
#
#   dev/run-nested.sh              # just the compositor
#   dev/run-nested.sh foot         # and a terminal inside it
#
# Two things this script is deliberately careful about:
#
#   * It refuses to start without an explicit WAYLAND_DISPLAY. An empty value
#     is not "no display" -- it resolves to the *default* socket, which on a
#     development machine is the developer's real session.
#   * The nested window carries the app_id `solium-nested`, so the host's window
#     rules can place it and stop it taking focus. See dev/host-window-rule.md.
set -uo pipefail

if [[ -z "${WAYLAND_DISPLAY:-}" ]]; then
    echo "refusing to start: WAYLAND_DISPLAY is empty or unset." >&2
    echo "an empty value resolves to the default socket, not to 'no display'." >&2
    exit 1
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
binary="$root/target/debug/solium"
[[ -x "$binary" ]] || { echo "not built: $binary" >&2; exit 1; }

log="$(mktemp -t solium-nested-XXXXXX.log)"
echo "host compositor: $WAYLAND_DISPLAY"
echo "log:             $log"

"$binary" >"$log" 2>&1 &
solium=$!
trap 'kill "$solium" 2>/dev/null' EXIT

# The socket is picked at runtime, so it is read back rather than assumed.
socket=""
for _ in $(seq 1 100); do
    kill -0 "$solium" 2>/dev/null || { echo "solium exited early:" >&2; tail -5 "$log" >&2; exit 1; }
    socket="$(grep -oE 'socket=wayland-[0-9]+' "$log" | tail -1 | cut -d= -f2)"
    [[ -n "$socket" ]] && break
    sleep 0.1
done
[[ -n "$socket" ]] || { echo "solium never reported a socket" >&2; exit 1; }

echo "solium socket:   $socket"

if [[ $# -gt 0 ]]; then
    echo "client:          $*"
    WAYLAND_DISPLAY="$socket" "$@" >/dev/null 2>&1 &
fi

wait "$solium"
