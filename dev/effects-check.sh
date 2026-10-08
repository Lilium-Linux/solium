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
# solium/effects/, as a user's would be. Sets $dir and $socket. The capture
# knobs come in the extra environment.
run_scene() {
    local name="$1" lua="$2"
    shift 2
    dir="$out/$name"
    mkdir -p "$dir/config/solium/effects"
    cp "$here/$lua" "$dir/init.lua"
    local folder
    for folder in ${SECTION_EFFECTS:-}; do
        cp -R "$folder" "$dir/config/solium/effects/"
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

all=(overlay t0 blur none fail tilt)
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
