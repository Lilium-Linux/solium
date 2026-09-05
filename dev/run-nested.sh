#!/usr/bin/env bash
#
# Run Solium as a nested window on the host compositor.
#
#   dev/run-nested.sh                       # just the compositor
#   dev/run-nested.sh konsole --separate    # and a client inside it
#
# Solium runs on the host, on the host's GPU driver. It is built in a container
# (dev/Containerfile) whose distribution matches the host's, and that match is
# not incidental — see dev/README.md. Running it inside a container instead
# costs hardware acceleration and drops the compositor to ~55 fps.
#
# Two things this script is deliberately careful about:
#
#   * It refuses to start without an explicit WAYLAND_DISPLAY. An empty value
#     is not "no display" — it resolves to the *default* socket, which on a
#     development machine is the developer's real session. That mistake has
#     already put a shell on the wrong screen once.
#   * The nested window carries the app_id `solium-nested`, so the host's window
#     rules can place it, keep it from taking focus, and pass keyboard shortcuts
#     through to it. See dev/host-window-rule.md.
set -uo pipefail

if [[ -z "${WAYLAND_DISPLAY:-}" ]]; then
    echo "refusing to start: WAYLAND_DISPLAY is empty or unset." >&2
    echo "an empty value resolves to the default socket, not to 'no display'." >&2
    exit 1
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
binary="$root/target/debug/solium"
[[ -x "$binary" ]] || { echo "not built: $binary  (see dev/README.md)" >&2; exit 1; }

log="$(mktemp -t solium-nested-XXXXXX.log)"
echo "host compositor: $WAYLAND_DISPLAY"
echo "log:             $log"

"$binary" >"$log" 2>&1 &
solium=$!
trap 'kill "$solium" 2>/dev/null' EXIT

# The socket is chosen at runtime, so it is read back rather than assumed.
socket=""
for _ in $(seq 1 150); do
    kill -0 "$solium" 2>/dev/null || { echo "solium exited early:" >&2; tail -5 "$log" >&2; exit 1; }
    socket="$(grep -oE 'socket=wayland-[0-9]+' "$log" | tail -1 | cut -d= -f2)"
    [[ -n "$socket" ]] && break
    sleep 0.1
done
[[ -n "$socket" ]] || { echo "solium never reported a socket" >&2; tail -5 "$log" >&2; exit 1; }

echo "solium socket:   $socket"
echo
echo "run anything inside it with:  WAYLAND_DISPLAY=$socket <program>"

if [[ $# -gt 0 ]]; then
    WAYLAND_DISPLAY="$socket" "$@" >/dev/null 2>&1 &
fi

wait "$solium"
