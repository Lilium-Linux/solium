# Keeping the nested window out of the way

Developing a compositor means running it as a window on the machine you are
using. That window should not decide where it goes or take focus from whatever
is already running.

Solium's nested window sets `app_id = solium-nested` for exactly this, so the
host compositor's own rules can place it. On KDE (KWin 6) the rule below puts it
on a chosen monitor and sets focus-stealing prevention to *Extreme*, so it opens
in the background and is focused only by clicking it.

`~/.config/kwinrulesrc`:

```ini
[<uuid>]
Description=Solium nested — opens on one monitor, movable, no focus steal
wmclass=solium-nested
wmclassmatch=2
wmclasscomplete=false
position=80,1560
positionrule=3
fsplevel=4
fsplevelrule=2
```

`position` is in the global layout, so the coordinates select the monitor: pick
a point inside the target output's geometry as reported by `kscreen-doctor -o`.
Apply without logging out:

```sh
qdbus-qt6 org.kde.KWin /KWin reconfigure
```

### `positionrule=3`, not `2`

This one matters and is not obvious. KWin's rule types are numbers:

| Value | Meaning |
|---|---|
| 2 | **Force** — KWin keeps the window there, permanently |
| 3 | **Apply Initially** — KWin places it there once, then leaves it alone |

`Force` is the wrong one, and it fails in a way that reads as a compositor bug:
the window opens where you asked and then **cannot be dragged at all**, because
every move is immediately overridden. Nothing logs, nothing errors, the
titlebar just does not work. Use `3` — the point of the rule is to decide where
the window *opens*, not to nail it down.

Solium logs which host monitor it landed on, so the rule can be checked rather
than assumed:

```
INFO solium::winit: nested window is on this host monitor monitor="DP-2" x=0 y=0
```

The monitor is reported a few frames in, not at startup — a Wayland client
learns its output only when the host sends `wl_surface.enter`.

## What focus prevention does and does not cover

`fsplevel=4` stops the nested window taking focus from whatever you are using.
It does **not** guarantee the window never receives focus: KWin allows
activation when there is no other active window, since then there is nothing to
steal from. So a run started while nothing is focused will show

```
INFO solium::winit: nested window focus changed focused=true
```

and that is the rule working as designed, not failing.
