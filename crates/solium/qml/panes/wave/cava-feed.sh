#!/usr/bin/env bash
#
# Feeds SOLIUM_PANE=wave from cava. Nothing in the compositor knows about
# audio: this writes numbers to a file and the style polls it.
#
#     crates/solium/qml/panes/wave/cava-feed.sh &
#     QML_XHR_ALLOW_FILE_READ=1 SOLIUM_PANE=wave ./target/debug/solium --tty
#
# QML_XHR_ALLOW_FILE_READ is Qt's, not Solium's: reading a local file from QML
# is off by default and has to be asked for. Without it the style still runs --
# it falls back to a travelling sine -- and Qt says why in the log.
#
# Writes a FILE rather than handing over cava's FIFO, because reading a FIFO
# blocks until a writer appears, and the thread that would block is the one the
# compositor draws every window on. Written to a temporary and renamed, so the
# style never reads a half-written line.
set -u

out="${SOLIUM_AUDIO_FILE:-/tmp/solium-audio}"
bars="${SOLIUM_AUDIO_BARS:-56}"

command -v cava >/dev/null 2>&1 || {
    echo "cava is not installed; the wave style will use its sine fallback" >&2
    exit 1
}

config=$(mktemp)
trap 'rm -f "$config" "$out.tmp"' EXIT

# `mono` so the border shows one spectrum rather than two half ones, and
# monstercat smoothing because raw bars flicker faster than an eye reads as
# movement. The eq lifts the treble, which is otherwise so far below the bass
# that most of the border never moves.
cat > "$config" <<CONF
[general]
mode = normal
framerate = 30
bars = $bars

[output]
method = raw
raw_target = /dev/stdout
data_format = ascii
ascii_max_range = 100
channels = mono

[smoothing]
monstercat = 1.5
noise_reduction = 35

[eq]
1 = 1
2 = 1.2
3 = 1.5
4 = 1.8
5 = 2.2
CONF

cava -p "$config" | while IFS= read -r line; do
    printf '%s' "$line" > "$out.tmp" && mv "$out.tmp" "$out"
done
