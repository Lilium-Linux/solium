# Ricing Solium

Everything below is a file you write. Nothing here needs the compositor
rebuilt, and nothing needs you to copy the files that ship in order to change
one thing in them.

Two commands are worth knowing first:

    solium --check          # would my configuration run? which bindings survived?
    super+shift+r           # read it again, in the running session

`--check` is the one that saves afternoons. A configuration that fails to load
is reported with the file and the line, and the running session keeps whatever
it already had — so a typo costs a line of output rather than your windows.

## The thirty-second version

Write `~/.config/solium/user.lua` with only what you want changed:

```lua
return {
    gap = 4,
    decoration = "reactive",
    tiling = { split = 0.618 },
}
```

It is merged over the defaults, key by key, through nested tables — so
`tiling = { split = ... }` keeps the animations underneath it. Lists are
replaced whole, because a list of column widths with one entry changed is a
different list, not a longer one.

`crates/solium/lua/config.lua` is the list of everything you can put in there.

Three guides go deeper than the recipes below:

| | |
|---|---|
| [modes.md](modes.md) | what a desktop mode is, and how to write one |
| [animation.md](animation.md) | the animation engine, curves, and where the feel lives |
| [decorations.md](decorations.md) | window frames, the loading window, the pointer |

## Where things live

| what | where |
|---|---|
| your settings | `~/.config/solium/user.lua` |
| your bindings and layout | `~/.config/solium/init.lua` |
| one module, replaced | `~/.config/solium/tiling.lua`, `scrolling.lua`, … |
| your decorations | `~/.config/solium/qml/decorations/*.qml` |
| your loading window | `~/.config/solium/qml/loading/*.qml` |
| your colours and fonts | `~/.config/solium/qml/Solium/Theme.qml` |

Your directory is searched first in every case. A file you write shadows the
one that ships, and everything you did not write still comes from the shipped
set — including its later improvements.

## Recipes

### A different frame

```lua
return { decoration = "left" }
```

`top`, `left`, `bottom`, `border`, `reactive`, `proximity`, `reveal`, `pulse`.
Reload and every open window is re-framed.

### Your own frame

Copy one you like into `~/.config/solium/qml/decorations/` and edit it. A file
named `top.qml` there shadows the shipped `top.qml`, so you can keep using
`decoration = "top"` and mean yours. `crates/solium/qml/decorations/README.md`
is the contract: what a frame is told, what it can ask for, and what it
reserves.

### What a window shows before its application exists

A window's life starts when you ask for the application, not when the program
gets around to connecting — it takes its place in the layout immediately, the
other windows move aside, and the application appears *inside* it. What is in
it meanwhile is yours:

```lua
return {
    loading = {
        scene = "window",       -- qml/loading/window.qml, or a path
        patience = 8000,        -- give up on it after this long, in ms
        reserves_a_slot = true, -- take a place in the layout straight away
        decorated = false,      -- draw the titlebar while it waits
        fade = 180,             -- how long the scene takes to dissolve, in ms
    },
}
```

`reserves_a_slot = false` is the quieter reading: the other windows only move
aside once the application is really there. `decorated = true` gives the
waiting window a titlebar, and therefore a close button for an application
that is not coming.

`fade` is the dissolve as the application appears underneath. It is the
compositor's, not the scene's, and a scene **cannot** do it for itself — Qt's
software renderer repaints only what it thinks changed, onto the pixels already
there, so each half-transparent frame would land on its own opaque previous one
and nothing would fade. Set `fade = 0` to cut straight to the application.

Your own scene goes in `~/.config/solium/qml/loading/`. It is handed the
program's name and how long it has waited; the one that ships uses only the
name, on the grounds that the window already *is* the window and the one fact
you do not otherwise have is which application you are waiting for.

    SOLIUM_LOADING=mine        # ~/.config/solium/qml/loading/mine.qml
    SOLIUM_LOADING=~/mine.qml  # anywhere

### Your monitors

The compositor cannot work out which screen is on the left. The kernel reports
connectors in an order that has nothing to do with your desk, so it guesses —
every connected screen driven, left to right in that order — and about half the
time the guess is wrong.

You almost never want coordinates. Say what is beside what:

```lua
return {
    monitors = {
        { name = "DP-1", mode = "2560x1440@260", vrr = true, primary = true },
        { name = "DP-2", mode = "2560x1440@75", right_of = "DP-1", align = "end" },
        { name = "DP-3", above = "DP-1", transform = "90" },
        { name = "HDMI-A-1", enabled = false },
    },
}
```

`solium --probe` prints the connector names this machine has, without taking
the screen from whatever is drawing on it. A name nothing answers to gets a
line in the log rather than being ignored — a monitor arrangement that silently
does nothing is the hardest kind to debug, and the usual cause is a name that
does not exist here.

| key | |
|---|---|
| `right_of`, `left_of`, `above`, `below` | beside another monitor, by name. A chain resolves whatever order you write the list in, and no monitor in it needs coordinates |
| `align` | `"start"`, `"centre"` (default) or `"end"` — how the *other* axis lines up against something taller or wider |
| `x`, `y` | the top-left corner outright, if you would rather |
| `mode` | `"2560x1440@165"`. The refresh is optional; so is the whole thing — see below |
| `vrr` | variable refresh rate, where the monitor and driver offer it |
| `transform` | `"90"`, `"180"`, `"270"`, `"normal"`, or the same with `flipped-` |
| `enabled = false` | do not drive it |
| `primary = true` | where a dock, a bar, or any layer surface that named no output goes |
| `scale` | read, reported in the log, **not honoured yet** — [#39](https://github.com/Lilium-Linux/solium/issues/39) |

`super+shift+r` applies a change without ending the session.

**Refresh rate lives in `mode`.** `"2560x1440@165"`, the way every display tool
on Linux writes one. Drop the `@` part and you get the fastest mode at that
resolution; drop `mode` entirely and you get the fastest mode at the
*preferred* resolution, which is almost always what you want. Three words also
work:

| | |
|---|---|
| `"best"` | highest refresh at the preferred resolution — the default |
| `"preferred"` | exactly what the monitor's EDID says, refresh included |
| `"widest"` | the largest resolution, fastest at that size |

`"best"` is not `"preferred"`, and the difference is the reason it is the
default: the EDID's preferred *flag* names a resolution and usually pairs it
with a pedestrian 60 Hz. A 260 Hz panel reports `2560x1440@60` as preferred.
Taking that literally drives a fast display slowly and makes every animation in
the compositor look worse than it is. `"preferred"` is there for a monitor that
is unstable at its fastest rate, which is a real thing and not something the
automatic choice can know about.

A mode the monitor does not have warns and falls back rather than going black.
`solium --probe` lists every mode each monitor offers, in exactly the form
`mode` takes — which is the whole reason to run it:

```
DP-1         connected, 36 modes, best 2560x1440@260
               2560x1440@260, 240, 200, 165, 144, 120, 100, 60  (preferred)
               1920x1080@240, 120, 60, 50
```

**`vrr` is off unless asked for.** FreeSync, G-Sync compatible, Adaptive-Sync —
the display's refresh follows what is being drawn instead of the other way
round, which is what removes tearing and stutter on anything that cannot hold a
steady frame rate. It is not on by default because it interacts badly with some
panels at low frame rates — visible flicker — and that is not something to turn
on for somebody. The log says whether the connector actually offered it.

Three more are worth a sentence.

**`align` is not cosmetic.** A 1080p beside a 1440p leaves 360 rows belonging
to no screen at all, and which end of the small monitor that dead strip is at
decides where the pointer catches on its way past. `"centre"` halves the strip
instead of putting all of it at one end, which is why it is the default;
`"end"` lines the bottom edges up, which is what you want if both monitors
stand on the same desk.

**`enabled = false` frees a CRTC**, not just the screen. A CRTC is the hardware
that scans a buffer out, there are usually four, and a connector without one
stays dark — so switching off a monitor you are not using is how you drive a
fourth on a card with three.

**A rotated monitor's work area is portrait.** `transform` changes the logical
size, so every layout follows it without knowing anything about rotation, and
the display pipeline does the turning.

Everything is applied at startup. A monitor plugged in while the session is
running is not picked up yet —
[#43](https://github.com/Lilium-Linux/solium/issues/43).

### One workspace per screen, or one for the desk

Each monitor has its own workspace in view by default: `super+2` switches the
screen the pointer is on and leaves the other showing what it was, so a
reference on the second monitor stays put while you move around on the first.

One line makes a workspace a whole desk instead, so a switch moves every screen
together:

```lua
return { workspaces = { per_monitor = false } }
```

Neither is more correct — the difference is whether you think of your monitors
as two screens or as one surface you happen to have cut in half.

### Bars, docks and wallpapers

They are ordinary clients over `wlr-layer-shell`, which means any panel already
written for that protocol works. A surface names the output it wants and the
compositor honours it, so a bar on every screen is one surface per screen, each
reserving from *that* monitor's work area. One that names no output gets the
primary monitor.

See **[shell-boundary.md](shell-boundary.md)** for why that is a client rather
than something the compositor draws, and what was learned from it being the
other way round.

### Your own colours

Copy `Solium/Theme.qml` into `~/.config/solium/qml/Solium/` and change it.
Every frame and every shell surface reads it, so one file restyles the desktop
rather than the titlebars.

### Your own animation feel

Named curves — `linear`, `outCubic`, `outBack`, `inOutQuad`, `spring` — or four
numbers, which are a cubic bezier's control points:

```lua
return {
    tiling = { motion = { duration = 220, easing = { 0.34, 1.56, 0.64, 1 } } },
}
```

Those are the same four numbers CSS calls `cubic-bezier` and every easing
generator on the internet hands out, so a feel you found elsewhere transfers
directly. y may leave 0..1 — that is what overshoot is.

### Your own bindings

`~/.config/solium/init.lua` replaces the entry point. It can still `require`
everything that ships, so starting from the shipped one and adding to it is
three lines:

```lua
require("modes")
require("tiling")

sol.bind("super+return", function() sol.spawn("kitty") end)
```

`solium --check` prints every binding it registered, which is how you find out
that an edit dropped one.

### Your own mode

A mode is a Lua module that reacts to events — `open`, `close`, `focus`,
`drop`, `resize`, `scroll` — and asks for placements. `tiling.lua` is Hyprland's
dwindle in about a hundred lines; `scrolling.lua` is niri's model. Copy either
into your own directory and it takes over.

**[modes.md](modes.md)** is the guide, with a whole working mode in forty lines
and the two mistakes everyone makes first.

## Worth knowing

A decoration is rasterised in software, over the window's whole outer rect.
Bars and borders are cheap because most of that rect is untouched, but a frame
that paints across the entire window every frame will cost you — the animation
only runs while the scene is actually changing, so favour transitions that
settle over ones that loop forever.

Frames stop being driven a few identical renders after they stop moving, so an
idle window costs a comparison rather than a rasterisation. A decoration that
animates continuously (`pulse` does, while focused) opts out of that for as
long as it animates.
