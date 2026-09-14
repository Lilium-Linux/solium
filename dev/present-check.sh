#!/usr/bin/env bash
#
# Does a presentation transform put pixels where it says it does?
#
# Three claims, none of which `cargo test` can reach. The `Command::Present` ->
# `Frame` wiring in `state.rs` is the seam this exists for: reverting `z` and
# `pivot` there to their defaults passes the whole unit suite, because every
# test of them is a test of `script.rs` and of pure functions below it, and
# nothing between the script and the screen is exercised by any of them.
#
#   pivot   a pivot is the point the matrix leaves alone. One window is
#           presented twice -- `rotate_z = 20` with the default pivot, and
#           again with `pivot_x = 0, pivot_y = 0` -- and its top-left corner
#           must be at the same pixel in the second as in an untransformed
#           frame, and not in the first.
#
#   depth   a raised window is drawn in front. Two overlapping windows; the
#           lower one is presented with `z = 1` and the overlap must change
#           hands.
#
#   input   clicks did not move. With the raised window still raised, a click
#           in the overlap must focus the window the *layout* has on top, not
#           the one now drawn on top. `rect` is the truth for input and `z`
#           never enters the hit test -- see `Solium::window_under`. This is
#           the half a screenshot cannot show.
#
# A reverted `z` reads as "nothing sorts" and would be caught by looking. A
# reverted `pivot` is not: it draws a perfectly good-looking rotation about the
# centre, identical in all four corners to the one that asked for the centre.
# That is why this measures corners rather than describing pictures.
#
# Nested, on the host. Only *builds* need the container.
#
# `SOLIUM_QML_GPU` is deliberately never set: nested there is no GBM device, so
# the wallpaper, the cursor and every decoration silently fail to load. The
# configurations here turn QML off on purpose instead -- `sol.pane("none")` and
# no wallpaper -- so a window is exactly its outer rect against the backdrop
# and its corner can be measured rather than guessed at.
#
# Each check's frames come from *one* run, with `SOLIUM_TRIGGER_AT` firing
# between captures. Separate runs would place the client differently each time,
# and then "the corner moved" would be measuring kitty rather than Solium.
#
#   SOLIUM_CHECK_DIR=/somewhere   keep the captures and the logs
set -uo pipefail

if [[ -z "${WAYLAND_DISPLAY:-}" ]]; then
    echo "refusing to start: WAYLAND_DISPLAY is empty or unset." >&2
    echo "an empty value resolves to the default socket, not to 'no display'." >&2
    exit 1
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
binary="$root/target/debug/solium"
here="$root/dev/present-check"
[[ -x "$binary" ]] || { echo "not built: $binary  (see dev/README.md)" >&2; exit 1; }
command -v kitty >/dev/null || { echo "present-check: needs kitty as a client" >&2; exit 1; }

out="${SOLIUM_CHECK_DIR:-$(mktemp -d -t solium-present-XXXXXX)}"
mkdir -p "$out"
failures=0
note() { echo "  $*"; }
fail() { echo "  FAIL: $*"; failures=$((failures + 1)); }

# Every pid started here is recorded, and only those are ever killed. A
# developer running this has their own session -- and quite possibly their own
# nested Solium -- and `pkill solium` would take both.
pids="$out/pids"
: >"$pids"
cleanup() {
    while read -r pid; do [[ -n "$pid" ]] && kill "$pid" 2>/dev/null; done <"$pids"
    sleep 0.4
    while read -r pid; do [[ -n "$pid" ]] && kill -9 "$pid" 2>/dev/null; done <"$pids"
}
trap cleanup EXIT

# Run one configuration: start the compositor, open the clients it expects in
# the order it places them, and wait for the burst.
#
# `$1` name, `$2` lua, `$3` capture-at ms, `$4` frames, `$5` interval ms,
# `$6` extra environment (newline separated), rest: `title:rrggbb` per client.
shoot() {
    local name="$1" lua="$2" at="$3" frames="$4" interval="$5" extra="$6"
    shift 6
    local dir="$out/$name"
    mkdir -p "$dir"
    rm -f "$dir"/f-*

    local -a environment=(
        "SOLIUM_LUA_INIT=$lua"
        "SOLIUM_CAPTURE=$dir/f"
        "SOLIUM_CAPTURE_AT=$at"
        "SOLIUM_CAPTURE_FRAMES=$frames"
        "SOLIUM_CAPTURE_INTERVAL=$interval"
    )
    while IFS= read -r line; do
        [[ -n "$line" ]] && environment+=("$line")
    done <<<"$extra"

    env "${environment[@]}" "$binary" >"$dir/log" 2>&1 &
    local solium=$!
    echo "$solium" >>"$pids"

    # The socket is chosen at runtime, so it is read back rather than assumed.
    local socket=""
    for _ in $(seq 1 200); do
        kill -0 "$solium" 2>/dev/null || { echo "  solium exited early"; tail -5 "$dir/log"; return 1; }
        socket="$(grep -oE 'socket=wayland-[0-9]+' "$dir/log" | tail -1 | cut -d= -f2)"
        [[ -n "$socket" ]] && break
        sleep 0.1
    done
    [[ -n "$socket" ]] || { echo "  solium never reported a socket"; tail -5 "$dir/log"; return 1; }

    # One client at a time, because the configuration places them by the order
    # they open. Everything is one colour -- foreground and cursor included --
    # with a marker block of another printed at the home position. The marker
    # is what makes a corner findable after an arbitrary transform: it travels
    # with the top-left, so the quad vertex nearest it is the top-left.
    local index=0
    for spec in "$@"; do
        local title="${spec%%:*}" colour="${spec##*:}"
        WAYLAND_DISPLAY="$socket" kitty --config NONE --title "$title" \
            -o "background=#$colour" -o "foreground=#$colour" \
            -o "cursor=#$colour" -o "cursor_text_color=#$colour" \
            -o "background_opacity=1.0" -o "window_padding_width=0" \
            -o "window_margin_width=0" -o "single_window_margin_width=0" \
            -o "placement_strategy=top-left" -o "confirm_os_window_close=0" \
            -o "enable_audio_bell=no" -o "font_size=14" \
            sh -c "printf '\033[?25l\033[H\033[48;2;255;255;0m    \033[0m'; sleep 600" \
            >"$dir/client-$index.log" 2>&1 &
        echo "$!" >>"$pids"
        index=$((index + 1))
        sleep 1.6
    done

    for _ in $(seq 1 250); do
        [[ "$(ls "$dir"/f-* 2>/dev/null | wc -l)" -ge "$frames" ]] && break
        sleep 0.1
    done
    sleep 1.0
    [[ "$(ls "$dir"/f-* 2>/dev/null | wc -l)" -ge "$frames" ]] \
        || { echo "  only $(ls "$dir"/f-* 2>/dev/null | wc -l) of $frames frames were captured"; return 1; }
    return 0
}

measure() { env -u LD_LIBRARY_PATH python3 "$here/measure.py" "$@"; }

echo "present-check: pivot"
# Captures at 3500, 5500 and 7500; the two presents land between them.
if shoot pivot "$here/pivot.lua" 3500 3 2000 \
    "SOLIUM_TRIGGER_AT=4500:super+a,6500:super+b" \
    "window:20a0c0"
then
    plain=$(measure corner "$out/pivot/f-000" 32 160 192 255 255 0 8 | grep TOP-LEFT)
    centre=$(measure corner "$out/pivot/f-001" 32 160 192 255 255 0 8 | grep TOP-LEFT)
    corner=$(measure corner "$out/pivot/f-002" 32 160 192 255 255 0 8 | grep TOP-LEFT)
    note "untransformed      ${plain#*= }"
    note "default pivot      ${centre#*= }"
    note "pivot_x/y = 0      ${corner#*= }"
    # The window is placed at 420,190 520x360 and turned by 20 degrees. About
    # its own top-left the corner cannot move; about its centre it lands on
    # (497.3, 111.9), which is arithmetic and not a recorded observation.
    [[ "$plain" == *"= (420,190)"* ]]  || fail "the untransformed corner is not at (420,190)"
    [[ "$corner" == *"= (420,190)"* ]] || fail "pivot_x/y = 0 moved the corner it must leave alone"
    [[ "$centre" == *"= (497,112)"* ]] || fail "the default pivot is not the centre it replaced"
else
    fail "the pivot capture did not run"
fi

echo "present-check: depth and input"
# The lower window is the one opened first: a later window is mapped restacked
# to the top. So raising the first with z = 1 draws it in front of the one the
# layout has on top, which is the only arrangement that can tell the two apart.
if shoot depth "$here/depth.lua" 3500 3 2000 \
    "$(printf 'SOLIUM_TRIGGER_AT=4300:super+s,4500:super+z,7300:super+d\nSOLIUM_DRAG_AT=6500:670,450>670,450')" \
    "lower:20a0c0" "upper:d04020"
then
    before=$(measure at "$out/depth/f-000" 670 450)
    after=$(measure at "$out/depth/f-001" 670 450)
    note "overlap before     ${before#*= }"
    note "overlap after      ${after#*= }"
    [[ "$before" == *"rgb(208, 64, 32)"* ]] || fail "the upper window is not drawn over the lower one to begin with"
    [[ "$after" == *"rgb(32, 160, 192)"* ]] || fail "z = 1 did not bring the lower window in front"

    # Hit-testing does not move. The stack dump is topmost-first, and the
    # window that took the click must be the one at its head.
    stack=$(grep -o 'STACK raised topmost-first: [0-9]*' "$out/depth/log" | tail -1)
    focus=$(grep -o 'FOCUS id=[0-9]*' "$out/depth/log" | tail -1)
    note "layout topmost     ${stack##* }"
    note "click focused      ${focus#*=}"
    if [[ -z "$stack" || -z "$focus" ]]; then
        fail "the click was never reported (stack='$stack' focus='$focus')"
    elif [[ "${stack##* }" != "${focus#*=}" ]]; then
        fail "the click followed what is drawn on top, not what the layout has on top"
    fi
else
    fail "the depth capture did not run"
fi

if [[ "$failures" -gt 0 ]]; then
    echo "present-check: FAILED ($failures) -- frames and logs in $out"
    exit 1
fi
echo "present-check: passed"
[[ -n "${SOLIUM_CHECK_DIR:-}" ]] || rm -rf "$out"
