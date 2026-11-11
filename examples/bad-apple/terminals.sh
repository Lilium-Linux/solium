#!/bin/sh
# Open the terminals `bad-apple` plays its film with: `count` foot windows
# of one theme, from one foot server, each with a shell and its prompt.
#
#     terminals.sh light|dark <count>
#
# The window manager runs this twice, once for each theme, and knows the
# windows by their application id: `bad-apple-light` and `bad-apple-dark`.
# It begins the film once every one of them is titled `ready`, which the
# shell's prompt does as it is printed.
#
# To play the film with another terminal, this is the one file to change:
# open `count` windows with that application id, each showing a prompt and
# titled `ready` once it does. It has to be a terminal that draws a new size
# as soon as it is given one, as foot does. One that waits for a resize to
# be over before it draws, as kitty does, is a size behind for the whole
# film.
set -eu
theme=$1
count=$2
case "$theme" in
    light) background=ffffff; foreground=111111 ;;
    dark) background=000000; foreground=eeeeee ;;
    *) echo "usage: terminals.sh light|dark <count>" >&2; exit 2 ;;
esac

# The prompt: escapes that clear the window and title it `ready`, and a
# `$`. A shell draws its prompt again when its terminal is resized, and a
# terminal wraps its lines again at every new width, so a plain prompt is
# soon somewhere down its window with copies of itself above it. One that
# clears the window first is always alone in the top-left corner.
prompt='\[\e[H\e[2J\e]2;ready\a\]\$ '

# One server for all the windows of a theme: a process for each would be
# a hundred processes. Nobody's own foot.ini: the film is the same on every
# machine. `resize-by-cells=no`: a window is the size it is given, to the
# pixel, not the nearest whole character under it. `resize-delay-ms=0`: the
# shell is told a new size at once, and draws its prompt for it. And no
# scrollback, so a window that grows has no old lines to bring back.
socket="${XDG_RUNTIME_DIR:-/tmp}/bad-apple-$theme-$$.sock"
foot --server="$socket" --config=/dev/null --log-no-syslog \
    -o "colors-dark.background=$background" -o "colors-dark.foreground=$foreground" \
    -o font=monospace:size=8 -o pad=2x2 -o resize-by-cells=no \
    -o resize-delay-ms=0 -o scrollback.lines=0 \
    -o initial-window-size-pixels=160x120 &
server=$!
trap 'kill "$server" 2>/dev/null; rm -f "$socket"' EXIT

# The server is ready once its socket is there.
tries=0
while [ ! -S "$socket" ]; do
    tries=$((tries + 1))
    [ "$tries" -le 100 ] || { echo "terminals.sh: foot did not start" >&2; exit 1; }
    sleep 0.05
done

i=0
while [ "$i" -lt "$count" ]; do
    footclient --server-socket="$socket" --no-wait --app-id="bad-apple-$theme" \
        env "PS1=$prompt" bash --norc --noprofile
    i=$((i + 1))
done

# Until the session ends, which ends the server.
wait "$server"
