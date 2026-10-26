#!/usr/bin/env bash
# Compare real screencopy crops with full-output pixels, including hosted QML
# and layer-shell. Run with an explicit host WAYLAND_DISPLAY, after building.
set -euo pipefail
: "${WAYLAND_DISPLAY:?explicit host Wayland socket required}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="${SOLIUM_CHECK_DIR:-$(mktemp -d -t solium-screencopy-XXXXXX)}"
mkdir -p "$out"
binary="$root/target/debug/solium"
probe="$root/target/debug/wl-probe"
[[ -x "$binary" && -x "$probe" ]] || { echo 'build solium and wl-probe first' >&2; exit 1; }
compositor=""
cleanup() {
    if [[ -n "$compositor" ]]; then
        kill "$compositor" 2>/dev/null || true
        wait "$compositor" 2>/dev/null || true
    fi
}
trap cleanup EXIT
for variant in ${SOLIUM_COPY_CASES:-single-1x single-2x mixed fractional}; do
    folder="$(mktemp -d "$out/$variant-XXXXXX")"
    mkdir -p "$folder/config" "$folder/state"
    count=1; extra=()
    case "$variant" in
        single-1x) monitors='{{name="winit", scale=1}}' ;;
        single-2x) monitors='{{name="winit", scale=2}}' ;;
        mixed) count=2; monitors='{{name="winit-1",scale=1},{name="winit-2",scale=2,right_of="winit-1",align="start"}}' ;;
        fractional) monitors='{{name="winit",scale=1.5}}'; extra=(WL_PROBE_CROP_SCALE=1.5) ;;
        *) echo "unknown case: $variant" >&2; exit 2 ;;
    esac
    cat > "$folder/init.lua" <<EOF
sol.session{systemd=false,autostart=false}
sol.monitors($monitors)
sol.surface("fixture", {scene="$root/dev/screencopy-check/fixture.qml", layer="background"})
EOF
    start=$SECONDS
    env -u WAYLAND_SOCKET -u SOLIUM_SESSION_BUS -u SOLIUM_LOGIND_BUS \
        -u SOLIUM_CAPTURE_FRAMES -u SOLIUM_TRIGGER_AT -u SOLIUM_KEY_AT -u SOLIUM_DRAG_AT \
        SOLIUM_QML=software SOLIUM_LUA_INIT="$folder/init.lua" \
        XDG_CONFIG_HOME="$folder/config" XDG_STATE_HOME="$folder/state" \
        SOLIUM_OUTPUTS="$count" RUST_LOG=solium=info,smithay=warn \
        SOLIUM_CAPTURE="$folder/desktop.ppm" SOLIUM_CAPTURE_AT=1800 \
        timeout -k 5s -s TERM 75s "$binary" > "$folder/compositor.log" 2>&1 &
    compositor=$!
    socket=""
    for _ in $(seq 1 150); do
        kill -0 "$compositor" 2>/dev/null || { echo "$variant: compositor exited early"; exit 1; }
        socket="$(sed -n 's/.*socket=\(wayland-[0-9]*\).*/\1/p' "$folder/compositor.log" | tail -1)"
        [[ -n "$socket" ]] && break
        sleep 0.1
    done
    [[ -n "$socket" && "$socket" != "$WAYLAND_DISPLAY" && -S "$XDG_RUNTIME_DIR/$socket" ]] \
        || { echo "$variant: no distinct nested socket"; exit 1; }
    # The socket exists before the first complete scene has been rasterised.
    for _ in $(seq 1 150); do
        [[ -s "$folder/desktop.ppm" ]] && break
        kill -0 "$compositor" 2>/dev/null || { echo "$variant: compositor exited before capture"; exit 1; }
        sleep 0.1
    done
    [[ -s "$folder/desktop.ppm" ]] || { echo "$variant: no initial frame"; exit 1; }
    env -u DISPLAY -u WAYLAND_SOCKET -u WL_PROBE_WINDOWS -u WL_PROBE_POPUP \
        -u WL_PROBE_KEYBOARD -u WL_PROBE_IDLE -u WL_PROBE_LOCK -u WL_PROBE_BAR \
        -u WL_PROBE_CROP_SCALE WAYLAND_DISPLAY="$socket" WL_PROBE_CROP=1 \
        WL_PROBE_CROP_DIR="$folder" "${extra[@]}" \
        timeout -k 2s 25s "$probe" > "$folder/probe.log" 2>&1 \
        || { cat "$folder/probe.log"; exit 1; }
    [[ "$(grep -c '^PASS ' "$folder/probe.log")" -eq $((3 * count)) ]] \
        || { echo "$variant: not every crop check ran"; cat "$folder/probe.log"; exit 1; }
    # At least one ordinary run can be held for the survival check.
    while (( SECONDS - start < ${SOLIUM_COPY_HOLD:-3} )); do
        kill -0 "$compositor" 2>/dev/null || { echo "$variant: compositor exited early"; exit 1; }
        sleep 0.2
    done
    kill -0 "$compositor" 2>/dev/null || { echo "$variant: compositor exited"; exit 1; }
    cat "$folder/probe.log"
    cleanup; compositor=""
done
echo "screencopy passed; evidence: $out"
