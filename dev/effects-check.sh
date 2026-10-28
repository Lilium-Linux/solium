#!/usr/bin/env bash
#
# Effects, judged on screen, nested. Each section starts a compositor of its
# own with a configuration of its own (dev/effects-check/<section>.lua, and a
# configuration directory where it puts effect folders), captures what an
# effect should have drawn, and judges it with dev/effects-check/judge.py.
#
# Every run is judged beside dev/pulse-control.sh (spec §8.1, "Judging
# captures"): if the pulse frame does not move, the renderer is frozen, no
# capture here proves anything, and the whole check fails.
#
#   dev/effects-check.sh [section]...       every section when none is named
#
#   overlay   a broken init.lua reloaded: the overlay appears in the top-right
#             corner, and nothing else on screen changes; and a rule naming
#             an effect whose .frag has a typo: the overlay names the .frag
#             at its line
#   t0        a generated ring behind a window (the `ring` fixture, no
#             capture): the band just outside the window changes, the window
#             itself does not, and the ring runs once or twice, never once the
#             window is at rest
#   blur      a video player's own pixels blurred through a rule matching its
#             title, the terminal beside it untouched; and, the player drawing
#             throughout, a still window's chain run only when the window
#             commits, never in the last 3 s while the passes run on, and on
#             every pass with SOLIUM_RECAPTURE=always
#   none      no rules: no pass captures and no chain runs
#   fail      rules naming an effect that will not compile, in every slot of
#             every part of a window: at rest the frame is the one with no
#             rules
#   tilt      the player blurred through a rule, then held at 6 degrees, then
#             pulled into the bottom of the screen by a genie: blurred flat,
#             still blurred while tilted and while it genies, against the
#             same run with no rule
#   genie     a window held half way through the shipped genie (super+g),
#             drawn through the `genie` folder's grid and, on the same debug
#             build, through the Rust genie's (SOLIUM_GEOMETRY_ORACLE=genie):
#             every channel of the two frames within 1
#   xray      a translucent terminal with the wallpaper (a checker) blurred
#             behind it, sharp outside; the xray built once and its memo run
#             once at rest, and on every pass with SOLIUM_XRAY_REBUILD=always;
#             a moving wallpaper rendered once a pass with the rule as
#             without it, at effects.xray.scale 0.5 too; and a plate there
#             from the start, its backdrop blurred in the first frame drawn,
#             after three held to the clear colour by SOLIUM_WARM_INJECT=3
#   regions   three squares of pure colour on a hosted scene, each a
#             `Solium.region`, one inside a Row in a parent scaled 2x:
#             `sol.anchors` lists each where the capture shows it, within 1 px
#   motion    a clear terminal with the wallpaper (bars no two places of
#             which look alike) blurred behind it, held away from its
#             layout place, at six moments of a glide, and half way through
#             a genie: every pixel of the window is the wallpaper blurred
#             where that pixel is, and the genie with
#             effects.xray.warped = "rest" fails the same judge; a plain
#             blur runs no pass while it moves, a chain with a step per part
#             runs it while it glides and not at rest
#
# The player is ffplay's testsrc2, or a kitty printing the time when ffplay is
# missing.
#
# QML is software nested (#148). Only what this script started is ever
# killed: a developer may have a nested Solium of their own running.
#
#   SOLIUM_CHECK_DIR=/somewhere   keep the captures and the logs
set -uo pipefail

[[ -n "${WAYLAND_DISPLAY:-}" ]] || { echo "refusing to start: WAYLAND_DISPLAY is empty or unset." >&2; exit 1; }
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
binary="$root/target/debug/solium"
here="$root/dev/effects-check"
[[ -x "$binary" ]] || { echo "not built: $binary  (see dev/README.md)" >&2; exit 1; }
command -v kitty >/dev/null || { echo "effects-check: needs kitty as a client" >&2; exit 1; }

out="${SOLIUM_CHECK_DIR:-$(mktemp -d -t solium-effects-XXXXXX)}"
mkdir -p "$out"
failures=0
fail() { echo "  FAIL: $*"; failures=$((failures + 1)); }
pass() { echo "  ok: $*"; }
judge() { python3 "$here/judge.py" "$@"; }

pids="$out/pids"
: >"$pids"
stop() {
    while read -r pid; do [[ -n "$pid" ]] && kill "$pid" 2>/dev/null; done <"$pids"
    sleep 0.4
    while read -r pid; do [[ -n "$pid" ]] && kill -9 "$pid" 2>/dev/null; done <"$pids"
    : >"$pids"
}
trap stop EXIT

# run_scene <name> <lua> [extra environment...]: one compositor with
# $here/<lua> copied to $out/<name>/init.lua as its whole configuration and
# $out/<name>/config as its XDG_CONFIG_HOME, holding a copy of every effect
# folder named in $SECTION_EFFECTS (space-separated paths) under
# solium/effects/, as a user's would be, and of every QML file named in
# $SECTION_QML under solium/qml/, where `sol.surface` finds a scene by its
# file name (the FX3 sections' checker.qml). Sets $dir and $socket. The
# capture knobs come in the extra environment.
run_scene() {
    local name="$1" lua="$2"
    shift 2
    dir="$out/$name"
    mkdir -p "$dir/config/solium/effects" "$dir/config/solium/qml"
    cp "$here/$lua" "$dir/init.lua"
    local folder
    for folder in ${SECTION_EFFECTS:-}; do
        cp -R "$folder" "$dir/config/solium/effects/"
    done
    for folder in ${SECTION_QML:-}; do
        cp "$folder" "$dir/config/solium/qml/"
    done
    rm -f "$dir"/f-*
    env SOLIUM_QML=software SOLIUM_LUA_INIT="$dir/init.lua" XDG_CONFIG_HOME="$dir/config" \
        SOLIUM_CAPTURE="$dir/f" "$@" \
        timeout -k 5 -s TERM 60 "$binary" >"$dir/log" 2>&1 &
    local solium=$!
    echo "$solium" >>"$pids"
    socket=""
    for _ in $(seq 1 200); do
        kill -0 "$solium" 2>/dev/null || { fail "$name: solium exited early; see $dir/log"; return 1; }
        socket="$(grep -oE 'socket=wayland-[0-9]+' "$dir/log" | tail -1 | cut -d= -f2)"
        [[ -n "$socket" ]] && return 0
        sleep 0.1
    done
    fail "$name: no socket; see $dir/log"
    return 1
}

# wait_frames <count>: until $dir holds that many captures, at most 20 s.
wait_frames() {
    for _ in $(seq 1 200); do
        [[ "$(ls "$dir"/f-* 2>/dev/null | wc -l)" -ge "$1" ]] && return 0
        sleep 0.1
    done
    fail "$(basename "$dir"): $1 frames never came; see $dir/log"
    return 1
}

# kitty_window <colour> [command]: one terminal of fixed content.
kitty_window() {
    WAYLAND_DISPLAY="$socket" kitty --config NONE -o "background=#$1" -o cursor_blink_interval=0 \
        -o confirm_os_window_close=0 -o font_size=14 \
        sh -c "${2:-printf '\033[?25leffects-check\n'; exec sleep 600}" >/dev/null 2>&1 &
    echo "$!" >>"$pids"
}

section_overlay() {
    # Frame 0 at 2.5 s with the configuration working; init.lua is broken on
    # disk as soon as frame 0 exists, which is always before the reload the
    # trigger presses at 3.5 s (both count from the compositor's start; a
    # `sleep` here would count from the socket line, which a slow start
    # delays); frame 1 at 5 s.
    run_scene overlay overlay.lua SOLIUM_TRIGGER_AT=3500:super+shift+r \
        SOLIUM_CAPTURE_AT=2500 SOLIUM_CAPTURE_FRAMES=2 SOLIUM_CAPTURE_INTERVAL=2500 || return
    wait_frames 1 || return
    printf 'this is not lua\n' >>"$dir/init.lua"
    wait_frames 2 || return
    grep -q 'reload failed' "$dir/log" || { fail "overlay: the reload did not fail, so nothing was tested"; return; }
    local frames=("$dir"/f-*)
    if judge differs "${frames[0]}" "${frames[1]}" 0.5 0 0.5 0.25 >/dev/null; then
        pass "overlay: the top-right corner changed after the broken reload"
    else
        fail "overlay: nothing appeared in the top-right corner"
    fi
    judge same "${frames[0]}" "${frames[1]}" 0 0.5 1 0.5 >/dev/null \
        || fail "overlay: the bottom half changed too, so the corner is not what was judged"

    # A rule naming the `typo` fixture, whose down.frag reads `p_ofset` on
    # line 2: the rule is refused, and the overlay names the .frag at that
    # line from the first frames. Judged against the working configuration's
    # frame above, which has no overlay.
    local working="${frames[0]}"
    stop
    SECTION_EFFECTS="$root/crates/solium/tests/fixtures/effects/typo" \
        run_scene overlay-effect overlay.lua BROKEN_EFFECT=1 \
        SOLIUM_CAPTURE_AT=2500 SOLIUM_CAPTURE_FRAMES=2 SOLIUM_CAPTURE_INTERVAL=1000 || return
    wait_frames 2 || return
    if grep -qE 'problem .*/typo/down\.frag:2: ' "$dir/log"; then
        pass "overlay-effect: the rule's effect is named at down.frag:2"
    else
        fail "overlay-effect: no problem at typo/down.frag:2 in $dir/log"
    fi
    frames=("$dir"/f-*)
    if judge differs "$working" "${frames[1]}" 0.5 0 0.5 0.25 >/dev/null; then
        pass "overlay-effect: the top-right corner shows the overlay"
    else
        fail "overlay-effect: nothing appeared in the top-right corner"
    fi
    judge same "$working" "${frames[1]}" 0 0.5 1 0.5 >/dev/null \
        || fail "overlay-effect: the bottom half changed too, so the corner is not what was judged"
}

# t0   a generated ring behind a window (the `ring` fixture, no capture):
#      the band just outside the window changes, the window itself does not.
section_t0() {
    local name
    for name in t0-off t0-on; do
        local ring=()
        [[ "$name" = t0-on ]] && ring=(T0_RING=1)
        SECTION_EFFECTS="$root/crates/solium/tests/fixtures/effects/ring" \
            run_scene "$name" t0.lua "${ring[@]}" SOLIUM_TRACE="$out/$name/trace.jsonl" \
            SOLIUM_CAPTURE_AT=6000 SOLIUM_CAPTURE_FRAMES=2 SOLIUM_CAPTURE_INTERVAL=100 || return
        kitty_window 3a6ea5
        wait_frames 2 || return
        stop
    done
    # A burst of two, because a single frame is written to the bare path and
    # not beside it, where `wait_frames` counts; the first is judged.
    local off on
    off="$(ls "$out/t0-off"/f-* | head -1)"
    on="$(ls "$out/t0-on"/f-* | head -1)"
    judge differs "$off" "$on" 408px 178px 12px 384px >/dev/null && pass "t0: the ring is drawn left of the window" || fail "t0: nothing was drawn around the window"
    judge same "$off" "$on" 440px 210px 480px 320px >/dev/null && pass "t0: the window itself is untouched" || fail "t0: the window changed under a behind effect"
    # The ring re-runs when its padded size changes, and kitty's first buffer
    # may be another size than the one it commits after sol.place: so once or
    # twice, and never once the window has settled (the last 2 s before the
    # capture at 6 s).
    local runs total
    read -r runs total < <(judge trace "$out/t0-on/trace.jsonl" effect_runs)
    runs="${runs:-0}" total="${total:-0}"
    (( total >= 1 && total <= 2 )) && pass "t0: the ring ran $total time(s), in $runs pass(es)" || fail "t0: the ring ran $total times"
    judge quiet "$out/t0-on/trace.jsonl" effect_runs 2000 >/dev/null && pass "t0: no run once the window was at rest" || fail "t0: the ring kept running with nothing changing"
}

# player <title>: ffplay's test pattern, titled, or a kitty printing the time
# ten times a second when ffplay is missing.
player() {
    if command -v ffplay >/dev/null; then
        env -u DISPLAY WAYLAND_DISPLAY="$socket" SDL_VIDEODRIVER=wayland ffplay -loglevel error -an -loop 0 \
            -window_title "$1" -f lavfi -i testsrc2=size=560x380:rate=60 >/dev/null 2>&1 &
    else
        WAYLAND_DISPLAY="$socket" kitty --config NONE --title "$1" -o cursor_blink_interval=0 \
            sh -c 'while :; do printf "\r%s" "$(date +%T.%N)"; sleep 0.1; done' >/dev/null 2>&1 &
    fi
    echo "$!" >>"$pids"
}

# The still terminal the `blur` and `none` sections put beside the player.
static_window() {
    WAYLAND_DISPLAY="$socket" kitty --config NONE --title effects-check-static -o "background=#3a6ea5" \
        -o cursor_blink_interval=0 -o confirm_os_window_close=0 -o font_size=14 \
        sh -c "printf '\033[?25leffects-check: sharp text\n'; exec sleep 600" >/dev/null 2>&1 &
    echo "$!" >>"$pids"
}

# translucent_kitty <title>: a terminal with a dark, half-transparent
# background, so what is behind it shows through, and no opaque region;
# printing its title. The section's .lua places it.
translucent_kitty() {
    WAYLAND_DISPLAY="$socket" kitty --config NONE --title "$1" -o "background=#202028" \
        -o background_opacity=0.5 -o cursor_blink_interval=0 -o confirm_os_window_close=0 \
        -o font_size=14 sh -c "printf '\033[?25l$1\n'; exec sleep 600" >/dev/null 2>&1 &
    echo "$!" >>"$pids"
}

# clear_kitty <title>: a terminal whose background is fully transparent and
# which prints nothing, so all of it is what is behind it.
clear_kitty() {
    WAYLAND_DISPLAY="$socket" kitty --config NONE --title "$1" -o background_opacity=0 \
        -o cursor_blink_interval=0 -o confirm_os_window_close=0 \
        sh -c "printf '\033[?25l'; exec sleep 600" >/dev/null 2>&1 &
    echo "$!" >>"$pids"
}

# blur  the player's own pixels blurred through a rule, the terminal beside it
#       untouched; and, the player drawing at 60 fps throughout, a still
#       window's chain runs only on the passes that captured the window, its
#       own few commits as it starts and loses the focus, and on none of the
#       last 3 s while the passes run on; on every pass with
#       SOLIUM_RECAPTURE=always. Nested drawing follows damage, so the player
#       keeps the passes coming; only the still window's runs say whether a
#       chain re-runs only when its part draws.
section_blur() {
    local name
    for name in blur-off blur-on blur-static blur-static-always; do
        local extra=()
        case "$name" in
            blur-on) extra=(BLUR=player) ;;
            blur-static) extra=(BLUR=static) ;;
            blur-static-always) extra=(BLUR=static SOLIUM_RECAPTURE=always) ;;
        esac
        # A burst of two, because a single frame is written to the bare path
        # and not beside it, where `wait_frames` counts; the first is judged.
        SECTION_EFFECTS="$root/crates/solium/effects/blur" \
            run_scene "$name" blur.lua "${extra[@]}" SOLIUM_TRACE="$out/$name/trace.jsonl" \
            SOLIUM_CAPTURE_AT=6000 SOLIUM_CAPTURE_FRAMES=2 SOLIUM_CAPTURE_INTERVAL=100 || return
        static_window
        sleep 1.5
        player effects-check-player
        wait_frames 2 || return
        stop
    done
    local off on
    off="$(ls "$out/blur-off"/f-* | head -1)"
    on="$(ls "$out/blur-on"/f-* | head -1)"
    local sharp soft
    sharp="$(judge edges "$off" 660px 80px 520px 340px)"
    soft="$(judge edges "$on" 660px 80px 520px 340px)"
    if (( soft * 2 < sharp )); then pass "blur: the player's edges fell from $sharp to $soft"; else fail "blur: the player is not blurred ($sharp -> $soft)"; fi
    judge same "$off" "$on" 60px 80px 520px 340px >/dev/null && pass "blur: the terminal is untouched" || fail "blur: the rule reached the terminal"
    local runs total
    read -r runs total < <(judge trace "$out/blur-on/trace.jsonl" effect_runs)
    runs="${runs:-0}" total="${total:-0}"
    (( runs > 0 )) && pass "blur: the player's chain ran on $runs passes" || fail "blur: the player's chain never ran"
    # The still window's chain runs only on a pass that captured the window,
    # which only its own commits make (kitty's first buffers, and its redraw
    # when the player takes the focus), and not at all in the last 3 s, while
    # the player draws on; with SOLIUM_RECAPTURE=always, on every pass.
    local passes still ran captured late quiet forced
    read -r passes still < <(judge after "$out/blur-static/trace.jsonl" effect_runs)
    read -r ran captured < <(judge only "$out/blur-static/trace.jsonl" effect_runs captures)
    read -r late quiet < <(judge since "$out/blur-static/trace.jsonl" effect_runs 3000)
    passes="${passes:-0}" still="${still:-0}" ran="${ran:-0}" captured="${captured:-0}" late="${late:-0}" quiet="${quiet:-0}"
    (( still >= 1 && ran == captured )) \
        && pass "blur: the still window's chain ran $still time(s) in $passes passes, each on a pass that captured it" \
        || fail "blur: the still window's chain ran on $ran passes, $captured of them capturing it"
    (( late >= 100 && quiet == 0 )) && pass "blur: no run in the last 3 s, $late passes while the player drew" \
        || fail "blur: the still window's chain ran $quiet times in the last 3 s, $late passes"
    read -r passes forced < <(judge after "$out/blur-static-always/trace.jsonl" effect_runs)
    read -r late quiet < <(judge since "$out/blur-static-always/trace.jsonl" effect_runs 3000)
    passes="${passes:-0}" forced="${forced:-0}" late="${late:-0}" quiet="${quiet:-0}"
    (( passes > 0 && forced >= passes - 2 && quiet >= late - 2 )) \
        && pass "blur: SOLIUM_RECAPTURE=always ran it on $forced of $passes passes, $quiet of the last $late" \
        || fail "blur: the control ran it on $forced of $passes passes ($quiet of the last $late), so the counts above prove nothing"
}

# none  no rules: no pass captures and no chain runs once the windows are up.
section_none() {
    run_scene none none.lua SOLIUM_TRACE="$out/none/trace.jsonl" \
        SOLIUM_CAPTURE_AT=6000 SOLIUM_CAPTURE_FRAMES=2 SOLIUM_CAPTURE_INTERVAL=100 || return
    static_window
    sleep 1.5
    player effects-check-player
    wait_frames 2 || return
    stop
    local captured runs
    read -r captured _ < <(judge trace "$out/none/trace.jsonl" captures)
    read -r runs _ < <(judge trace "$out/none/trace.jsonl" effect_runs)
    captured="${captured:-0}" runs="${runs:-0}"
    [[ -s "$out/none/trace.jsonl" ]] || { fail "none: no trace, so nothing was counted"; return; }
    (( captured == 0 && runs == 0 )) && pass "none: nothing captured, nothing run" || fail "none: $captured passes captured and $runs ran with no rule"
}

# fail  rules naming an effect that will not compile on this GPU, in every
#       slot of every part of a window: at rest the frame is the one with no
#       rules ([16] §5, spec §8.4).
section_fail() {
    local name
    for name in fail-off fail-on; do
        local extra=()
        [[ "$name" = fail-on ]] && extra=(FAIL=1)
        SECTION_EFFECTS="$root/crates/solium/tests/fixtures/effects/fail-compile" \
            run_scene "$name" fail.lua "${extra[@]}" \
            SOLIUM_CAPTURE_AT=5000 SOLIUM_CAPTURE_FRAMES=2 SOLIUM_CAPTURE_INTERVAL=100 || return
        kitty_window 3a6ea5
        wait_frames 2 || return
        stop
    done
    grep -q 'problem .*fail-compile' "$out/fail-on/log" || { fail "fail: nothing failed, so nothing was tested"; return; }
    judge same "$(ls "$out/fail-off"/f-* | head -1)" "$(ls "$out/fail-on"/f-* | head -1)" 0 0 1 1 1 >/dev/null \
        && pass "fail: with every effect failing, every part is drawn as with no rules" \
        || fail "fail: a failing effect changed what was drawn"
}

# tilt  the player blurred through a rule and warped, so what is drawn is
#       its pane's capture, which walks its slots once the chains have run
#       (Ruling 17): held at 6 degrees from 4.5 s (super+t), then pulled into
#       the bottom of the screen over 3 s from 6.5 s (super+m, as the shipped
#       genie). Frames at 4 s (flat), 6 s (tilted) and 8 s (half way through
#       the genie), each judged against the same frame of a run with no rule
#       by its gradient energy inside the player (`judge sharpness`; `edges`
#       sums a widened step to its height, so it barely moves where the
#       genie squeezes the colour bars together): under half, flat, tilted
#       and in the genie. Built before the chains, the warp draws the bare
#       client, and the tilted and genied frames are as sharp as with no rule.
section_tilt() {
    local name
    for name in tilt-off tilt-on; do
        local extra=()
        [[ "$name" = tilt-on ]] && extra=(BLUR=player)
        SECTION_EFFECTS="$root/crates/solium/effects/blur" \
            run_scene "$name" tilt.lua "${extra[@]}" GENIE_MS=3000 \
            SOLIUM_TRIGGER_AT=4500:super+t,6500:super+m \
            SOLIUM_CAPTURE_AT=4000 SOLIUM_CAPTURE_FRAMES=3 SOLIUM_CAPTURE_INTERVAL=2000 || return
        static_window
        sleep 1.5
        player effects-check-player
        wait_frames 3 || return
        stop
    done
    local off=("$out/tilt-off"/f-*) on=("$out/tilt-on"/f-*)
    judge differs "${on[0]}" "${on[1]}" 720px 120px 400px 260px >/dev/null \
        || { fail "tilt: the player did not turn at super+t, so nothing was tested"; return; }
    judge differs "${on[1]}" "${on[2]}" 680px 80px 440px 200px >/dev/null \
        || { fail "tilt: the player did not move at super+m, so nothing was tested"; return; }
    # Inside the player by more than a 6-degree turn moves its border, and
    # in the genie inside its upper part, which is still wide half way: the
    # player's own pixels, never its edge against the background.
    local at sharp soft box frame=(flat tilted genie)
    for at in 0 1 2; do
        box=(720px 120px 400px 260px)
        (( at == 2 )) && box=(680px 80px 440px 200px)
        sharp="$(judge sharpness "${off[$at]}" "${box[@]}")"
        soft="$(judge sharpness "${on[$at]}" "${box[@]}")"
        if (( soft * 2 < sharp )); then
            pass "tilt: ${frame[$at]}, the player's gradient energy fell from $sharp to $soft"
        else
            fail "tilt: ${frame[$at]}, the player is not blurred ($sharp -> $soft)"
        fi
    done
}

# genie  the `genie` folder against the Rust genie it is held to, on one
#        debug build: a window held half way into a strip below the monitor
#        (super+g at 4.5 s; frames at 4 s, at rest, and 6 s), once with
#        SOLIUM_GEOMETRY_ORACLE=genie and once without; every channel of the
#        whole 6 s frame within 1. With GENIE_FAIL=1 the folder's run alone
#        is given spread 1.5 against the oracle's 1.4, and the two differ:
#        the check failing as it should.
section_genie() {
    local name
    for name in genie-oracle genie-folder; do
        local extra=()
        [[ "$name" = genie-oracle ]] && extra=(SOLIUM_GEOMETRY_ORACLE=genie)
        [[ "$name" = genie-folder && -n "${GENIE_FAIL:-}" ]] && extra=(GENIE_SPREAD=1.5)
        run_scene "$name" genie.lua "${extra[@]}" SOLIUM_TRIGGER_AT=4500:super+g \
            SOLIUM_CAPTURE_AT=4000 SOLIUM_CAPTURE_FRAMES=2 SOLIUM_CAPTURE_INTERVAL=2000 || return
        kitty_window 3a6ea5
        wait_frames 2 || return
        stop
    done
    local oracle=("$out/genie-oracle"/f-*) folder=("$out/genie-folder"/f-*)
    # The window was pulled, or the two runs agree for nothing: below where
    # it stood (its bottom edge is at 550) the genie frame differs from the
    # one at rest.
    judge differs "${folder[0]}" "${folder[1]}" 420px 560px 520px 300px >/dev/null \
        || { fail "genie: the window did not move at super+g, so nothing was tested"; return; }
    if judge same "${oracle[1]}" "${folder[1]}" 0 0 1 1 1 >/dev/null; then
        pass "genie: the folder's genie equals the Rust genie within 1"
    else
        fail "genie: the folder's genie differs from the Rust genie"
    fi
}

# xray  a translucent terminal with the wallpaper blurred behind it, the
#       first FX3 effect (a T2 slot reading xray through the memo), and the
#       wallpaper outside it untouched; the xray built once and its memo run
#       once while a player keeps the passes coming, and on every pass with
#       SOLIUM_XRAY_REBUILD=always. A moving wallpaper rendered once a pass
#       with the rule, as without it, and at effects.xray.scale 0.5 too,
#       where a still one is still built once. And a plate there from the
#       start, its backdrop already blurred in the first frame drawn (spec
#       C14: nothing pops in after it), the first frame's gate held three
#       passes by SOLIUM_WARM_INJECT=3, which draw only the clear colour.
section_xray() {
    local name
    for name in xray-off xray-on xray-always xray-anim-off xray-anim-on xray-half xray-half-anim; do
        local extra=()
        case "$name" in
            xray-on) extra=(BLUR=1) ;;
            xray-always) extra=(BLUR=1 SOLIUM_XRAY_REBUILD=always) ;;
            xray-anim-off) extra=(SOLIUM_CHECKER_PHASE=1) ;;
            xray-anim-on) extra=(BLUR=1 SOLIUM_CHECKER_PHASE=1) ;;
            xray-half) extra=(BLUR=1 XRAY_SCALE=0.5) ;;
            xray-half-anim) extra=(BLUR=1 XRAY_SCALE=0.5 SOLIUM_CHECKER_PHASE=1) ;;
        esac
        # A burst of two, because a single frame is written to the bare path
        # and not beside it, where `wait_frames` counts; the first is judged.
        SECTION_EFFECTS="$root/crates/solium/effects/blur" SECTION_QML="$here/checker.qml $here/veil.qml" \
            run_scene "$name" xray.lua "${extra[@]}" SOLIUM_TRACE="$out/$name/trace.jsonl" \
            SOLIUM_CAPTURE_AT=6000 SOLIUM_CAPTURE_FRAMES=2 SOLIUM_CAPTURE_INTERVAL=100 || return
        translucent_kitty effects-check-xray
        sleep 1.5
        player effects-check-player
        wait_frames 2 || return
        stop
    done
    local off on half each
    off="$(ls "$out/xray-off"/f-* | head -1)"
    on="$(ls "$out/xray-on"/f-* | head -1)"
    half="$(ls "$out/xray-half"/f-* | head -1)"
    for each in "$off" "$on" "$half"; do
        [[ "$(judge size "$each")" = "1600 900" ]] \
            || { fail "xray: $each is $(judge size "$each"), not 1600x900, so nothing here can be judged"; return; }
    done
    # Gradient energy (`judge sharpness`), not `edges`: the checker's 64 px
    # cells keep their plateaus through the blur, and `edges` sums each
    # widened step to its height. Inside the terminal below its line of text,
    # which is drawn sharp over the blur.
    local sharp soft
    sharp="$(judge sharpness "$off" 80px 130px 560px 310px)"
    soft="$(judge sharpness "$on" 80px 130px 560px 310px)"
    if (( soft * 2 < sharp )); then
        pass "xray: the wallpaper behind the terminal fell from $sharp to $soft in gradient energy"
    else
        fail "xray: nothing blurred behind the terminal ($sharp -> $soft)"
    fi
    judge same "$off" "$on" 700px 500px 400px 200px >/dev/null \
        && pass "xray: the wallpaper outside the terminal is untouched" \
        || fail "xray: the blur reached past the terminal"
    # A still wallpaper is captured into the xray once and kept: the
    # capture is keyed on the scripted surface's element, so a wallpaper
    # scene that were re-made each pass would be built every pass.
    local passes builds runs forced
    read -r passes builds < <(judge after "$out/xray-on/trace.jsonl" xray_builds)
    read -r _ runs < <(judge after "$out/xray-on/trace.jsonl" memo_runs)
    passes="${passes:-0}" builds="${builds:-0}" runs="${runs:-0}"
    (( builds == 1 && runs == 1 && passes >= 100 )) \
        && pass "xray: built once and blurred once in $passes passes" \
        || fail "xray: built $builds times and blurred $runs times in $passes passes"
    read -r passes forced < <(judge after "$out/xray-always/trace.jsonl" xray_builds)
    passes="${passes:-0}" forced="${forced:-0}"
    (( passes >= 100 && forced >= passes - 2 )) \
        && pass "xray: SOLIUM_XRAY_REBUILD=always built it on $forced of $passes passes" \
        || fail "xray: the control built it on $forced of $passes passes, so the count above proves nothing"
    # The scenes phase renders a hosted scene at the size and scale
    # `elements` asks for, and the xray draws it from that same picture,
    # scaled down at effects.xray.scale 0.5: so a moving wallpaper is
    # rendered once a pass with the rule as without it, and on nearly every
    # pass, or "at most once" would prove nothing.
    local most moved
    for each in xray-anim-off xray-anim-on xray-half-anim; do
        read -r passes most moved < <(judge most "$out/$each/trace.jsonl" rendered 3000)
        passes="${passes:-0}" most="${most:-0}" moved="${moved:-0}"
        (( passes >= 60 && most == 1 && moved * 10 >= passes * 9 )) \
            && pass "$each: the moving wallpaper rendered once a pass, on $moved of the last $passes" \
            || fail "$each: the wallpaper rendered up to $most times a pass, on $moved of the last $passes"
    done
    read -r passes builds < <(judge after "$out/xray-half/trace.jsonl" xray_builds)
    passes="${passes:-0}" builds="${builds:-0}"
    (( builds == 1 && passes >= 100 )) \
        && pass "xray: at effects.xray.scale 0.5 a still wallpaper built once in $passes passes" \
        || fail "xray: at effects.xray.scale 0.5 a still wallpaper built $builds times in $passes passes"
    soft="$(judge sharpness "$half" 80px 130px 560px 310px)"
    if (( soft * 2 < sharp )); then
        pass "xray: at effects.xray.scale 0.5 the wallpaper behind the terminal fell from $sharp to $soft"
    else
        fail "xray: at effects.xray.scale 0.5 nothing blurred behind the terminal ($sharp -> $soft)"
    fi

    # The first frames drawn, a plate on `top` from the start, every frame
    # captured from the first, one a pass (SOLIUM_CAPTURE_INTERVAL=0). With
    # nothing held (`-off`, no rule) the first is drawn. With the blur behind
    # the plate, nested every pass is ready at once, so SOLIUM_WARM_INJECT=3
    # (the control, Ruling F50) holds the first frame's gate three passes as
    # if it were not: those three frames are only the clear colour, prepare
    # setting `Prepared.held` and `elements` drawing nothing for it, and the
    # fourth, the first drawn, has the plate's backdrop blurred already.
    for name in xray-first-off xray-first-on; do
        local extra=(VEIL=1)
        [[ "$name" = xray-first-on ]] && extra+=(BLUR=1 SOLIUM_WARM_INJECT=3)
        SECTION_EFFECTS="$root/crates/solium/effects/blur" SECTION_QML="$here/checker.qml $here/veil.qml" \
            run_scene "$name" xray.lua "${extra[@]}" \
            SOLIUM_CAPTURE_AT=0 SOLIUM_CAPTURE_FRAMES=5 SOLIUM_CAPTURE_INTERVAL=0 || return
        player effects-check-player
        wait_frames 5 || return
        stop
    done
    local first_off first_on
    first_off="$(judge first_lit "$out/xray-first-off")"
    first_on="$(judge first_lit "$out/xray-first-on")"
    if [[ -z "$first_off" || -z "$first_on" ]]; then
        fail "xray: no frame of the first five drawn showed anything (off: '$first_off', on: '$first_on')"
        return
    fi
    for each in "$first_off" "$first_on"; do
        [[ "$(judge size "$each")" = "1600 900" ]] \
            || { fail "xray: $each is $(judge size "$each"), not 1600x900, so nothing here can be judged"; return; }
    done
    [[ "$(basename "$first_off")" = f-000 ]] \
        && pass "xray: with nothing to wait for, the first frame drawn is the desktop" \
        || fail "xray: with nothing to wait for, the first frame drawn is not the desktop (first lit: $(basename "$first_off"))"
    [[ "$(basename "$first_on")" = f-003 ]] \
        && pass "xray: SOLIUM_WARM_INJECT=3 held three frames to the clear colour and drew the fourth" \
        || fail "xray: under SOLIUM_WARM_INJECT=3 the first frame drawn is $(basename "$first_on"), not f-003"
    sharp="$(judge sharpness "$first_off" 100px 460px 700px 340px)"
    soft="$(judge sharpness "$first_on" 100px 460px 700px 340px)"
    if (( soft * 2 < sharp )); then
        pass "xray: the first frame drawn ($(basename "$first_on")) has the plate's backdrop blurred, $sharp -> $soft"
    else
        fail "xray: the first frame drawn ($(basename "$first_on")) has the plate's backdrop sharp, $sharp -> $soft"
    fi
}

# motion  a window's backdrop read where it is drawn (Task 54), over bars no
#         two places of which look alike: held away from its layout place
#         (super+p), at six moments of a 600 ms glide (800 px, 1.3 px a
#         millisecond, so a walk drawn even a few milliseconds after its
#         places were measured is pixels off the bars), and half way
#         through a genie, every pixel of the window is the wallpaper
#         blurred where that pixel is (the reference: the wallpaper blurred
#         everywhere), within 6 for 95 % of them. The genie run again with
#         warped = rest must fail the same judge. A plain blur runs no
#         effect pass while it moves; blur-tint's tint, its step per part,
#         runs while the window glides and not at rest.
section_motion() {
    local name
    local fixtures="$root/crates/solium/tests/fixtures/effects"
    for name in sharp ref held glide suffix genie genie-rest; do
        local extra=() trigger="" at=6000 frames=2 interval=100
        case "$name" in
            sharp) extra=(NORULE=1) ;;
            ref) extra=(REF=1) ;;
            held) trigger=4500:super+p ;;
            glide) extra=(GLIDE_MS=600) trigger=4500:super+m at=4560 frames=6 interval=80 ;;
            suffix) extra=(EFFECT=blur-tint) trigger=4500:super+m at=9000 ;;
            genie) trigger=4500:super+g ;;
            genie-rest) extra=(WARPED=rest) trigger=4500:super+g ;;
        esac
        SECTION_EFFECTS="$root/crates/solium/effects/blur $fixtures/blur-tint $fixtures/kawase $fixtures/tint" \
            SECTION_QML="$here/bars.qml" \
            run_scene "motion-$name" motion.lua "${extra[@]}" \
            SOLIUM_TRACE="$out/motion-$name/trace.jsonl" ${trigger:+SOLIUM_TRIGGER_AT=$trigger} \
            SOLIUM_CAPTURE_AT="$at" SOLIUM_CAPTURE_FRAMES="$frames" SOLIUM_CAPTURE_INTERVAL="$interval" || return
        clear_kitty effects-check-clear
        wait_frames "$frames" || return
        stop
    done
    local sharp ref each
    sharp="$(ls "$out/motion-sharp"/f-* | head -1)"
    ref="$(ls "$out/motion-ref"/f-* | head -1)"
    for each in "$out"/motion-*/f-*; do
        [[ "$(judge size "$each")" = "1600 900" ]] \
            || { fail "motion: $each is $(judge size "$each"), not 1600x900, so nothing here can be judged"; return; }
    done
    local said
    said="$(judge live "$sharp" "$(ls "$out/motion-held"/f-* | head -1)" "$ref")" \
        && pass "motion: held away from its place, the window shows the backdrop where it is drawn ($said)" \
        || fail "motion: held away from its place, the window shows another place's backdrop ($said)"
    local frame moments=0
    for frame in "$out/motion-glide"/f-*; do
        judge differs "$sharp" "$frame" 0 0 1 1 >/dev/null || continue
        if said="$(judge live "$sharp" "$frame" "$ref")"; then
            moments=$((moments + 1))
            echo "    $(basename "$frame"): $said"
        else
            fail "motion: gliding, $(basename "$frame") shows another place's backdrop ($said)"
        fi
    done
    (( moments == 6 )) && pass "motion: gliding, all six moments show the backdrop where the window is" \
        || fail "motion: gliding, $moments of six moments were right"
    said="$(judge live "$sharp" "$(ls "$out/motion-genie"/f-* | head -1)" "$ref")" \
        && pass "motion: half way through a genie, the funnel shows the backdrop under each of its pixels ($said)" \
        || fail "motion: half way through a genie, the funnel shows another place's backdrop ($said)"
    said="$(judge live "$sharp" "$(ls "$out/motion-genie-rest"/f-* | head -1)" "$ref")" \
        && fail "motion: warped = rest passed the judge, so the genie's pass proves nothing ($said)" \
        || pass "motion: warped = rest bends the resting backdrop, and the judge sees it ($said)"
    local runs
    read -r _ runs < <(judge trace "$out/motion-glide/trace.jsonl" effect_runs)
    runs="${runs:-0}"
    (( runs == 0 )) && pass "motion: a plain blur ran no effect pass while it moved" \
        || fail "motion: a plain blur ran $runs effect passes while it moved"
    read -r _ runs < <(judge trace "$out/motion-suffix/trace.jsonl" effect_runs)
    runs="${runs:-0}"
    (( runs >= 30 )) && judge quiet "$out/motion-suffix/trace.jsonl" effect_runs 1000 >/dev/null \
        && pass "motion: blur-tint's tint re-ran $runs times while it glided, and not at rest" \
        || fail "motion: blur-tint's tint ran $runs times, or ran at rest"
}

# regions   where `sol.anchors` says each square is, against where the
#           capture shows it.
section_regions() {
    SECTION_QML="$here/regions.qml" run_scene regions regions.lua \
        SOLIUM_TRIGGER_AT=3000:super+a \
        SOLIUM_CAPTURE_AT=3500 SOLIUM_CAPTURE_FRAMES=2 SOLIUM_CAPTURE_INTERVAL=100 || return
    wait_frames 2 || return
    local frame
    frame="$(ls "$dir"/f-* | head -1)"
    local colour name rgb listed seen
    for colour in "red 255 0 0" "row/green 0 255 0" "blue 0 0 255"; do
        name="${colour%% *}"
        rgb="${colour#* }"
        listed="$(grep -oE "anchor probe/$name [^ ]+ [-0-9.]+ [-0-9.]+ [-0-9.]+ [-0-9.]+ true" "$dir/log" | head -1 | cut -d' ' -f4-7)"
        [[ -n "$listed" ]] || { fail "regions: sol.anchors listed no shown probe/$name; see $dir/log"; continue; }
        # shellcheck disable=SC2086
        seen="$(judge box "$frame" $rgb)" || { fail "regions: probe/$name is not in the capture"; continue; }
        if python3 - "$listed" "$seen" <<'PY'
import sys
listed = [float(v) for v in sys.argv[1].split()]
seen = [float(v) for v in sys.argv[2].split()]
sys.exit(0 if all(abs(a - b) <= 1 for a, b in zip(listed, seen)) else 1)
PY
        then
            pass "regions: probe/$name listed at $listed, drawn at $seen"
        else
            fail "regions: probe/$name listed at $listed but drawn at $seen"
        fi
    done
}

all=(overlay t0 blur none fail tilt genie xray motion regions)
sections=("$@")
[[ ${#sections[@]} -gt 0 ]] || sections=("${all[@]}")

"$root/dev/pulse-control.sh" >"$out/pulse.log" 2>&1 \
    || { cat "$out/pulse.log"; echo "effects-check: the pulse control failed: the renderer is frozen, and nothing here would prove anything"; exit 1; }
for section in "${sections[@]}"; do
    declare -F "section_$section" >/dev/null || { fail "no section called $section"; continue; }
    echo "== $section"
    "section_$section"
    stop
done
echo "effects-check: $failures failure(s); captures in $out"
exit "$((failures > 0))"
