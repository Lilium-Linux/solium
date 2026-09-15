#!/usr/bin/env bash
#
# Does a presentation transform put pixels where it says it does, and does the
# pointer still land where it should?
#
# Four claims, none of which `cargo test` can reach. The `Command::Present` ->
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
#   input   clicks did not follow it. With the raised window still raised, a
#           click in the overlap must focus the window the *layout* has on top,
#           not the one now drawn on top.
#
#   rect    but they did follow the rect. A window presented somewhere else
#           takes clicks at the rect it is *drawn* at and not at the one it
#           lives at. This is the load-bearing half: `state.rs` tests
#           `drawn_at(..).rect.contains(location)`, and a regression to
#           `outer.contains` would pass the other three.
#
# A reverted `z` reads as "nothing sorts" and would be caught by looking. A
# reverted `pivot` is not, and the strongest statement of that is measured:
# with `pivot` dropped, the frame asking for `pivot 0,0` comes back **byte for
# byte identical** to the frame asking for the centre -- `cmp -l` reports zero
# differing bytes, against 382,961 between the two correct frames. There is no
# visual signal to miss, because there is none. That is why this measures
# corners rather than describing pictures.
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
note() { printf '  %-22s %s\n' "$1" "$2"; }
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

# Clients are started one at a time, because every configuration places them by
# the order they open. `ready_at` is when the last of them is certainly mapped
# and drawn, and every capture and trigger below is expressed from it rather
# than from a number typed once and left to rot: adding a client to a
# configuration used to silently move its first capture in front of its last
# window.
SPAWN_INTERVAL=1800
ready_at() { echo $((2500 + $1 * SPAWN_INTERVAL)); }

# What a capture is called. **With `SOLIUM_CAPTURE_FRAMES=1` the file is not
# numbered**: `winit.rs` gives a single capture the name it was handed, and
# numbers only a burst. Globbing `f-*` for a single frame waits out the whole
# timeout and then reports "0 of 1 frames", which is a lie about the
# compositor.
captured() {
    local dir="$1" frames="$2"
    if [[ "$frames" -le 1 ]]; then
        [[ -s "$dir/f" ]] && echo "$dir/f"
    else
        ls "$dir"/f-* 2>/dev/null
    fi
}

# Run one configuration: start the compositor, open the clients it expects in
# the order it places them, wait for the burst, and hold it up until everything
# the configuration scheduled has actually happened.
#
# **`until` is not the same as "the frames arrived".** A configuration may
# schedule clicks after its last capture -- the rect check does, deliberately,
# so its picture is taken before anything is clicked -- and tearing down as
# soon as the file appeared killed the compositor mid-run with the interesting
# half of the log never written.
#
# `$1` name, `$2` lua, `$3` capture-at ms, `$4` frames, `$5` interval ms,
# `$6` the last moment the configuration scheduled anything, in ms,
# `$7` extra environment (newline separated), rest: `title:rrggbb` per client.
shoot() {
    local name="$1" lua="$2" at="$3" frames="$4" interval="$5" until_ms="$6" extra="$7"
    shift 7
    local started
    started=$(date +%s%3N)
    local dir="$out/$name"
    mkdir -p "$dir"
    rm -f "$dir"/f "$dir"/f-*

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

    # Everything is one colour -- foreground and cursor included -- with a
    # marker block of another printed at the home position. The marker is what
    # makes a corner findable after an arbitrary transform: it travels with the
    # top-left, so the quad vertex nearest it is the top-left.
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
        sleep "$(awk "BEGIN{print $SPAWN_INTERVAL/1000}")"
    done

    # Generously past the last capture: the caller's schedule is in the
    # compositor's own clock and this one is not.
    for _ in $(seq 1 500); do
        [[ "$(captured "$dir" "$frames" | wc -l)" -ge "$frames" ]] && break
        sleep 0.1
    done
    sleep 1.0
    local have
    have="$(captured "$dir" "$frames" | wc -l)"
    [[ "$have" -ge "$frames" ]] || { echo "  only $have of $frames frames were captured"; return 1; }

    # Two seconds past the last thing the configuration asked for, measured
    # from when the compositor was started -- which is the clock its own
    # `SOLIUM_*_AT` numbers are in, near enough.
    local left
    left=$(( until_ms + 2000 - ($(date +%s%3N) - started) ))
    [[ "$left" -gt 0 ]] && sleep "$(awk "BEGIN{print $left/1000}")"
    return 0
}

# `-u LD_LIBRARY_PATH` as `cursor-check.sh` does: a value inherited from a Qt
# or container environment makes the host interpreter load libraries it was not
# built against, and the failure reads as a broken capture rather than as a
# broken environment.
measure() { env -u LD_LIBRARY_PATH python3 "$here/measure.py" "$@"; }

# The window colour and the marker colour every configuration uses.
WINDOW=(32 160 192)
MARKER=(255 255 0)

corner_of() {
    measure corner "$1" "${WINDOW[@]}" "${MARKER[@]}" 8 \
        | sed -n 's/.*TOP-LEFT = (\(-\?[0-9]*\),\(-\?[0-9]*\)).*/\1 \2/p'
}

# Within a pixel or two, not exactly. The expected values are arithmetic --
# rotating 420,190 520x360 by twenty degrees about its centre lands the corner
# on (497.3, 111.9) -- and a rasteriser that rounds one pixel differently is
# not a compositor that stopped honouring a pivot.
TOLERANCE=2
near() {
    local got="$1" want_x="$2" want_y="$3" label="$4"
    local -a xy=($got)
    if [[ "${#xy[@]}" -ne 2 ]]; then
        fail "$label: no corner could be measured"
        return
    fi
    local dx=$((xy[0] - want_x)) dy=$((xy[1] - want_y))
    [[ "$dx" -lt 0 ]] && dx=$((-dx))
    [[ "$dy" -lt 0 ]] && dy=$((-dy))
    if [[ "$dx" -gt "$TOLERANCE" || "$dy" -gt "$TOLERANCE" ]]; then
        fail "$label: (${xy[0]},${xy[1]}), expected within $TOLERANCE px of ($want_x,$want_y)"
    fi
}

# The id the compositor said held focus at a named moment, and the ids a
# configuration announced for its windows.
focused_at() { grep -o "FOCUSED $2 id=[0-9]*" "$1" | tail -1 | grep -o '[0-9]*$'; }
id_of()      { grep -o "IDS .*" "$1" | tail -1 | grep -o "$2=[0-9]*" | grep -o '[0-9]*$'; }

echo "present-check: pivot"
ready=$(ready_at 1)
if shoot pivot "$here/pivot.lua" "$ready" 3 2000 $((ready + 4000)) \
    "SOLIUM_TRIGGER_AT=$((ready + 1000)):super+a,$((ready + 3000)):super+b" \
    "window:20a0c0"
then
    plain=$(corner_of "$out/pivot/f-000")
    centre=$(corner_of "$out/pivot/f-001")
    corner=$(corner_of "$out/pivot/f-002")
    note "untransformed" "(${plain// /,})"
    note "default pivot" "(${centre// /,})"
    note "pivot_x/y = 0" "(${corner// /,})"
    near "$plain"  420 190 "the untransformed corner"
    near "$centre" 497 112 "the default pivot is not the centre it replaced"
    near "$corner" 420 190 "pivot_x/y = 0 moved the corner it must leave alone"
else
    fail "the pivot capture did not run"
fi

echo "present-check: depth and input"
# The lower window is the one opened first: a later window is mapped restacked
# to the top. The third overlaps neither and opens last, so it holds focus when
# the click happens -- without it, "focus is the upper window" cannot be told
# from "the click did nothing".
ready=$(ready_at 3)
if shoot depth "$here/depth.lua" "$ready" 3 2000 $((ready + 4000)) \
    "$(printf 'SOLIUM_TRIGGER_AT=%d:super+s,%d:super+z,%d:super+d\nSOLIUM_DRAG_AT=%d:670,450>670,450' \
        $((ready + 700)) $((ready + 900)) $((ready + 3400)) $((ready + 2600)))" \
    "lower:20a0c0" "upper:d04020" "parked:40a040"
then
    log="$out/depth/log"
    lower=$(id_of "$log" lower); upper=$(id_of "$log" upper); parked=$(id_of "$log" parked)
    before=$(measure at "$out/depth/f-000" 670 450)
    after=$(measure at "$out/depth/f-001" 670 450)
    note "overlap before" "${before#*= }"
    note "overlap after" "${after#*= }"
    [[ "$before" == *"rgb(208, 64, 32)"* ]] || fail "the upper window is not drawn over the lower one to begin with"
    [[ "$after" == *"rgb(32, 160, 192)"* ]] || fail "z = 1 did not bring the lower window in front"

    was=$(focused_at "$log" raised)
    now=$(focused_at "$log" after-click)
    note "focus before click" "$was  (parked=$parked)"
    note "focus after click" "$now  (upper=$upper lower=$lower)"
    if [[ -z "$lower" || -z "$upper" || -z "$parked" || -z "$was" || -z "$now" ]]; then
        fail "the run did not report its ids and focus (see $log)"
    elif [[ "$was" != "$parked" ]]; then
        fail "the parked window did not hold focus before the click, so the click proves nothing"
    elif [[ "$now" == "$lower" ]]; then
        fail "the click followed what is drawn on top, not what the layout has on top"
    elif [[ "$now" != "$upper" ]]; then
        fail "the click in the overlap focused $now, which is neither window in it"
    fi
else
    fail "the depth capture did not run"
fi

echo "present-check: the rect clicks follow"
# One window presented away from where it lives, and a second parked elsewhere
# holding focus. A hit test against real geometry gets both clicks exactly
# backwards.
ready=$(ready_at 2)
if shoot rect "$here/rect.lua" $((ready + 1400)) 1 16 $((ready + 4200)) \
    "$(printf 'SOLIUM_TRIGGER_AT=%d:super+p,%d:super+s,%d:super+d\nSOLIUM_DRAG_AT=%d:350,250>350,250;%d:910,400>910,400' \
        $((ready + 700)) $((ready + 2800)) $((ready + 4200)) \
        $((ready + 2100)) $((ready + 3500)))" \
    "moved:20a0c0" "parked:8040c0"
then
    log="$out/rect/log"
    moved=$(id_of "$log" moved); parked=$(id_of "$log" parked)
    # The pixels first, so a failure below is a hit test and not a present that
    # never happened.
    empty=$(measure at "$out/rect/f" 350 250)
    filled=$(measure at "$out/rect/f" 910 400)
    note "at the real rect" "${empty#*= }"
    note "at the drawn rect" "${filled#*= }"
    [[ "$filled" == *"rgb(32, 160, 192)"* ]] || fail "the window was not drawn at the rect it was presented at"
    [[ "$empty" == *"rgb(32, 160, 192)"* ]] && fail "the window is still drawn at the rect it lives at"

    after_real=$(focused_at "$log" after-real-click)
    after_drawn=$(focused_at "$log" after-drawn-click)
    note "click the real rect" "$after_real  (parked=$parked)"
    note "click the drawn rect" "$after_drawn  (moved=$moved)"
    if [[ -z "$moved" || -z "$parked" || -z "$after_real" || -z "$after_drawn" ]]; then
        fail "the run did not report its ids and focus (see $log)"
    else
        [[ "$after_real" == "$parked" ]] \
            || fail "a click at the rect the window *lives* at focused $after_real: the hit test is using real geometry"
        [[ "$after_drawn" == "$moved" ]] \
            || fail "a click at the rect the window is *drawn* at focused $after_drawn, not $moved"
    fi
else
    fail "the rect capture did not run"
fi

if [[ "$failures" -gt 0 ]]; then
    echo "present-check: FAILED ($failures) -- frames and logs in $out"
    exit 1
fi
echo "present-check: passed"
[[ -n "${SOLIUM_CHECK_DIR:-}" ]] || rm -rf "$out"
