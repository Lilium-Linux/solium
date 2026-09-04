#!/usr/bin/env bash
#
# Run Solium as a nested window on the host compositor.
#
#   dev/run-nested.sh              # just the compositor
#   dev/run-nested.sh foot         # and a terminal inside it
#
# Solium runs *in the build container*, not on the host. It is built there, and
# the container's C library is newer than the host's — a binary linking code
# built against 2.44 will not start on 2.43, which is exactly what happened the
# first time a vendored C dependency (Lua) was compiled in. Building and running
# in one place removes the skew instead of papering over it.
#
# Two things this script is deliberately careful about:
#
#   * It refuses to start without an explicit WAYLAND_DISPLAY. An empty value
#     is not "no display" — it resolves to the *default* socket, which on a
#     development machine is the developer's real session. That mistake has
#     already put a shell on the wrong screen once.
#   * The nested window carries the app_id `solium-nested`, so the host's window
#     rules can place it and stop it taking focus. See dev/host-window-rule.md.
set -uo pipefail

image="${SOLIUM_DEV_IMAGE:-localhost/lilium-base:v1}"

if [[ -z "${WAYLAND_DISPLAY:-}" ]]; then
    echo "refusing to start: WAYLAND_DISPLAY is empty or unset." >&2
    echo "an empty value resolves to the default socket, not to 'no display'." >&2
    exit 1
fi
if [[ -z "${XDG_RUNTIME_DIR:-}" ]]; then
    echo "refusing to start: XDG_RUNTIME_DIR is unset." >&2
    exit 1
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
binary="$root/target/debug/solium"
[[ -x "$binary" ]] || { echo "not built: $binary" >&2; exit 1; }

# Every SOLIUM_* knob the caller set, forwarded as-is. See dev/README.md.
declare -a passthrough=()
for name in $(compgen -v SOLIUM_ 2>/dev/null); do
    passthrough+=(-e "$name=${!name}")
done

# /tmp is shared so that SOLIUM_CAPTURE=/tmp/frame.ppm writes where the caller
# can read it; without it the capture lands in the container and disappears with
# it, having logged that it succeeded.
run_in_container() {
    podman run --rm --userns=keep-id --security-opt label=disable \
        --device /dev/dri --group-add keep-groups \
        -v "$HOME:$HOME" -v "$XDG_RUNTIME_DIR:$XDG_RUNTIME_DIR" \
        -v /tmp:/tmp \
        -e "XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR" \
        -e "RUST_LOG=${RUST_LOG:-info}" \
        ${passthrough[@]+"${passthrough[@]}"} \
        "$@"
}

log="$(mktemp -t solium-nested-XXXXXX.log)"
echo "host compositor: $WAYLAND_DISPLAY"
echo "log:             $log"

run_in_container -e "WAYLAND_DISPLAY=$WAYLAND_DISPLAY" -w "$root" \
    --name solium-nested "$image" ./target/debug/solium >"$log" 2>&1 &
solium=$!
trap 'podman rm -f solium-nested solium-nested-client >/dev/null 2>&1; kill "$solium" 2>/dev/null' EXIT

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

if [[ $# -gt 0 ]]; then
    echo "client:          $*"
    run_in_container -e "WAYLAND_DISPLAY=$socket" \
        --name solium-nested-client "$image" "$@" >/dev/null 2>&1 &
fi

wait "$solium"
