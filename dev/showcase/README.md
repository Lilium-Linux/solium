# The showcase, recorded again with one command

Every picture and video on the README and on the book's
[Showcase](../../docs/showcase.md) page is made by `record.py` from the shots
in `shots/`. After a change to the desktop, run it again; nothing is drawn
by hand, so nothing goes stale.

```sh
cargo build --release
dev/showcase/record.py                 # every shot
dev/showcase/record.py alt-tab glass   # just these
dev/showcase/record.py --list
```

It needs a host Wayland session, ffmpeg with libx264 and libwebp, Python's
Pillow and dbus-python, kitty, Firefox, and for the `foot` shots the build
image (`dev/foot`). Run one at a time: each shot is a nested Solium with real
applications in it.

## What a run does

For each shot it:

1. copies the shipped default configuration
   (`crates/solium/default-config`) into a scratch `XDG_CONFIG_HOME`, and
   applies the shot's `patch` and `files` on top: what is recorded is what
   ships, plus exactly the lines the shot shows;
2. makes a scratch home from `stage/home` (a prompt with no user or host in
   it, the kitty theme, nano's settings, a throwaway Firefox profile), so no
   name, file or account of the machine's appears;
3. starts a private D-Bus with `stage/logind.py` on it. Solium serves
   notifications, the tray and media players there (`SOLIUM_SESSION_BUS`),
   and hears `Lock` and `Unlock` from the stand-in (`SOLIUM_LOGIND_BUS`), so
   an unlock is captured without a password;
4. runs Solium nested with the timeline as its knobs (`SOLIUM_KEY_AT`,
   `SOLIUM_TRIGGER_AT`, `SOLIUM_MOVE_AT`, `SOLIUM_DRAG_AT`, ...), opens the
   applications in it, and captures a burst of frames, one per frame drawn;
5. makes the media into `docs/media/`, a contact sheet of every frame into
   `target/showcase/review/`, and checks the frames.

Every frame is the compositor's own. A crop is a crop; a comparison is
several shots side by side with their labels in a margin under each, never
on the picture.

## What it checks

- **Late frames.** Each frame is logged with its moment on the compositor's
  clock, and the video is paced by those moments, so it plays at true speed
  whatever the capture cost. A frame more than 26 ms after the one before is
  named.
- **Black frames, and pops:** something appearing, vanishing or moving at
  once in a moment that is otherwise still. A skip in the middle of a motion
  is not seen this way; looking at the frames is what finds those.
- **Loops.** A shot with `loop = true` must end on the picture it began on,
  so a loop never jumps where it repeats.
- **What changed.** Each still is compared with the one committed; the ones
  that differ are named, which after a change to the desktop is the list of
  pictures to look at.

Look at the contact sheet of every shot you keep, frame by frame.

## A shot

```toml
title = "Alt+Tab"
what = "Every window as a live card in a row, the chosen one big in the middle."
keys = "alt+tab, tab, enter"

[stage]
apps = ["notes", "log", "docs"]   # opened once Solium is up
patch = '''
config.pane = "left"              -- Lua, run on the table user.lua returns
'''
files = ["docs/showcase/examples/lock/Iris.qml:qml/lock/Iris.qml"]
scale = 2                         # the nested monitor's scale, for close-ups

[[at]]
ms = 3000
key = "alt+tab"                   # a real key, by keysym names

[[at]]
ms = 3600
move = "800,420"                  # the pointer travels there...
over = 500                        # ...in 500 ms, a step each frame

[capture]
from = 2600                       # ms after Solium starts
to = 5200
loop = true

[out]
webp = 960                        # an animated WebP this wide, for the README
poster = 1400                     # a PNG of the frame this far into the capture
slow = 4                          # and a copy four times slower
```

### Timeline events

Each `[[at]]` has an `ms`, on the compositor's clock from its start, and one
of these:

| Key | What happens |
|---|---|
| `key = "super+t"`, `times`, `every` | a key combination pressed through the real input path, `times` times `every` ms apart |
| `type = "pane = \"left\","` | typed a key at a time, `every` ms apart (70) |
| `trigger = "super+tab"` | the binding run, without a key |
| `move = "x,y"`, `over`, `from` | the pointer travels to `x,y` in `over` ms, from `from` or from where it is |
| `drag = "x,y"`, `over`, `from`, `hold` | the same with the left button down, and the keys in `hold` held |
| `click = "x,y"`, `button` | a press and release at `x,y`; `button = "right"` for the right button |
| `notify = "title"`, `body`, `from_app` | a notification, sent on the private bus |
| `lock = true`, `unlock = true` | logind's `Lock` or `Unlock`, from the stand-in |
| `app = "docs"` | an application opened then |
| `run = "command"` | a command run then, inside the session |
| `outputs = 2` | the nested backend's monitors, now |
| `loading = "program"` | a loading window for an application that never arrives |

Coordinates are logical pixels on the nested monitor, 1600 by 900 at scale
1 (800 by 450 at scale 2).

### Applications

`notes` (kitty, nano on the configuration's `user.lua`), `log` (kitty, the
repository's history), `shell` (kitty), `foot` (foot from the build image),
`docs` (Firefox on the documentation). Any other string is run as a shell
command. `{ app = "notes", size = [900, 560] }` sizes a kitty window.

### Outputs

| Key | Written to `docs/media/` |
|---|---|
| `mp4 = true` (the default) | `<shot>.mp4`, 60 frames a second, true speed |
| `webp = 960` | `<shot>.webp`, animated, that wide, within `webp_kb` (3072) |
| `poster = ms`, `still = true` | `<shot>.png` |
| `slow = 4` | `<shot>-x4.mp4` |
| `filmstrip = 6` | `<shot>-strip.png`, six frames side by side |
| `crop = [x, y, w, h]` | every output cut to that, in logical pixels |
| `follow = { size = [w, h] }` | a crop of that size centred on the pointer's path |

### A grid

A shot with a `[grid]` instead of `[capture]` sets recorded shots side by
side, started together, each labelled under its picture:

```toml
title = "Minimise, four ways"
[grid]
cells = ["minimize-genie", "minimize-spring", "minimize-fold", "minimize-dissolve"]
labels = ["genie (ships)", "genie, spread 3, spring", "fold", "dissolve"]
cols = 2
suffix = "-x4"          # the cells' slow copies
[out]
webp = 960
```
