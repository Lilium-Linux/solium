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

### Where your monitors are

The compositor cannot work out which screen is on the left. The kernel reports
connectors in an order that has nothing to do with your desk, so it guesses —
left to right in that order, top edges aligned — and about half the time the
guess is wrong. Fixing it is one line per screen:

```lua
return {
    monitors = {
        { name = "DP-1", x = 0, y = 0 },
        { name = "DP-2", x = 2560, y = 180 },
    },
}
```

`solium --probe` prints the connector names this machine has, without taking
the screen away from whatever is currently drawing on it. A name nothing
answers to gets a line in the log rather than being ignored — a monitor
arrangement that silently does nothing is the hardest kind to debug, and the
usual cause is a connector name that does not exist here.

Positions are top-left corners in one **global space** that every monitor is a
window onto. So `y` is how much lower one screen sits than the other, which is
what a monitor standing on a different-height desk actually needs, and a
monitor you do not name goes to the right of everything you did — plugging in a
third does not land it on top of one of the other two.

`super+shift+r` applies a change without ending the session.

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
