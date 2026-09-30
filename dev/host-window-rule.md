# Keeping the nested window out of the way

Developing a compositor means running it as a window on the machine you are
using. That window should not decide where it goes or take focus from whatever
is already running.

Solium's nested window sets `app_id = solium-nested` for exactly this, so the
host compositor's own rules can place it. On KDE (KWin 6) the rule below puts it
on a chosen monitor, sets focus-stealing prevention to *Extreme*, so it does not
take focus on its own, and passes keyboard shortcuts through to it while it is
active. It is focused when you activate it — a click, Alt+Tab, the task bar —
or when nothing else is active; see the last section.

`~/.config/kwinrulesrc`:

```ini
[<uuid>]
Description=Solium nested — opens on one monitor, movable, no focus steal, shortcuts passed through
wmclass=solium-nested
wmclassmatch=2
wmclasscomplete=false
position=<x>,<y>
positionrule=3
fsplevel=4
fsplevelrule=2
disableglobalshortcuts=true
disableglobalshortcutsrule=2
```

`wmclassmatch=2` is a substring match, so it also matches any app_id that
merely contains `solium-nested`; `1` is an exact match.

`position` is in the global layout, so the coordinates select the monitor:
replace `<x>,<y>` with a point inside the target output's geometry, as reported
by `kscreen-doctor -o`.
Apply without logging out:

```sh
qdbus-qt6 org.kde.KWin /KWin reconfigure
```

(`qdbus-qt6` is Fedora's name for the tool; Arch calls it `qdbus6`.)

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
INFO solium::winit: nested window is on this host monitor monitor="<connector>" x=<x> y=<y>
```

`x` and `y` are that host monitor's own origin, not the window's position, and
in physical pixels, so on a scaled monitor they will not match the logical
geometry `kscreen-doctor` prints. The monitor is reported a few frames in, not
at startup — a Wayland client learns its output only when the host sends
`wl_surface.enter` — and only once, so the line says where the window opened,
not where it was dragged afterwards. A window across two monitors names one of
them.

## Shortcuts

Nearly every default binding is on Super (`super+return`, `super+1` to
`super+9`, `super+shift+r` and the rest), and a desktop such as Plasma has
global shortcuts of its own on Meta, which the host takes first. Without the
`disableglobalshortcuts` lines a key pressed in the nested window may never
reach Solium. Scripted tests do not notice, because `SOLIUM_TRIGGER_AT` fires
bindings inside the compositor and never goes through the host; only testing by
hand does. `Force` (`2`) is right
for this rule, unlike the position rule: KWin suspends its global shortcuts
only while the nested window is the active one.

## What focus prevention does and does not cover

`fsplevel=4` stops the nested window taking focus from whatever you are using.
It does **not** guarantee the window never receives focus: KWin allows
activation when there is no other active window, since then there is nothing to
steal from. So a run started while nothing is focused will show

```
INFO solium::winit: nested window focus changed focused=true
```

and that is the rule working as designed, not failing.
