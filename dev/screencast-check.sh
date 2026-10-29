#!/usr/bin/env bash
# Screen sharing as an application gets it: xdg-desktop-portal-wlr streaming a
# nested Solium into PipeWire, and a consumer counting the frames that arrive
# and how many of them differ (#387). Run with an explicit host
# WAYLAND_DISPLAY, after building.
#
# Everything it starts is its own: a D-Bus session bus (it runs itself under
# dbus-run-session), a PipeWire daemon on a socket in a fresh directory, the
# compositor, the portal backend and the consumer. Nothing reaches the user's
# bus, PipeWire or portal.
set -euo pipefail
: "${WAYLAND_DISPLAY:?explicit host Wayland socket required}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ -z "${SOLIUM_SCREENCAST_PRIVATE:-}" ]]; then
    exec env SOLIUM_SCREENCAST_PRIVATE=1 dbus-run-session -- "$0" "$@"
fi
binary="$root/target/debug/solium"
portal="${SOLIUM_XDPW:-/usr/libexec/xdg-desktop-portal-wlr}"
[[ -x "$binary" ]] || { echo 'build solium first' >&2; exit 1; }
for tool in "$portal" pipewire pw-link gdbus gst-launch-1.0; do
    command -v "$tool" >/dev/null || { echo "skipped: $tool is not installed"; exit 0; }
done
gst-inspect-1.0 pipewiresrc >/dev/null 2>&1 \
    || { echo "skipped: GStreamer's pipewiresrc is not installed"; exit 0; }
out="${SOLIUM_CHECK_DIR:-$(mktemp -d -t solium-screencast-XXXXXX)}"
mkdir -p "$out/config" "$out/state" "$out/frames"
# A socket path has to fit in 108 bytes, which a check directory need not.
export PIPEWIRE_RUNTIME_DIR="$(mktemp -d /tmp/solium-pw-XXXXXX)"
unset PIPEWIRE_REMOTE
pids=()
cleanup() {
    for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done
    for pid in "${pids[@]}"; do wait "$pid" 2>/dev/null || true; done
    rm -rf "$PIPEWIRE_RUNTIME_DIR"
}
trap cleanup EXIT

pipewire > "$out/pipewire.log" 2>&1 & pids+=($!)
for _ in $(seq 1 50); do [[ -S "$PIPEWIRE_RUNTIME_DIR/pipewire-0" ]] && break; sleep 0.1; done
[[ -S "$PIPEWIRE_RUNTIME_DIR/pipewire-0" ]] || { echo "no private PipeWire"; exit 1; }

cat > "$out/init.lua" <<LUA
sol.session{systemd=false,autostart=false}
sol.surface("fixture", {scene="$root/dev/screencast-check/moving.qml", layer="background"})
LUA
env -u WAYLAND_SOCKET -u SOLIUM_SESSION_BUS -u SOLIUM_LOGIND_BUS \
    -u SOLIUM_CAPTURE_FRAMES -u SOLIUM_TRIGGER_AT -u SOLIUM_KEY_AT -u SOLIUM_DRAG_AT \
    SOLIUM_QML=software SOLIUM_LUA_INIT="$out/init.lua" \
    XDG_CONFIG_HOME="$out/config" XDG_STATE_HOME="$out/state" \
    RUST_LOG=solium=info,smithay=warn \
    timeout -k 5s -s TERM 60s "$binary" > "$out/compositor.log" 2>&1 & pids+=($!)
compositor=$!
socket=""
for _ in $(seq 1 150); do
    kill -0 "$compositor" 2>/dev/null || { echo "compositor exited early"; exit 1; }
    socket="$(sed -n 's/.*socket=\(wayland-[0-9]*\).*/\1/p' "$out/compositor.log" | tail -1)"
    [[ -n "$socket" ]] && break
    sleep 0.1
done
[[ -n "$socket" && "$socket" != "$WAYLAND_DISPLAY" && -S "$XDG_RUNTIME_DIR/$socket" ]] \
    || { echo "no distinct nested socket"; exit 1; }
sleep 3

WAYLAND_DISPLAY="$socket" timeout 50s "$portal" -l DEBUG -r \
    -c "$root/dev/screencast-check/xdpw.conf" > "$out/xdpw.log" 2>&1 & pids+=($!)
name=org.freedesktop.impl.portal.desktop.wlr
for _ in $(seq 1 50); do
    gdbus call --session -d org.freedesktop.DBus -o /org/freedesktop/DBus \
        -m org.freedesktop.DBus.NameHasOwner "$name" 2>/dev/null | grep -q true && break
    sleep 0.1
done
# The portal frontend's three calls, made straight to the backend.
call() {
    gdbus call --session -d "$name" -o /org/freedesktop/portal/desktop \
        -m "org.freedesktop.impl.portal.ScreenCast.$1" "${@:2}"
}
request=/org/freedesktop/portal/desktop/request/1_1
session=/org/freedesktop/portal/desktop/session/1_1/check
call CreateSession "$request/r1" "$session" check '{}' > "$out/portal.log"
call SelectSources "$request/r2" "$session" check \
    "{'types': <uint32 1>, 'cursor_mode': <uint32 2>}" >> "$out/portal.log"
call Start "$request/r3" "$session" check '' '{}' >> "$out/portal.log"
node="$(sed -n "s/.*'streams': <\[(uint32 \([0-9]*\).*/\1/p" "$out/portal.log" | head -1)"
[[ -n "$node" ]] || { echo "the portal started no stream"; cat "$out/portal.log"; exit 1; }
echo "backend: $(grep -o 'using [a-z_]*' "$out/xdpw.log" | head -1)"

seconds="${SOLIUM_CAST_SECONDS:-8}"
# Kept small: a few hundred whole frames of a monitor is most of a gigabyte,
# and /tmp is often memory. Small is enough to see the square move.
timeout "$((seconds + 2))s" gst-launch-1.0 -q pipewiresrc path="$node" target-object="$node" \
    always-copy=true ! videoconvert ! videoscale \
    ! video/x-raw,format=RGB,width=160,height=90 ! pnmenc \
    ! multifilesink location="$out/frames/f-%05d.ppm" > "$out/gst.log" 2>&1 & pids+=($!)
consumer=$!
# No session manager runs here to link the two, so it is done by hand.
for _ in $(seq 1 50); do
    from="$(pw-link -o | grep -m1 'xdg-desktop-portal-wlr:' || true)"
    to="$(pw-link -i | grep -m1 'gst-launch' || true)"
    [[ -n "$from" && -n "$to" ]] && break
    sleep 0.1
done
pw-link "$from" "$to"
wait "$consumer" || true

frames="$(find "$out/frames" -name 'f-*.ppm' | wc -l)"
changes="$(find "$out/frames" -name 'f-*.ppm' | sort | xargs -r md5sum \
    | awk '$1 != last { n++ } { last = $1 } END { print n + 0 }')"
echo "$frames frames in ${seconds} s, the picture changed $changes times; evidence: $out"
# The square moves ten times a second. Half of that over the whole run is
# a stream; one frame, or one picture, is #387.
(( frames >= seconds * 10 && changes >= seconds * 5 )) \
    || { echo "SCREENCAST FAILED: the stream stopped"; exit 1; }
echo "screencast passed"
