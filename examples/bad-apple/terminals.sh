#!/bin/sh
# Open the terminals `bad-apple` plays its film with: `count` kitty windows
# of one theme, from one kitty process, each with a shell and its prompt.
#
#     terminals.sh light|dark <count>
#
# The window manager runs this twice, once for each theme, and knows the
# windows by their application id: `bad-apple-light` and `bad-apple-dark`.
# It begins the film once every one of them is titled `ready`, which the
# shell's prompt does as it is printed. To play the film with another
# terminal, this is the one file to change: open `count` windows with that
# application id, each showing a prompt and titled `ready` once it does.
set -eu
theme=$1
count=$2
case "$theme" in
    light) background=#ffffff; foreground=#111111 ;;
    dark) background=#000000; foreground=#eeeeee ;;
    *) echo "usage: terminals.sh light|dark <count>" >&2; exit 2 ;;
esac

# The prompt: an escape that titles the window `ready`, and a `$`.
prompt='\[\e]2;ready\a\]\$ '

# A kitty session: one window, and a new window for each of the rest.
session=$(mktemp)
trap 'rm -f "$session"' EXIT
i=0
while [ "$i" -lt "$count" ]; do
    [ "$i" -gt 0 ] && echo "new_os_window" >> "$session"
    printf '%s\n' "launch env PS1='$prompt' bash --norc --noprofile" >> "$session"
    i=$((i + 1))
done

# Nobody's own kitty.conf: the film is the same on every machine. And
# `sync_to_monitor=no`: a terminal draws its prompt when the shell prints it,
# not when the compositor next asks it for a frame, which with a few hundred
# windows in one process is seconds later.
kitty --config NONE --class "bad-apple-$theme" --session "$session" \
    -o "background=$background" -o "foreground=$foreground" -o "cursor=$foreground" \
    -o cursor_blink_interval=0 -o font_size=9 -o window_padding_width=3 \
    -o placement_strategy=top-left -o remember_window_size=no \
    -o initial_window_width=160 -o initial_window_height=120 \
    -o confirm_os_window_close=0 -o enable_audio_bell=no -o update_check_interval=0 \
    -o sync_to_monitor=no
