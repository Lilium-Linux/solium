#!/usr/bin/env bash
# Check real XGrabKeyboard delivery and reclaim through nested XWayland.
# Build solium and x11-probe first. Run under the machine's nested-run lock.
set -euo pipefail
: "${WAYLAND_DISPLAY:?explicit host Wayland socket required}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="${SOLIUM_CHECK_DIR:-$(mktemp -d -t solium-shortcuts-XXXXXX)}"
mkdir -p "$out"
compositor=""; client=""
cleanup() {
    for pid in "$client" "$compositor"; do
        if [[ -n "$pid" ]]; then
            kill "$pid" 2>/dev/null || true
            wait "$pid" 2>/dev/null || true
        fi
    done
}
trap cleanup EXIT
trap 'echo "shortcuts check failed; logs: $out" >&2' ERR
for pass in 1 2 deny; do
    folder="$out/$pass"; mkdir -p "$folder"
    cat > "$folder/init.lua" <<'EOF'
sol.pane("none")
sol.session{systemd=false, autostart=false}
sol.keyboard{layout="us,ru", active=2}
sol.input{shortcuts_inhibit={allow=true, escape="super+Escape"}}
sol.bind("super+q", function() sol.log("SHORTCUT_Q") end)
sol.bind("alt+Tab", function() sol.log("SHORTCUT_TAB") end)
local return_to
sol.bind("super+F2", function()
    local other
    for _, window in ipairs(sol.windows()) do
        if window.app_id == "x11-probe" then
            if window.focused then return_to = window.id
            else other = window.id end
        end
    end
    assert(return_to and other, "both probe windows must be mapped")
    sol.focus(other)
    sol.log("FOCUS_SPARE")
end, {inhibited=true})
sol.bind("super+F3", function()
    assert(return_to, "focus must have left the grabbed window")
    sol.focus(return_to)
    sol.log("FOCUS_GRAB")
end, {inhibited=true})
EOF
    if [[ "$pass" == deny ]]; then
        echo 'sol.input{shortcuts_inhibit={apps={deny={"x11-probe"}}}}' >> "$folder/init.lua"
    fi
    start=$SECONDS
    env -u WAYLAND_SOCKET -u SOLIUM_SESSION_BUS -u SOLIUM_LOGIND_BUS \
        -u SOLIUM_TRIGGER_AT -u SOLIUM_DRAG_AT -u SOLIUM_CAPTURE \
        -u SOLIUM_CAPTURE_FRAMES -u SOLIUM_CLICK_AT \
        SOLIUM_QML=software SOLIUM_LUA_INIT="$folder/init.lua" \
        XDG_CONFIG_HOME="$folder/config" XDG_STATE_HOME="$folder/state" \
        SOLIUM_KEY_AT="6000:super+q,8000:alt+Tab,10000:super+F2,12000:super+q,14000:super+F3,16000:super+q,18000:super+Escape,20000:super+q,22000:alt+Tab" \
        SOLIUM_OUTPUTS=1 RUST_LOG=solium=info,smithay=warn NO_COLOR=1 \
        timeout -k 5s -s TERM 75s "$root/target/debug/solium" > "$folder/compositor.log" 2>&1 &
    compositor=$!
    socket=""; display=""
    for _ in $(seq 1 150); do
        kill -0 "$compositor" 2>/dev/null || { cat "$folder/compositor.log"; exit 1; }
        socket="$(sed -n 's/.*socket=\(wayland-[0-9]*\).*/\1/p' "$folder/compositor.log" | tail -1)"
        display="$(sed -n 's/.*XWayland is up display=\([0-9]*\).*/\1/p' "$folder/compositor.log" | tail -1)"
        [[ -n "$socket" && -n "$display" ]] && break
        sleep 0.1
    done
    [[ -n "$socket" && "$socket" != "$WAYLAND_DISPLAY" && -S "$XDG_RUNTIME_DIR/$socket" && -n "$display" ]] \
        || { echo "no distinct nested socket and XWayland display"; exit 1; }
    env -u WAYLAND_SOCKET DISPLAY=":$display" WAYLAND_DISPLAY="$socket" \
        X11_PROBE_HOLD=3000 timeout -k 2s 38s "$root/target/debug/x11-probe" keyboard-grab > "$folder/client.log" 2>&1 &
    client=$!
    wait "$client"; client=""
    q_bindings=2; tab_bindings=1; q_forwarded=2; tab_forwarded=1
    [[ "$pass" == deny ]] && { q_bindings=4; tab_bindings=2; q_forwarded=0; tab_forwarded=0; }
    [[ "$(grep -c 'SHORTCUT_Q' "$folder/compositor.log")" -eq "$q_bindings" ]]
    [[ "$(grep -c 'SHORTCUT_TAB' "$folder/compositor.log")" -eq "$tab_bindings" ]]
    [[ "$(grep -c 'FOCUS_SPARE' "$folder/compositor.log")" -eq 1 ]]
    [[ "$(grep -c 'FOCUS_GRAB' "$folder/compositor.log")" -eq 1 ]]
    grep -q 'keyboard SUCCESS' "$folder/client.log"
    for key in 24 23; do
        forwarded="$tab_forwarded"
        [[ "$key" == 24 ]] && forwarded="$q_forwarded"
        [[ "$(grep -c "key press: $key$" "$folder/client.log")" -eq "$forwarded" ]]
        [[ "$(grep -c "key release: $key$" "$folder/client.log")" -eq "$forwarded" ]]
    done
    if [[ "$pass" != deny ]]; then
        ! grep -q 'key press: 9$' "$folder/client.log"
    fi
    hold=35
    [[ "$pass" == 1 ]] && hold="${SOLIUM_SHORTCUTS_HOLD:-61}"
    while (( SECONDS - start < hold )); do
        kill -0 "$compositor" 2>/dev/null || { echo "compositor exited early"; exit 1; }
        sleep 0.2
    done
    kill -0 "$compositor"
    cleanup; compositor=""
    echo "PASS XWayland shortcut forwarding and reclaim, Russian layout ($pass)"
done
echo "shortcuts passed; logs: $out"
