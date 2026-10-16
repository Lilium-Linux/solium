# Ricing Solium

Everything below is a file you write. Nothing here needs the compositor
rebuilt, and nothing needs you to copy the files that ship in order to change
one thing in them.

Start with [the first configuration](#the-thirty-second-version) and
[file locations](#where-things-live). Use the
[generated settings reference](https://lilium-linux.github.io/solium/generated/reference/settings.html)
for a specific option, [modes](modes.md) for window management,
[decorations](decorations.md) for frames, and
[shell hosting](shell-boundary.md) for your own desktop scenes.

Two commands are worth knowing first:

    solium --check          # would my configuration run? which bindings survived? do its scenes build?
    super+shift+r           # read it again, in the running session

`--check` is the one that saves afternoons. A configuration that fails to load
is reported with the file and the line, and the running session keeps whatever
it already had — so a typo costs a line of output rather than your windows.

It also reports the quieter failure: a setting the defaults do not define.
`tilling = { split = 0.6 }` is perfectly good Lua and merges in perfectly
cleanly, and then nothing ever reads it — so `--check` names it, suggests what
you probably meant, and exits non-zero.

```
  2 unrecognised setting(s) -- written, merged, and read by nothing:
    open.scal                   did you mean open.scale?
    tilling                     did you mean tiling?
```

Then it builds every scene the configuration declares when it loads (the
wallpaper, the shell, any `sol.surface` of your own) on one monitor, draws each
once, and loads every pane style in your own `qml/panes/`, every layer
included, with the pane style and loading scene `SOLIUM_PANE` and
`SOLIUM_LOADING` name when they are set. A scene that fails to build, misses a
required property or prints a QML warning while it is built fails the check
too. It builds them in software, so a style that needs the GPU is named and
not built, and a scene declared later, by a handler, is not seen.

Three things it cannot see, so that its silence means something: the contents
of a list (`monitors` entries are replaced whole and not descended into), the
keys inside `keyboard`, `cursor` and `bindings` — the first two are checked
against a list, the third accepts anything you can press — and a value of the
wrong *type*, since `gap = "12"` is a recognised key.

## The thirty-second version

**Two files, and the difference matters.** `user.lua` is *settings*: a table of
what you want changed, merged over the defaults, and everything you do not
mention keeps shipping. Bindings are settings too — there is a `bindings`
section, so adding a key does not cost you the second file. `init.lua` is the
*entry point* — writing one replaces the shipped configuration entirely,
layouts and modes included, and is what you want only when you are rewriting
the session rather than adjusting it.

Almost everything on this page is `user.lua`.


Write `~/.config/solium/user.lua` with only what you want changed:

```lua
return {
    gap = 4,
    pane = "reactive",
    tiling = { split = 0.618 },
}
```

It is merged over the defaults, key by key, through nested tables — so
`tiling = { split = ... }` keeps the animations underneath it. Lists are
replaced whole, because a list of column widths with one entry changed is a
different list, not a longer one.

[Every setting](https://lilium-linux.github.io/solium/generated/reference/settings.html)
is the list of everything you can put in there, one row per section, and
[the configuration reference](../crates/solium/lua/config.lua) explains each
key. Both are made from `config.lua`, the file that sets the defaults, so they
cannot drift from it. [Key bindings](https://lilium-linux.github.io/solium/generated/reference/bindings.html)
is every key that ships, and what it does.

Three guides go deeper than the recipes below:

| | |
|---|---|
| [modes.md](modes.md) | what a desktop mode is, and how to write one |
| [animation.md](animation.md) | the animation engine, curves, and where the feel lives |
| [decorations.md](decorations.md) | window frames, the loading window, the pointer |

## Where things live

| what | where |
|---|---|
| your settings, bindings included | `~/.config/solium/user.lua` |
| a whole session of your own | `~/.config/solium/init.lua` — replaces the shipped entry point |
| one module, replaced | `~/.config/solium/tiling.lua`, `scrolling.lua`, … |
| your pane styles | `~/.config/solium/qml/panes/<name>/` |
| your loading window | `~/.config/solium/qml/loading/*.qml` |
| your pointer | any QML file, named by `cursor.scene` ([below](#your-own-pointer)) |
| your colours and fonts | `~/.config/solium/qml/Solium/Theme.qml`, once [#88](https://github.com/Lilium-Linux/solium/issues/88) is fixed |
| the shipped files, to copy from | `crates/solium/qml` and `crates/solium/lua` in a checkout; `share/solium/qml` and `share/solium/lua` under the prefix of an install: `/usr` for the Fedora package, `~/.local` for `dev/install.sh` |
| the log | `~/.local/state/solium/session.log`, for a session started with `solium --tty` or from the login screen. A nested run logs to the terminal it was started from |

`~/.config` is `$XDG_CONFIG_HOME` when that is set, and `~/.local/state` is
`$XDG_STATE_HOME`.

For the files you write, your directory is searched first in every case but
your colours. A file you write shadows the one that ships, and everything you
did not write still comes from the shipped set — including its later
improvements. Your own `Theme.qml` does not shadow the shipped one yet: the
shipped `Solium` module is found first
([#88](https://github.com/Lilium-Linux/solium/issues/88)). Don't edit the
shipped files in place: an install replaces them, and `user.lua` does the same
job without being overwritten.

## Recipes

### A different frame

```lua
return { pane = "left" }
```

Twelve styles ship, and
[the pane styles' README](../crates/solium/qml/panes/README.md#what-is-here)
lists them with what each one is for. The default is `glass`: each window and
its bar one rounded piece of dark liquid glass, the bar along the bottom, and
a thin dark shade just inside the rounded edge of a window that does not have
the keyboard ([its numbers](../crates/solium/qml/panes/README.md#glass)).
`pane = "top"` is the opaque titlebar above the window that was the default
before it. Reload and every open window is re-framed.

### No frame at all

```lua
return { pane = "none" }
```

No bar, no border, and no QML scene built per window — which is different from
a decoration that draws nothing: there is no scene to rasterise, so an
undecorated desktop costs nothing per window. For a tiling layout whose own bar
makes a titlebar redundant, or for taste. `SOLIUM_PANE=none` does it for
one run.

### Your own frame

Copy one you like into `~/.config/solium/qml/panes/` and edit it. A folder
named `top` there shadows the shipped `top`, so you can keep using
`pane = "top"` and mean yours. `crates/solium/qml/panes/README.md` is the
contract: what a style declares, what each layer is told, what it can ask for,
and what it costs.

A style is a folder holding a `Pane.qml` — what the frame reserves, and a list
of layers, each its own QML scene at its own depth: behind the client, in the
frame, or above it. A layer can also `bleed` past the window's edge, which is
how a border waves or a shadow reaches. A single QML file still works too and
is still called a decoration; it lives in
`~/.config/solium/qml/decorations/` and is one layer in the frame.

A layer is also told where the focused text field's caret is, as `caret`, in
the pane's own space; whatever the configuration hands every pane with
`sol.pane_values{ key = value }`, as `values`, which a delegated layer
declares as `property var values: ({})`; its window's id, process and state
(`windowId`, `pid`, `maximized`, `fullscreen`, `floating`, `minimized`), and
what the configuration handed that window alone with
`sol.frame(id, { key = value })`, as `windowValues`; and it can read the
keyboard's layout and locks as the `Keyboard` singleton. That is how `KeyboardPillLayer {}` draws
the keyboard pill: the panes README's
[What the compositor sets on every layer](../crates/solium/qml/panes/README.md#what-the-compositor-sets-on-every-layer)
has each of them.

### A titlebar that does things

A frame is QML, so a titlebar can do whatever its QML does, and talk to your
configuration both ways (#417). A button sets `action` to `"close"`,
`"maximize"` or `"minimize"`; anything else is
`Solium.send("name", { ... })`, which your Lua hears as
`sol.on("frame", function(id, name, data) ... end)`, and answers with
`sol.frame(id, { ... })`. A right press on the titlebar is the right button to
its QML, the wheel over it reaches a `WheelHandler`, and a panel dropped down
over the window, quick settings for one, holds the pointer and the keys while
it is open with a `Grab`, as a shell's popup does: a press outside it, or
Escape if you handle it, closes it. The panes README,
[Sending to Lua, and a panel over the window](../crates/solium/qml/panes/README.md#sending-to-lua-and-a-panel-over-the-window),
has the whole of it.

A titlebar can also grow and push the window's content down, rather than drop
a panel over it: a layer's `growTop`, animated with a `Behavior`, makes the
frame reserve that much more at the top for this window alone. A floating
window grows taller with its content moved down at its size; a tiled or
maximised one keeps its size and its application is given that much less
height, once, when the value stops moving. The grown band is the frame's, so
what you draw there is pressed and the application hears nothing of it. The
panes README,
[A titlebar that grows](../crates/solium/qml/panes/README.md#a-titlebar-that-grows),
has the details.

### Rounded corners

```qml
// Pane.qml
client.radius: 12
```

A style may round the client's own corners. The compositor cuts them as it
draws the client: each of the window's surfaces goes through a fragment program
where it already is, so rounding buys no pass at all and no copy of the window,
and a window that is tilted or pulled by a genie keeps its corners.
`client.radius: 0` is no effect at all rather than a radius of nothing, so a
style that does not want it draws its client exactly as before, and nine of the
twelve shipped bundles do not want it.

A rounded window that fades in or out is faded surface by surface, with no
picture of it taken: where a subsurface overlaps its parent (a player's video
over its window, say) the two show through each other while the window is
translucent. An opaque window looks as it always did. A fullscreen window has
no frame, so it is drawn square; a maximised one keeps its corners.

`pane = "rounded"` is one that does, and is there to be looked at. Two things
in it are worth copying. The radius is in **logical** pixels — the compositor
multiplies by the monitor's scale, so the corner is the same size on a HiDPI
screen rather than half of it. And every layer of the style is told the number
as `clientRadius`, so a bar or a border can hug the same curve:

```qml
// Frame.qml
property int clientRadius: 0   // written by the compositor
radius: clientRadius           // or clientRadius + 2, to hug it from outside
```

That split is the whole design: the compositor rounds the client, because those
pixels belong to the application and only a shader can mask them, and QML rounds
itself, because `Rectangle.radius` is free. A style that wants a rounded border
around a *square* client sets only its own `radius`, and its client is drawn as
an unstyled one is.

#### One corner at a time

Each corner can be given its own radius. Each of the four defaults to `radius`,
so one key still means all four and nothing written before these existed
changes:

| key | |
|---|---|
| `client.radius` | all four corners, and the default the four below fall back to |
| `client.radiusTopLeft` | that corner alone |
| `client.radiusTopRight` | |
| `client.radiusBottomLeft` | |
| `client.radiusBottomRight` | |

```qml
// Pane.qml — square on top, round underneath
client.radius: 12
client.radiusTopLeft: 0
client.radiusTopRight: 0
```

A `0` here has to be written out. Squaring a corner is half of what these keys
are for, so a corner that is *absent* follows `radius` and a corner that says
`0` is square — leaving the key out is not a way of squaring it.

Every layer is told all four by name as well:

```qml
// Frame.qml
property int clientRadiusTopLeft: 0      // and TopRight, BottomLeft, BottomRight
property int clientRadius: 0             // still written: the LARGEST of the four
```

`clientRadius` survives and is the largest of the four, because what a layer
does with one number is hug the window from outside — and a hug has to clear
the biggest cut or it crosses the curve. A layer that cares about one corner
reads that corner.

#### Two ways a titlebar meets a rounded window

The shipped pair, and the reason the corners are separable at all. Both reserve
a band at the top; what differs is which corners the shader cuts and where the
bar is drawn.

| | `pane = "rounded"` | `pane = "flush"` |
|---|---|---|
| the client's top corners | rounded | square |
| what draws the window's top | the bar's own `radius` | the bar's own `radius` |
| the bar's height | `insetTop + clientRadius` | `insetTop` |
| the bar's depth | `behind` | `frame` |
| where the two meet | the bar shows through the client's cut corners | a flat seam, corner to corner |

`flush` is the simpler of the two: with the client's top squared there is
nothing to fill, so the bar stops at the seam and stays in the frame, where the
compositor copies it band by band.

`rounded` cuts all four, which leaves two notches inside the window where the
client's top corners were. Something has to be behind them or they show the
wallpaper — so its bar is `clientRadius` taller than its band and sits at
`depth: "behind"`. Under the client it fills the notches and covers nothing;
over the client the same rectangle would eat the client's top `clientRadius`
rows, which in a terminal is the top half of the first line.

### Effects

An effect is a folder of Lua and GLSL, yours in `~/.config/solium/effects/`
or one Solium ships, and a rule puts it in a slot of a part of a window. To
blur what mpv itself draws, in its own place:

```lua
return {
    effects = {
        rules = {
            { match = { app_id = "mpv" }, part = "client", slot = "replace",
              effect = { "blur", source = "self" } },
        },
    },
}
```

`part` is the window's `client`, its whole `pane`, its `popup`s, a layer of
its pane style, its `region:titlebar`, or a scripted or layer surface; `slot`
is `behind` it, in `front` of it, or `replace` it; `effect` is a name, a name
with params (`{ "blur", passes = 3 }`), a chain of them, or `false` to empty
the slot. A later rule for the same part and slot wins, and yours win over a
pane style's: a style may give its own windows rules in an `effects.lua`
beside its `Pane.qml` ([pane styles](../crates/solium/qml/panes/README.md#effectslua)). `source = "self"` is what makes this blur read mpv's own
pixels. Without it, a blur reads what is behind the window: put it `behind`
a translucent terminal and the wallpaper shows through blurred, the blur run
once per monitor and shared by every window it is behind
([effect folders](../crates/solium/effects/README.md#the-memo)):

```lua
{ match = { app_id = "kitty" }, part = "client", slot = "behind", effect = "blur" }
```

The rules are applied whole: one that cannot be, a typo in a key or an
effect with a broken shader, keeps the whole set out, leaves the rules that
were running, and is named in the top-right corner of your primary monitor,
at its rule number or at the effect's own file and line. `solium --check`
fails on it too. With no rules nothing changes, and nothing costs anything.

`blur` ships with Solium, a dual Kawase blur with `passes` and `offset`
([its folder](../crates/solium/effects/blur/README.md)); copy the folder to
`~/.config/solium/effects/blur/` to change it, and your copy is used instead.
So does `glass`, liquid glass, dark unless its `tint` says otherwise
([its folder](../crates/solium/effects/glass/README.md)), which the `glass`
pane style puts behind every window it frames; a rule of yours for
`{ style = "glass" }`, the `pane` part and its `behind` slot wins over the
style's, with other params (`tint = { 1, 1, 1, 0.11 }` is light glass) or with
`effect = false`. And `shade`, a thin dark edge just inside a part's rounded
outline ([its folder](../crates/solium/effects/shade/README.md)), which the
`glass` style puts in front of a window without the keyboard with a rule's
`fade`: the slot eases in as the rule starts to match and out as it stops,

```lua
{ match = { focused = false }, part = "pane", slot = "front", effect = "shade",
  fade = { duration = 240, easing = "outCubic" } }
```

the compositor blending the result it has at another opacity on each frame
and running nothing for it. Two rules for one slot, one with the keyboard and
one without, each with a `fade`, crossfade from one result to the other as
the keyboard moves: a window's own pixels eased between two opacities, say,
with a blur behind it that stays solid
([crossfades](../crates/solium/effects/README.md#crossfades)).
An effect that reads the part's own pixels (`self`, or `source = "self"`, as
the rule above gives the blur) is drawn in its slot, and runs again only when
the window draws something new, changes size, or the rule's params change:
a still window's effect runs once and is then placed as it was. So the rule
above costs a capture of mpv's window and the blur's six small passes (three
halvings and three doublings back, at its default `passes = 3`) each time mpv
draws a frame, and nothing while it is paused. A window the rule does not
match is untouched, drawn exactly as with no rule; `match = { title = … }`
works too, for a player whose `app_id` you do not know. If the blur fails,
on a GPU that refuses its shaders or anywhere else, mpv is drawn as if no
rule named it. An effect that reads nothing of the window, only its shape (a
border, a glow behind it), is drawn too, with no capture of the window at
all: it runs once, and again only when the window changes size or the rule's
params change, however much the window draws.
[The effects reference](../crates/solium/effects/README.md#tiers) says what
each kind of effect costs, and [its rules](../crates/solium/effects/README.md#rules)
every key a rule takes and how to write an effect of your own.

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
compositor's, not the scene's: whether a window is see-through is a
presentation transform, like where it is and how big, so the compositor applies
it to the scene's picture as it draws it ([animation.md](animation.md#animations-you-should-not-write)).
Set `fade = 0` to cut straight to the application.

Some applications show a splash first, a small window of one fixed size, and
their main window a moment later: Vesktop does. A window that names one size
and only one cannot fill the waiting window, so it does not go into it: it is
shown over the middle of the waiting window, at its own size, and the waiting
window stays where it is until the main window arrives and fills it. When the
splash goes, `patience` starts again. A fixed-size window that is the
application's own, a calculator that cannot be resized say, is on screen and
clickable at once, and becomes an ordinary window, with its frame, the keyboard
and a place in the layout, when `patience` runs out with it still there; the
waiting window goes then. The launch's activation token cannot tell the two
apart sooner: Vesktop's splash brings it too.

Your own scene goes in `~/.config/solium/qml/loading/`. It is handed the
program's name and how long it has waited; the one that ships uses only the
name, on the grounds that the window already *is* the window and the one fact
you do not otherwise have is which application you are waiting for.

    SOLIUM_LOADING=mine        # ~/.config/solium/qml/loading/mine.qml
    SOLIUM_LOADING=~/mine.qml  # anywhere

### Your wallpaper

```lua
wallpaper = "~/Pictures/whatever.png",
```

`~` is expanded, the image is cropped to fill rather than fitted, and
`wallpaper = false` turns it off — which is what you want if you run `swaybg`,
`hyprpaper`, or a shell that draws its own. A layer surface on the background
layer is drawn *over* this one, so leaving both on means paying to rasterise a
picture nobody sees.

A **list** gives each workspace its own, and it travels with that workspace:

```lua
wallpaper = { "~/Pictures/one.png", "~/Pictures/two.png" },
```

`workspaces.lua` puts each one in the same selection as its desk's windows, so
one animation carries both and the background stops being left behind when you
switch. Fewer pictures than workspaces cycles. It costs a scene per desk on
every monitor, built when it is declared, and one screen-sized rasterisation per
desk you have actually visited, per monitor — which is why one image stays one
static surface: every desk sharing a picture would make a wallpaper that slides
pixel-identical to one that does not.

The more interesting part is that **there is no wallpaper in the compositor.**
`lua/wallpaper.lua` calls one thing:

```lua
sol.surface("wallpaper", {
    scene = "wallpaper.qml",
    layer = "background",
    on    = "every-monitor",
    properties = { source = "wallpaper/solium.png" },
})
```

`sol.surface` draws any QML scene at any layer on any monitor, so a bar, a
dock, a heads-up display or a debug overlay is the same call with a different
`layer` and a different `on`:

```lua
sol.surface("clock", {
    scene = "clock.qml",
    layer = "top",
    on    = { x = 40, y = 40, w = 420, h = 90 },
    properties = { format = "HH:mm" },
})
```

`layer` is `background`, `bottom`, `top` or `overlay`, and each sits *under*
the matching wlr-layer-shell layer — so a real bar covers a scripted one, and
`swaybg` covers this wallpaper. `on` takes `every-monitor`, `primary`, a
connector name, or a rect in the global space. Each monitor it is on gets a
scene of its own, built when the surface is declared or the monitor arrives and
dropped when the monitor goes, and inside it `Solium.monitor` is that monitor.
`sol.surface(name, false)` removes one.

Declaring the same name again with the same `scene` writes the changed
`properties` into the live scene rather than building it again, so an open
popup or a running animation in it survives. A key left out of `properties`
keeps the value the scene last had, and a dotted key such as
`["panel.open"] = true` reaches a grouped property. Only a different `scene`
file builds it again.

`interactive = true` lets the pointer reach it, where its items take input:
a `MouseArea`, a control such as a `Button`, a pointer handler other than
`HoverHandler`, or an item marked `Solium.input: true`
([shell-boundary.md](shell-boundary.md#what-a-hosted-shell-is-given) has the
whole list).
Everywhere else the pointer goes to what is under it
(`state::tests::real_client::reflow_on_close::hosted::a_press_where_the_shell_draws_nothing_reaches_the_window_under_it`).
A surface without it holds neither a `Grab` nor the keyboard
(`state::tests::real_client::reflow_on_close::hosted::a_surface_the_pointer_does_not_reach_holds_no_grab`,
`state::tests::real_client::reflow_on_close::hosted::a_surface_the_pointer_does_not_reach_holds_no_keyboard`).
The scene sends an action, with data if it has any, `Solium.send("pressed",
{ id: 3 })`, and whoever is listening is told:

```lua
sol.surface("panel", { scene = "panel.qml", layer = "overlay",
                       on = area, interactive = true })

sol.on("surface", function(surface, action, data)
    if surface == "panel" then
        sol.log("pressed " .. action)
    end
end)
```

An action of the compositor's own vocabulary, `windows.focus` with `{ id:
model.id }` say, is done for you, by `lua/actions.lua` through `sol.act`
([shell-boundary.md](shell-boundary.md#what-a-hosted-shell-is-given) has the
list).

That is the whole of how the Developer Tweaks panel works, and it is entirely
in `lua/tweaks.lua` — the compositor has no idea what a tweak is.

`reserve = { bottom = 48 }` takes those edges out of the work area of every
monitor the surface is on, whatever its size, so a bar scene drawn across the
whole monitor reserves only its strip and the windows are placed above it. The
scene can say it too, `Solium.surface.reserve.bottom: 48`, which wins for each
edge it sets; [shell-boundary.md](shell-boundary.md), "Room of its own", has
how the windows re-flow when it changes
(`state::tests::real_client::reflow_on_close::hosted::a_declared_reserve_takes_its_edge_out_of_the_work_area`).

**Place a rect on the `monitors` event, not at the top of your script.**
Scripts load before the screens are known — on the hardware backend, before
the GPU is even opened — so a rect computed at load time is computed against
zeros. The event fires once when the monitors are first known, again on every
hotplug, again when a monitor changes size without one coming or going (a
nested Solium whose window is resized), and again on every reload, which is
when a placement needs redoing anyway. A surface whose `on` is `"every-monitor"`, `"primary"` or a connector
name needs no event: declare it at the top of the script, and the compositor
builds its scene on each monitor it names as that monitor arrives, as
`wallpaper.lua` and `shell.lua` do. Only a rect computed from the monitors
waits for them:

```lua
sol.on("monitors", function()
    local area = sol.monitor()   -- the active monitor's work area
    sol.surface("panel", { scene = "panel.qml", layer = "top",
                           on = { x = area.x, y = area.y, w = area.w, h = 40 } })
end)
```

`sol.monitors()` lists every monitor, with its name, its work area, its whole
rect and whether it is `primary`, for a panel on a particular one.

Copy `qml/wallpaper.qml` to `~/.config/solium/qml/wallpaper.qml` and the
background becomes whatever QML can be:

```qml
import QtQuick

Item {
    required property string source

    Rectangle {
        anchors.fill: parent
        gradient: Gradient {
            GradientStop { position: 0.0; color: "#12002e" }
            GradientStop { position: 1.0; color: "#7c4dff" }
        }
    }
}
```

`super+shift+r` reloads it without ending the session. `source` is handed in
whatever the configuration said, and a scene that ignores it — like the one
above — is perfectly valid.

### Reloading without pressing anything

Saving a file under `~/.config/solium` — `user.lua`, a module of your own, a
pane style, a loading scene, your wallpaper or your shell — reloads it on its
own, the same way `super+shift+r` does: a typo leaves the running
configuration alone and logs it, same as a manual reload.

```lua
reload = {
    automatic = false,  -- back to pressing super+shift+r yourself
    quiet_ms = 600,      -- wait longer for a slow save to settle
}
```

`automatic` defaults to true. A burst of writes from one save — an editor
that writes a temporary file and renames it over the original fires several —
is collapsed into the one reload it earns, `quiet_ms` after the last write in
it, not one reload per write. `automatic = false` watches nothing at all,
rather than merely never finishing the wait.

Developing a shell, a pane style or a titlebar as its own project — pointed
at with `SOLIUM_SHELL_SCENE`, `SOLIUM_PANE`, `SOLIUM_QML_TITLEBAR` or
`SOLIUM_LOADING` — is watched too, wherever that is, even well outside
`~/.config/solium`. The one case this does not reach is an absolute path
written directly in `config.lua` with none of those set: saving it reloads
nothing, and the log says once that it was configured outside everywhere
automatic reload watches.

### Your keyboard

```lua
keyboard = {
    layout  = "us,ua",
    options = "grp:alt_shift_toggle",
},
```

The first layout is the one you start in. `super+shift+k` cycles them and so
does Alt+Shift, because `grp:` options are implemented inside the keymap and
work without the compositor being involved — the binding exists so that the
feature is discoverable from `solium --check` rather than from knowing xkb.

Bindings follow the key, not the layout. With a Cyrillic layout active,
`super+q` is still the key `us` has Q on, and so is `super+shift+k`. A binding
you write in the other alphabet, such as `super+Cyrillic_shorti`, gets that key
first. A key the active layout already labels in ASCII keeps its own name:
Russian puts `.` where `us` has `/`, so on Russian that key is `super+period`.

`variant` takes one per layout in the same order, blank for plain:
`layout = "us,ua", variant = ",dvorak"` is ordinary US and Ukrainian Dvorak.
`options` also carries `compose:ralt`, which is the nearest thing to an input
method until [#26](https://github.com/Lilium-Linux/solium/issues/26) lands.

Leaving a name out means "whatever the session already said": an empty name
makes xkbcommon read `XKB_DEFAULT_LAYOUT` and its siblings, which is where a
display manager puts it. So a machine already configured elsewhere keeps
working, and writing it here takes precedence.

The repeat rate is the compositor's own — no environment variable reaches it,
and before this it could not be changed at all:

```lua
keyboard = {
    repeat_rate  = 40,   -- repeats per second. 25 by default
    repeat_delay = 350,  -- milliseconds before repeating starts. 200
},
```

A held key whose binding asks to repeat, such as volume up, repeats at the same
rate, and a rate of 0 stops that too.

The locks can be set too, and Num Lock on at login is the usual one:

```lua
keyboard = { num = true },
```

That turns Num Lock on when the session starts, and again at every
`super+shift+r`. `caps` works the same way for Caps Lock. Each is set by
pressing the keymap's own key inside the compositor, so the application with
the keyboard is told and the layout stays where it was.

`sol.keyboard()` reads all of it back to Lua — the layout names, which is
active and its names (`layout_name`, `Russian`; `layout_short`, `RU`), whether
Caps Lock and Num Lock are on, and the repeat settings — and
`sol.keyboard{ caps = false }` sets a lock.
`sol.on("keyboard", function(state, changed) end)` hears the layout or a lock
change, however it changed, and never ordinary typing, nor what is pressed at
the lock screen. QML reads the same as the `Keyboard` singleton, in a hosted
scene and a pane style alike. `sol.on("text_input", function(field, why) end)`
hears the focused text field: `why` is `"field"` when one is enabled or
focused, `"caret"` when its caret moves and `"framed"` when its window goes
fullscreen or comes back. A caret that moves twice before the next frame is
told once, where it ended up.

### The keyboard pill

Switch layout, or press Caps Lock, and a small capsule appears just below
where you are typing: `⇪` while Caps Lock is on, `EN` or `RU` for a moment
after a switch, after the one macOS shows. It is drawn in the theme's `accent`
with its glyph in `accentInk`, which in the shipped theme is a light grey
capsule with a dark glyph and a soft shadow: it stands out on a dark window
and on a light one.

It is not a compositor feature. It is the shipped configuration's example of
doing something with what the compositor publishes, and it is two ways of
doing the same thing, so you can see both:

- **Inside the window** (`show = "pane"`, the default). Every shipped pane
  style ends with one line, `KeyboardPillLayer {}`, a layer above the client
  that draws the pill at the pane's `caret` — where the focused text field's
  caret is, in the pane's own space. Because it is part of the pane, it moves,
  scales and fades with its window. Your own style gets it by adding the same
  line. A window drawn with no frame — fullscreen, one that draws its own
  decorations, or every window when `pane = "none"` — has no pane style
  around it, so it gets the surface below at its caret instead:
  `sol.text_input()` says which, as `framed`.
- **On a surface of its own** (`show = "surface"`): an overlay `sol.surface`
  that `lua/keyboard_indicator.lua` places at the caret in the global space,
  from `sol.text_input()`, and moves again each time `sol.on("text_input")`
  says the caret moved.

The policy is the Lua file: it listens to `sol.on("keyboard")` and
`sol.on("text_input")` and decides what shows and where. An application that
says where its caret is only after a key, as kitty does, gets the pill on its
screen for the first key and at its caret a few milliseconds later, when it
says. Kitty says so only as it handles each key, before the shell has echoed
it, so a pill that stays while you type, Caps Lock's, can sit a cell behind
the cursor until the next key. The look is QML,
`KeyboardPill` in `import Solium`, on `Theme`. The compositor provides the
data underneath — `text-input-v3` for the caret, the keyboard's state and its
events — and knows nothing about pills. These are its defaults, from
`config.lua`; any of them goes under `keyboard` in `user.lua`:

```lua
keyboard = { indicator = {
    show = "pane",            -- "pane", "surface", or false
    on = { layout = true, caps = true, num = false },
    caps_on_focus = true,     -- Caps' pill again when a field is focused with Caps on (needs on.caps)
    fallback = "surface",     -- with no caret: on the screen, or false
    position = "bottom",      -- where on the screen: "bottom", "center" or "top"
    duration = 1200,          -- ms a layout's pill stays
} }
```

The caret comes only from applications that say where it is through
`text-input-v3`; for the others, `fallback = "surface"` shows the pill on the
focused window's monitor instead. To try it nested, `SOLIUM_KEY_AT` presses
Caps Lock and a layout switch for you ([`dev/README.md`](../dev/README.md#keys-by-name)).
To change the policy, copy `lua/keyboard_indicator.lua` next to your
configuration, where it is found first. To change the on-screen pill's look,
copy `qml/indicator/keyboard.qml` to `~/.config/solium/qml/indicator/`; a
pane style of your own can draw a pill of its own at `caret` in place of
`KeyboardPillLayer {}`. `KeyboardPill` itself is in the shipped `Solium`
module, which a copy cannot replace yet
([#88](https://github.com/Lilium-Linux/solium/issues/88)). To have none,
write `keyboard = { indicator = false }` in `user.lua`; a whole `init.lua` of
your own can instead leave out `require("keyboard_indicator")`.

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
| `primary = true` | where a layer surface that named no output goes, and a hosted shell or `sol.surface` declared with `on = "primary"` |
| `scale` | device pixels per logical one. Left out, worked out from the panel |

`super+shift+r` applies the placement keys (`right_of`, `left_of`, `above`,
`below`, `align`, `x`, `y`), `scale`, `primary` and `enabled` without ending
the session. `mode`, `vrr` and `transform` are read when a monitor is first
lit, so on one that is already lit they wait until it is next plugged in —
or until a reload with `enabled = false` and another with it back.

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
  DP-1         connected, 36 modes, best 2560x1440@260, 600x340mm, 108 dpi, scale 1
                 2560x1440@260, 240, 200, 165, 144, 120, 100, 60  (preferred)
                 1920x1080@240, 120, 60, 50
```

**`scale` is worked out for you, and you can override it.** Left out, it comes
from the panel's own size: 2x above 192 dpi and 1x below, which is the number
GNOME and KDE both use — matching them matters more than being right in the
abstract, because it is what every monitor's marketing and every forum answer
is implicitly calibrated against. That puts a 13" 4K laptop at 2x and a 27" 4K
at 1x, and the second of those is genuinely a matter of taste, which is why it
is settable. Fractional values work.

`solium --probe` prints the dpi it measured and the scale it would choose, per
monitor, at the end of each monitor's first line above. That is the one number
you need to decide whether to disagree. A monitor that reports no physical size
says so there, and gets scale 1.

Scaling is not a zoom. Everything the compositor draws itself — titlebars, the
loading window, the pointer — is *rasterised* at the monitor's pixel count
rather than drawn small and stretched, so a 2x screen gets twice the detail and
not twice the blur. Your QML keeps working unchanged: it is laid out in logical
units, so `titlebarHeight: 32` is 32 logical pixels on every monitor and is
drawn with as many real pixels as that monitor has.

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

**Plugging in and unplugging work while the session runs.** A monitor that
arrives is driven with its entry above, as it would have been at startup, and
the `monitors` event fires so anything placed per screen can be placed again.
One that goes takes no windows with it: a window left on no screen is moved
onto one that remains, and the layouts run again.
[dev/README.md](../dev/README.md#hotplug-and-how-to-test-it-without-a-cable)
has how to try it without reaching behind the desk.

### Turning screens off

The screens go dark on their own after ten minutes with nobody at the machine,
with no daemon to install. It is one number:

```lua
return { idle = { screens_off_after = 300 } }   -- seconds; 0 never does it
```

The session is locked before that, after five minutes (`idle.lock_after`, see
"Locking and sleep" below), so the screen that goes dark is the lock screen.

Ten minutes and not GNOME's five, because GNOME dims first and Solium does not:
here the first you know of it is the screen going dark. Anything that plays --
a film, a call, a presentation -- holds an idle inhibitor, and while that window
is on screen the timer waits. When it lets go the ten minutes start again from
then, so a film ending an hour in does not take the screen with it, and the
next one in a playlist does not start in the dark. Nothing holds it off behind
the lock screen, which goes dark like anything else.

**Browsers ask over D-Bus instead.** Chrome and Firefox keep the screen on
during a film by calling `org.freedesktop.ScreenSaver.Inhibit` on the session
bus, not through the Wayland protocol, so Solium owns that name while it runs,
and what they hold counts like a window's inhibitor: the timer waits, and so do
`swayidle`'s timeouts. It goes when the browser lets go, closes or crashes, and
holds nothing behind the lock screen. Firefox asks `org.freedesktop.ScreenSaver`
first for a tab in view, before the portal. A program that asks the portal's
`Inhibit` reaches the same place through `xdg-desktop-portal-gtk`, which
`lilium-portals.conf` gives `Inhibit` to, as long as no GNOME session runs on
the same bus; KDE's backend sends it to Plasma's power manager, which is not
there, so do not give `Inhibit` to `kde`. If another desktop already owns the
name on the same bus -- one running on another VT -- it keeps it: Solium says
so in its log and carries on without it. Nested, the name is owned only on the
bus `SOLIUM_SESSION_BUS` names. To leave it alone altogether:

```lua
return { idle = { dbus_inhibit = false } }
```

**Any key, click, scroll, touch or pointer motion turns every screen back on**,
however they went off, and that input is delivered as usual rather than
swallowed. The case that decides it is typing a password at a lock screen that
went dark: the first key is part of the password, and a compositor that ate it
would fail the unlock for a reason nobody could see. A key *release* does not
wake anything, so a binding that turns the screens off leaves them off when you
let go of it.

A monitor that is off is **not gone**. It keeps its place in the arrangement,
its work area and its windows; nothing is moved, no client is told a screen
went away, and `sol.monitors()` still lists it, with `power = "off"`. That is
the difference from `enabled = false`, which takes a monitor out and frees its
CRTC.

From a binding -- none ships, since which key does this is a matter of taste:

```lua
sol.bind("super+F12", function() sol.monitor_power("all", "off") end)
sol.bind("super+shift+F12", function() sol.monitor_power("DP-2", "off") end)
```

**Or leave it to `swayidle`.** Solium speaks `wlr-output-power-management`, so
`wlopm` works, and `swayidle` drives it without needing `swaymsg`. Turn the
built-in blank and lock off so the two do not both do it:

```lua
return { idle = { screens_off_after = 0, lock_after = 0 } }
```

```sh
swayidle -w \
    timeout 300 'swaylock -f' \
    timeout 600 'wlopm --off \*' resume 'wlopm --on \*' \
    before-sleep 'swaylock -f'
```

`wlopm` on its own lists the monitors and whether each is on.

A window on a screen that is off is still told it may draw, once a second
(`idle.off_frame_interval`, in milliseconds). Not telling it at all would freeze
any program that waits for that inside its swap for as long as the screen is
dark, which with the idle blank is all night. `0` stops them altogether, which
is what sway does. A screenshot of a screen that is off is refused rather than
left waiting.

Nested, there is no display to power off: a monitor that is "off" is drawn
black and then not drawn at all, so all of the above can be tried in a window.

### Touch

A finger operates the compositor's frame buttons and hosted QML controls.
The shipped `touch.lua` chooses desktop touch behaviour: touching a titlebar
away from its controls moves the window, and dragging a border resizes a
floating window or moves a tiled seam. Each action is configurable in
`config.touch = { focus = true, move = true, resize = true, click = true }`.
Set individual options to `false` in `user.lua`, or omit `require("touch")`
from a custom entry point. Without Lua bindings, there is no touch move,
resize, focus or mode click, regardless of `SOLIUM_FORM_FACTOR`.

Each finger keeps its selected target until release or cancel, even outside
that target's bounds. Mouse capture is independent. QML receives real touch
events for `TapHandler`, `DragHandler` and `PinchHandler`; Qt Quick supplies
mouse synthesis for `MouseArea` controls. Scene grabs use their usual outside
click policy, and lock-screen touches belong exclusively to the lock.
One resize owns the shared resize pipeline at a time. Further requested
resizes are swallowed until it ends; ordinary input continues.

Touchscreen output selection is tracked by
[#42](https://github.com/Lilium-Linux/solium/issues/42): its
`input.touchscreen.output` setting selects a connector, with per-device overrides
in `input.devices`. Without an override, that work uses libinput's `WL_OUTPUT`
binding, then the first connected output. It must be present in the build to
validate a touchscreen with an external monitor; these UI changes alone do
not establish hardware mapping accuracy.

A release can carry its measured velocity into a physical spring:

```lua
sol.animate({ easing = { spring = {
    stiffness = 300, damping = 25, mass = 1,
    initial_velocity = velocity / remaining_distance,
} } })
-- Queue the target presentation here. Guard zero remaining distance in Lua.
```

Velocity is in **remaining-distance fractions per second**, with its sign
preserved. Springs use physical settling time rather than the requested duration.
For a normalized gesture progress `p` settling to `target`, use
`progress_velocity / (target - p)`. The solver derives acceleration from spring
force and damping. One spring controls one progress track; it does not supply
independent velocities for every rectangle component.

### Touch events in Lua

`sol.on("touch", function(event) end)` receives each finger. **Down runs
before delivery**, outside the seat's input locks, so Lua can choose ownership.
`kind` is `"down"`, `"motion"`, `"up"` or `"cancel"`; `slot` identifies the finger
until it lifts or is cancelled. `x` and `y` are global logical coordinates,
retaining the last position for up/cancel. `output` is the connector containing
the original down, or empty outside all outputs. `time` is the backend's
32-bit timestamp in milliseconds, including its normal wraparound.

`t` is the same moment in seconds, monotonic and from the kernel's microsecond
stamp, and `vx` and `vy` are the finger's velocity in pixels a second, measured
by Solium: a least-squares fit over the last
`input.touchscreen.velocity_window_ms` milliseconds (80 by default). Use them
for a release's throw rather than dividing positions by `time`: some
touchscreens stamp in coarse steps, and dividing by those gives speeds that
jump. A finger that stops before it lifts reads near zero.

`target` describes the original hit: `"client"`, `"frame"`, `"border"`, `"scene"`,
`"script"` (a Lua input grab), or `"blocked"`. `window` is the window ID when
present, `control` tells whether the frame's QML claims the point, and `edges`
names a border: `"top"`, `"bottom"`, `"left"`, `"right"`, or a corner such as
`"bottom_right"`. These hit fields remain the same throughout the stream.

```lua
-- Example custom policy: capture a client finger and handle its stream in Lua.
sol.on("touch", function(event)
    if event.kind == "down" and event.target == "client" then
        sol.touch(event.slot, { action = "capture" })
    end
    -- Feed captured motion/up/cancel to your own Lua interaction here.
end)
```

`sol.touch(slot, options)` accepts `action = "capture"`, `"move"` or `"resize"`.
Move and resize require `window`; resize also requires `edges`. These are
operations, with no implicit focus: call `sol.focus(event.window)` if wanted.
The first accepted ownership request wins. Only the currently dispatching down
can be claimed; requests for stale slots or from motion/up are ignored. Capture
keeps the whole stream away from clients and QML. Without a claim, normal Qt
or client delivery continues. Calling `sol.grab_input(true)` alone does not
turn touch into a click: the desktop module explicitly calls
`sol.click(event.x, event.y)` to dispatch existing click listeners.

Nothing is exposed while locked or replayed on unlock. These primitives support
Lua policy without choosing a mobile gesture vocabulary in Rust. Recognition,
competition and claims after client delivery are tracked by
[#252](https://github.com/Lilium-Linux/solium/issues/252).

### Touchpad, mouse and keyboard settings

A touchpad taps and scrolls naturally out of the box; a mouse does not get
natural scroll, because a wheel is not a surface your fingers are on. Nothing
else is decided for you: acceleration, left-handed, disable-while-typing and
the rest are libinput's own defaults until `input` says otherwise, by device
type:

```lua
return {
    input = {
        touchpad = { tap = false },        -- you don't want tap-to-click
        mouse = { accel_profile = "flat" }, -- raw speed, no acceleration curve
    },
}
```

A setting here applies to every device of that type. One device on its own,
by name (a case-insensitive substring of what `libinput list-devices` calls
it) or by `vendor`/`product` (the same tool's "Vendor"/"Product", or `lsusb`'s):

```lua
return {
    input = {
        devices = {
            -- An external touchpad that should act like a mouse.
            { name = "SynPS/2 Synaptics", tap = false, natural_scroll = false },
            -- A gaming mouse, slowed down, by usb id.
            { vendor = 0x046d, product = 0xc52b, accel_speed = -0.3 },
        },
    },
}
```

Applied when the device is plugged in and again on every reload, so editing
this and pressing `super+shift+r` is enough -- no replug, no restart. The
running compositor logs what each device got, name and all. Every option is
on `sol.InputOptions` in editor completion.

Bind a touchscreen to its monitor with `input.touchscreen = { output = "eDP-1" }`,
or a tablet with `input.tablet_tool = { output = "DP-2" }`. Use the monitor's
connector name from `sol.monitors()`. A `devices` entry such as
`{ name = "ELAN Touchscreen", output = "eDP-1" }` overrides the type's binding
for that panel. When `output` is unset, Solium uses libinput's udev `WL_OUTPUT`
binding; when the selected connector is absent or no binding exists, it uses
the first screen. Removing the setting on reload restores the device's own
binding. This setting affects absolute positions only; mice and touchpads
ignore it.

**This replaced `SOLIUM_FORM_FACTOR`'s natural scroll.** The environment
variable used to guess "a laptop has a touchpad" for the whole machine at
once; a device's own type now answers that directly, so a desktop with a USB
touchpad gets natural scroll too. `SOLIUM_FORM_FACTOR` still picks
focus-follows-mouse and the drag modifier (#159).

Nested, none of this reaches anything: there is no libinput device in a
window inside another compositor, only the host's own pointer and keyboard.

### Locking and sleep

Solium locks the session with a lock screen of its own (the next section),
with nothing to install. Five things lock it, and all of them the same way:

- `super+ctrl+l` (`super+l` is focus-right, with `h`, `j` and `k`);
- `sol.lock_now()`, from a binding or a script of your own. It only ever
  locks: nothing in Lua unlocks;
- five minutes with nobody at the machine, `idle.lock_after`, held off like
  the screens going dark by a film or a call that holds an idle inhibitor;
- logind's `Lock`, which `loginctl lock-session`, a power menu that locks that
  way, and a lid switch logind handles itself all send;
- the machine going to sleep (below).

```lua
return { idle = { lock_after = 900 } }   -- seconds; 0 never does it
```

A second lock over a session already locked changes nothing. Only the
password, checked by PAM, unlocks it, or `loginctl unlock-session` from
another console or over SSH, which is the way out when PAM cannot answer. On
a machine where whoever can reach logind should not be able to unlock your
session, turn that off and leave only the password
(`logind::tests::with_logind_unlock_off_an_unlock_signal_is_ignored`):

```lua
return { lock = { logind_unlock = false } }
```

The lock screen then names `loginctl terminate-session` as the way out
instead, which ends the session rather than unlocking it.

**A locker of your own instead.** Name it, and it is what all five run:

```lua
return { lock = { command = "swaylock -f" } }
```

Split on whitespace, with no quoting, like an autostart `Exec=` line. It runs
as a client of this compositor, so it asks for `ext_session_lock_v1` the same
way any other lock screen would, and a second ask while it is already locked
runs nothing more — the second locker would only be told `finished` and exit.
Setting `command` turns Solium's own lock screen off; `native = true` beside it
keeps Solium's, and `native = false` with no `command` locks nothing at all.

**Before suspending, Solium holds sleep for the lock.** Closing the lid or
`systemctl suspend`-ing locks the session and holds a logind delay inhibitor
until the lock is on every screen, Solium's own or the locker's, or five
seconds pass (`InhibitDelayMaxUSec`, logind's own limit — not Solium's to
raise), so the machine never shows the desktop again on waking. Turn it off,
say because `swayidle`'s own `before-sleep` already does this:

```lua
return { lock = { command = "swaylock -f", before_sleep = false } }
```

`swayidle` keeps working exactly as before either way — the two only overlap
in that the lock screen may be asked for twice, and a second ask is refused as
above. Set `idle.lock_after` to 0 when `swayidle` locks on its own timeout.

`loginctl show-session $XDG_SESSION_ID -p LockedHint` reads `yes` or `no`
following whatever is actually locked, for a greeter or `loginctl` itself to
read.

### Your own lock screen

Solium has a lock screen of its own (#281), drawn by a QML scene on every
monitor, with the password checked by `solium-auth`, a small helper that runs
PAM's `solium` service. The scene shows things and nothing more: the password
never reaches it, and nothing in it can unlock. It is the session's lock
unless `lock.command` names another, and the settings below decide what it
shows.

```lua
return {
    lock = {
        scene = "Mine",          -- qml/lock/Mine.qml, yours first, then Solium's
        prompt_on = "pointer",   -- or "primary", or a monitor's name
        enter_ms = 350,          -- the entry over the desktop, at most 600
        exit_ms = 450,           -- the exit over the desktop, at most 900
    },
}
```

`scene` is a path (`"~/my-lock.qml"`), or a name looked for as
`qml/lock/<name>.qml` in `~/.config/solium/qml/` and then in the shipped QML
(`lock_scene::tests::a_bare_name_is_looked_for_in_the_users_directory_first`).
The default, `"Lock"`, is the shipped one, `crates/solium/qml/lock/Lock.qml`:
the date and a big clock over the dimmed wallpaper, the user and the password
field on one monitor, and on a touchscreen a swipe up that follows the finger
to bring up a passcode pad. All of it is one sheet, as on an iPhone or an
iPad: it comes down from the top over the desktop as the session locks, and
goes back up and off the top as it unlocks. Copy it to
`~/.config/solium/qml/lock/Lock.qml` and it is the one used.

`native` says whether Solium's own lock is the session's at all: true unless
`command` names a locker. While it is, each monitor's scene is built as the
session starts and kept between locks, so the first locked frame is already
the scene, and while you work it is kept quiet: drawn by nothing, its Timers
and animations stopped
(`lock_scene::tests::a_kept_scene_neither_ticks_nor_wakes_while_unlocked`).
Run yours only while `Lock.locked`, as the example below does: a Timer or a
loop Qt runs for you, in a `Behavior` or a `Transition`, is not stopped for
you.

A scene that is missing, will not load, or fails while it runs is replaced by
the shipped one, and that by a fallback compiled into Solium, never by the
desktop, and the log says which and why
(`lock_scene::tests::a_broken_scene_falls_back_to_the_shipped_one_and_then_to_the_fallback`,
`lock_scene::tests::a_scene_that_fails_at_run_time_is_swapped_down_the_chain`).
A scene you mend is tried again at the next lock.

A lock scene has the same `import Solium` as any other, and adds:

- `Lock`, read-only, an attached object any item reads: `state`
  (`"peeking"`, `"locking"`, `"idle"`, `"prompting"`, `"checking"`,
  `"failed"`, `"lockedOut"`, `"unavailable"`, `"unlocking"`), `mode`
  (`"pin"` while a PIN is taken, `"password"` otherwise; the next section),
  `locked`,
  `progress` (0 to 1, below), `attempts`, `retryAfter` (seconds, while
  locked out or retrying), `message`, `user` (`name`, `realName`,
  `avatar`), `promptOn` (the monitor `prompt_on` names), `enterMs` and
  `exitMs`. `Lock.submit()`
  checks what was typed, as Return does, and `Lock.clear()` empties it, as
  Escape does; both carry nothing. React to it as to any attached object,
  with `Lock.onStateChanged: ...` on one of your items, as the example
  below does.
- `SecretInput`, the password field: a dot for each character typed and a
  caret. Keys go into it whichever item has focus, with no click first.
  Its colours, sizes and spacing are properties. `SecretField` is the
  native item under it, for a field drawn your own way.
- `SecretKeypad`, a passcode pad whose keys are your `delegate`, laid out
  as `lock.keypad` says. Solium finds the key under a press or a finger and
  types it; your scene is told only that a key was pressed
  (`keyPressed()`), never which.

A lock scene never sees the session, whether it is locked or not: it reads
the monitors, the keyboard layout and its own clock, and `Windows`,
`Workspaces`, `Apps`, `Folder`, a `WindowList` or `WorkspaceList`,
`Solium.status` and `Solium.dirs` read empty in it at all times, the hours it
waits, kept, for the next lock included
(`lock_auth::tests::a_lock_scene_reads_only_what_a_lock_may_show`). A scene
you got from someone else can show a clock and a field, and nothing of what
you were doing. The pointer's scene, the one other scene drawn over the lock,
reads the session the same way, at all times
(`lock_auth::tests::the_pointer_scene_reads_only_what_a_lock_may_show`); your
bar, dock and home screen keep theirs through the lock, since they are not
drawn behind it, and nothing in them is built again at the unlock
(`state::tests::real_client::lock_focus::native::a_session_scene_keeps_its_rows_through_a_native_lock`). The other way round, no other scene sees the lock: in a bar
or a widget, `Lock` reads `"idle"`, `Lock.submit()` and `Lock.clear()` do
nothing, a `SecretField` reads nothing and a `SecretKeypad` lays out no key,
so nothing on the desktop can time what is typed at the lock or submit for
you (`lock_auth::tests::a_session_scene_reads_nothing_of_the_lock_and_cannot_submit`).
Keys never reach the scene; the pointer and fingers on its monitor do, as Qt
events, so its buttons and swipes work. Over a `SecretKeypad`, and within
`lock.keypad_margin` pixels of it (32 by default), the scene is told the
pointer left and nothing more, since the way the pointer takes to a key says
which key it is
(`lock_auth::tests::the_pointer_near_the_keypad_is_kept_from_the_scene`).

When the password cannot be checked, `Lock.state` reads `"unavailable"` and
`Lock.message` says how to unlock from another console. If PAM had the
password and asked for more than it, such as a one-time code, that counts as
a wrong password toward `lock.lockout`. If the helper crashed or took too
long, the same password is tried again, at most `lock.retry.attempts` times
(`Lock.retryAfter` counting down to each), and then wiped; if it never can
answer (it is missing, or PAM has no `solium` service), the password is wiped
at once. Either way the field takes typing again
(`lock_auth::tests::a_refused_check_counts_toward_the_lockout_and_wipes_the_password`,
`lock_auth::tests::an_unavailable_check_is_retried_at_most_the_settings_times`).

When the session locks, it is locked at once: the keyboard and the pointer
are the lock's from that instant, and a screenshot or a recording sees
nothing of the session from then on
(`state::tests::real_client::lock_focus::native::entry`). But for `enter_ms`
the desktop is still drawn under the scene, with `Lock.state` reading
`"locking"` and `Lock.locked` true, so the scene can come down over it like a
sheet, or fade in, or anything else that starts transparent. At that bound
the desktop is gone, whatever the scene is doing
(`lock_auth::tests::the_entry_is_bounded_even_if_the_scene_never_finishes`),
and only then is the session counted as locked: `LockedHint`, and the wait
`before_sleep` holds, follow the first frame that shows the lock whole. What
is typed during the entry is the password's. The lock taken because the
machine is about to sleep has no entry, nor does one taken while every
monitor is off: it is up on its first frame, and an entry under way when
sleep comes ends there
(`logind::tests::a_lock_before_sleep_has_no_entry_and_cuts_one_short`). A
scene that fails during its entry is replaced as above, and the desktop is
not drawn under the one that stands in for it; nor under the fallback, which
does not move.

When the password is right, Solium unlocks at once: the keyboard and the
pointer are the desktop's from that moment. The scene stays over the desktop
with `Lock.state` reading `"unlocking"`, so it can slide or fade itself away,
for `exit_ms` counted from the first frame drawn after the unlock, so a
slow frame cannot eat the slide; at that bound it is gone, whatever it is
doing
(`lock_auth::tests::the_exit_plays_over_the_desktop_for_exit_ms_and_input_is_the_desktops_at_once`,
`lock_auth::tests::the_exit_lasts_exit_ms_from_its_first_frame_however_late_that_comes`).
`prompt_on` picks the one monitor whose scene shows the field; the others
show theirs without it
(`lock_auth::tests::the_prompt_is_on_the_monitor_prompt_on_names_and_no_other`).

**Pulled down by a finger.** On an iPhone, the lock follows a finger pulled
down from the top and locks when it is let go far enough. Solium gives your
configuration the piece it cannot make itself, and leaves the gesture to you:
`sol.lock_peek(progress)` draws every monitor's lock scene over the desktop,
with `Lock.state` reading `"peeking"` and `Lock.progress` how far down, from 0
to 1. It is a picture and nothing more: nothing is locked, `LockedHint` is
left alone, the keyboard, the pointer and the fingers stay the session's, and
nothing typed reaches the lock
(`state::tests::real_client::lock_focus::native::peek`). Call it as the finger
moves, then `sol.lock_now()` to lock from there, or `sol.lock_peek(false)` to
let go. Locked from a peek, the entry goes on from `Lock.progress` and lasts
what is left of `enter_ms`, `enterMs * (1 - progress)`, so the scene does not
jump back to the top; `Lock.progress` keeps the peek's value until the entry
is over. Let go, the scene goes back up through the exit, reading
`"unlocking"` for `exit_ms` at most, and is then quiet again. Over a session
already locked, or with `native` off, `sol.lock_peek` does nothing, and the
fallback is never drawn in a peek, since it does not move. A pull from the
top edge of any monitor that locks past halfway:

```lua
-- In your config.lua, or a file it requires.
local pull = nil -- the finger pulling: its slot, and its monitor's top and height

local function monitor_named(name)
    for _, monitor in ipairs(sol.monitors()) do
        if monitor.name == name then
            return monitor.whole
        end
    end
end

sol.on("touch", function(event)
    if event.kind == "down" then
        local whole = not pull and monitor_named(event.output)
        if whole and event.y - whole.y < 24 then
            sol.touch(event.slot, { action = "capture" })
            pull = { slot = event.slot, top = whole.y, height = whole.h }
        end
        return
    end
    if not pull or event.slot ~= pull.slot then
        return
    end
    local progress = math.max(0, math.min(1, (event.y - pull.top) / pull.height))
    if event.kind == "motion" then
        sol.lock_peek(progress)
    elseif event.kind == "up" and progress > 0.5 then
        sol.lock_now()
        pull = nil
    else
        sol.lock_peek(false)
        pull = nil
    end
end)
```

`event.vy`, the finger's speed as it lifts, makes a quick flick lock too.

A minimal scene: the time, the field on the prompt's monitor, what went
wrong, and a sheet that follows a peek, comes down as the session locks, and
goes back up as it unlocks.

```qml
// ~/.config/solium/qml/lock/Mine.qml
import QtQuick
import Solium

Item {
    id: root
    readonly property bool prompt: Lock.promptOn === Solium.monitor.name

    // How far down the sheet is: 0 above the top, 1 covering the screen.
    property real down: 1

    // The time, read each minute while locked, and afresh at every lock.
    property date now: new Date()
    Timer {
        interval: 60000
        running: Lock.locked
        repeat: true
        onTriggered: root.now = new Date()
    }
    Lock.onLockedChanged: {
        if (Lock.locked) {
            root.now = new Date()
            slide.stop()
            // From the top, or from where a peek left it.
            root.down = Lock.state === "locking" ? Lock.progress : 1
            if (Lock.state === "locking")
                slide.go(1, Lock.enterMs * (1 - Lock.progress))
        }
    }
    // Pulled down by a finger: exactly where the finger has it.
    Lock.onProgressChanged: {
        if (Lock.state === "peeking") {
            slide.stop()
            root.down = Lock.progress
        }
    }
    Lock.onStateChanged: {
        if (Lock.state === "peeking") {
            root.now = new Date()
            slide.stop()
            root.down = Lock.progress
        } else if (Lock.state === "unlocking") {
            slide.go(0, Lock.exitMs)
        } else if (Lock.locked && Lock.state !== "locking") {
            // The entry is over, or there was none: down, whatever is left.
            slide.stop()
            root.down = 1
        }
    }
    NumberAnimation {
        id: slide
        target: root; property: "down"
        easing.type: Easing.OutCubic
        function go(to, ms) {
            stop()
            slide.to = to
            slide.duration = Math.max(1, ms)
            start()
        }
    }

    Rectangle {
        width: root.width
        height: root.height
        y: (root.down - 1) * root.height
        color: "#101014"

        Column {
            anchors.centerIn: parent
            spacing: 16
            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                text: Qt.formatTime(root.now, "hh:mm")
                color: "white"
                font.pixelSize: 96
            }
            SecretInput {
                anchors.horizontalCenter: parent.horizontalCenter
                visible: root.prompt
            }
            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                visible: root.prompt
                color: "#bbbbbb"
                text: Lock.state === "failed" ? "Wrong password"
                    : Lock.state === "lockedOut" ? "Try again in " + Lock.retryAfter + " s"
                    : Lock.message
            }
        }
    }
}
```

### A PIN

Solium's own lock screen can take a PIN, a few digits, in place of your
password. There is none until you set one, from a terminal:

```sh
solium pin set      # asks for your password, then the new PIN twice
solium pin clear    # asks for your password
```

Both ask for your account password first and check it exactly as the lock
does, through `solium-auth` and PAM. That keeps a PIN from being set or
cleared by mistake or by a script. It does not stop someone already at your
unlocked session: they run as you and can write the PIN's files directly,
which is no more than they could do anyway, since `loginctl unlock-session`
from inside that session unlocks it. Lock the session when you leave it.
Neither command echoes what you type, and both refuse to run without a
terminal, so a PIN cannot be piped in from a script or left in a file. Only an argon2id hash of the PIN is stored, in
`~/.local/state/solium/pin` (`$XDG_STATE_HOME/solium/pin`), mode 0600; the PIN
itself is written nowhere (`solium_auth::pin::tests::the_file_holds_a_hash_and_never_the_pin`),
and is checked by `solium-auth` too, never by the compositor.

While a PIN is set, the lock asks for it: `Lock.mode` reads `"pin"`, and the
shipped lock screen says "Enter PIN" with its digit pad already up. Type it
on the keyboard or tap it on the pad, and Return or OK. **Your password still
works**: whatever you type that is not 4 to 8 digits (`lock.pin.min_length`
and `max_length`) is checked as your password instead
(`lock_auth::tests::the_password_unlocks_in_pin_mode_and_resets_the_counts`).
A password that is itself 4 to 8 digits works too: what is not the PIN is
then checked as the password, and one wrong entry counts once
(`lock_auth::tests::a_password_of_digits_unlocks_in_pin_mode`).

The lock asks for your password instead, `Lock.mode` reading `"password"` and
`Lock.message` saying why:

- after `lock.pin.attempts` wrong PINs in a row (5), until the password is
  entered. The count is kept in `~/.local/state/solium/pin-state`, so
  restarting Solium does not start it again
  (`lock_auth::tests::a_wrong_pin_counts_and_after_attempts_only_the_password_unlocks`,
  `lock_auth::tests::the_pin_counts_survive_a_restart`);
- the first time the lock is unlocked after the computer starts
  (`lock_auth::tests::a_restart_needs_the_password`);
- when the password has not unlocked it for `lock.pin.password_after_hours`
  (48), counted both on the wall clock and on `CLOCK_BOOTTIME`, which
  includes sleep and which setting the time does not move, so turning the
  clock back does not stretch it
  (`lock_auth::tests::the_password_is_needed_after_password_after_hours`).

Resuming from suspend or from hibernation is not a restart: the kernel's boot
id survives both, so the PIN keeps working, within the same hours.

Entering the password resets all three. Wrong PINs count toward
`lock.lockout` as wrong passwords do, on top of these. A PIN file that is not
yours, that others can write, or that does not hold such a hash is no PIN at
all: the lock asks for the password, and the log says why. The same goes for
`pin-state`, and for the `solium` directory they are in: one that is not
yours or that others can write is never written to, nor mended, and the PIN
is not used until you fix it.

```lua
return {
    lock = {
        pin = {
            enabled = true,             -- false keeps a PIN that is set from being used
            min_length = 6,             -- the fewest digits, 4 or more
            max_length = 8,             -- the most, up to 64
            attempts = 5,               -- wrong PINs before the password is needed, at most 10
            password_after_hours = 48,  -- the password at least this often, at most 72
        },
    },
}
```

A lock screen of your own can follow `Lock.mode` the same way: show a
`SecretKeypad` for `"pin"`, a `SecretInput` either way, and `Lock.message`
when it is not empty.

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

### Notifications

Solium is the notification server: what `notify-send`, a mail client or a
browser sends comes to it, and your scenes draw it. Nothing is drawn for you;
a banner is QML of your own. `config.lua`'s `notifications` table turns the
server on (it is on in the shipped configuration) and says how long banners
last and how many are kept afterwards:

```lua
return { notifications = { timeouts = { normal = 5000 }, history = { max = 50 } } }
```

A program that already has the name — mako, dunst, another desktop's shell —
keeps it, and Solium waits its turn; `Notifications.serving` says which.
Nested, Solium serves only on the bus `SOLIUM_SESSION_BUS` names, so a test
run never takes your desktop's notifications.

Every scene, and the lock's, can read `Notifications`: `banners` (on screen
now) and `history` (kept once a banner ends, with its actions still working),
both newest first, plus `dnd`, `serving` and `owner`. A banner, as a sketch:

```qml
Column {
    anchors { top: parent.top; horizontalCenter: parent.horizontalCenter; topMargin: 12 }
    spacing: 8
    Repeater {
        model: Notifications.banners
        delegate: Rectangle {
            required property var model
            readonly property var n: Notifications.get(model.id)
            width: 380; height: 72; radius: 18
            color: "#202024"
            HoverHandler { onHoveredChanged: hovered ? n.hold() : n.release() }
            TapHandler { onTapped: n.invoke("default") }
            Image { x: 14; y: 14; width: 44; height: 44; source: model.image || model.icon }
            Column {
                x: 70; y: 14; width: parent.width - 84
                Text { text: model.summary; textFormat: Text.PlainText; font.bold: true; elide: Text.ElideRight; width: parent.width }
                Text { text: model.bodyRich; textFormat: Text.StyledText; elide: Text.ElideRight; width: parent.width }
            }
        }
    }
}
```

The calls on a notification (`n.hold()`, `n.release()`, `n.dismiss()`,
`n.invoke(key)`) and on `Notifications` (`dismissAll()`, `setDnd(on)`) go
straight to the server. A banner's time stops while it is held, while the
session is locked and while the screens are off. `bodyRich` is markup safe
for `Text.StyledText`: bold, italic, underline and links only, never an
image. Every other field is the sender's own text: show it with
`textFormat: Text.PlainText`, or a `<img>` in a summary is a picture fetched
from wherever it points. `image` is the sender's picture, decoded and capped by Solium, and
`icon` the app's theme icon.

Lua decides what happens to each one, and hears what happened:

```lua
-- No banners from chat apps: they go straight to the history.
sol.on("notification", function(n)
    if n.category:match("^im") then return { banner = false } end
end)

-- An effect, or a sound, when something arrives.
sol.on("notifications", function(e)
    if e.what == "added" then sol.log("from " .. e.notification.appName) end
end)

-- The shell's own, with a button that runs Lua.
sol.notify{ summary = "Battery low", urgency = "critical",
            actions = { { key = "default", label = "", fn = function() sol.spawn("gnome-power-statistics") end } } }
```

`sol.act("notifications.dnd", { value = "toggle" })` turns do-not-disturb on
and off, where only critical notifications get a banner and the rest go
straight to the history.

### Volume

Solium reads PipeWire itself, so a slider in your shell is live and every
change says who made it. Every scene, the lock's too, can read `Audio`:
`Audio.output` and `Audio.input` follow the default device, so a binding to
`Audio.output.volume` survives a switch to headphones, and `Audio.outputs`
and `Audio.inputs` list every sink and source.

```qml
Slider {
    from: 0; to: Audio.output.max
    value: Audio.output.volume
    // Solium sends one write at a time, the newest, so this may write on
    // every move without flooding PipeWire.
    onMoved: Audio.output.setVolume(value)
}
```

An endpoint has `id`, `name`, `label`, `kind` (`speakers`, `headphones`,
`headset`, `hdmi`, `bluetooth`, `usb`, `virtual` or `other`), `icon`,
`volume` (as `wpctl` shows it), `muted`, `pending`, `max`, `step`,
`isDefault`, `portLabel`, `portKind` and `state`, and the calls
`setVolume(v)`, `adjust(by)`, `setMuted(on)`, `toggleMuted()` and
`makeDefault()`. A card's volume is written through its route, as
WirePlumber does, so the hardware mixer moves and the level is remembered.

Every application playing is there too, as `Audio.streams`, each with its
`app` (the application's name), `appId`, `binary`, `pid`, `mediaName` (what it
plays), `icon` and `windows`, the ids of the windows it is, with the volume,
`muted` and the calls an endpoint has but `makeDefault()`. A stream's volume is
its own, over the device's: 1.0 leaves the application as loud as it is.

A window's own volume, a titlebar's scroll or a slider in its quick settings,
is `Audio.forWindow(id)`: the window's streams as one stream that is never
null, `present` while it has any, its `volume` the loudest of theirs, `muted`
while every one is, `count` of them, and every call acting on all of them
together, so a browser's three tabs move as one. A window is matched to its
streams by its process: the stream's own, then the processes that started it,
so a browser whose sound comes from a helper process, or a player started in a
terminal, is found; failing that, by its application id, `org.mozilla.firefox`
naming Firefox's stream by its last part.

```qml
// A taskbar's button for each window: the wheel over it is that window's
// volume.
Repeater {
    model: Windows
    delegate: Item {
        id: button
        required property var model
        readonly property var sound: Audio.forWindow(model.id)
        width: 32; height: 32
        WheelHandler {
            target: null
            enabled: button.sound.present
            onWheel: (event) => button.sound.adjust(
                event.angleDelta.y > 0 ? button.sound.step : -button.sound.step)
        }
    }
}
```

Lua reads the same: `sol.audio().streams`, and `sol.audio().windows[id]` for
one window's, and sets a window's with
`sol.act("audio.volume", { window = id, by = 0.05 })` or
`sol.act("audio.mute", { window = id, value = "toggle" })`. The lock's scenes
read no streams: which applications are playing, and what, is no one else's.

The volume keys in the shipped configuration go through the same service,
capped at `config.lua`'s `audio.max`. Lua hears every change, and who made
it, which is what an on-screen display needs:

```lua
sol.on("audio", function(e)
    -- A key or a headset's own button, not a slider already on screen.
    if e.what == "volume" and e.output and e.origin ~= "scene" then
        sol.log(string.format("volume %d%% (%s)", math.floor(e.value * 100 + 0.5), e.origin))
    end
end)

sol.bind("super+m", function() sol.act("audio.mute", { target = "input", value = "toggle" }) end)
```

### Tray

Solium is the tray: the icons Steam, Discord, Telegram and the rest put there
come to it (`org.kde.StatusNotifierWatcher`), and your scenes draw them, menus
included. `config.lua`'s `tray` table turns it on, as it is in the shipped
configuration:

```lua
return { tray = { serve = true } }
```

As with notifications, a program that already has the name (another
desktop's panel) keeps it and Solium waits its turn, `Tray.serving` says
which, and a nested run serves only on the bus `SOLIUM_SESSION_BUS` names.

Every scene can read `Tray.items`, in the order the items arrived; a lock's
scenes read none. A tray in a bar, with its menu, as a sketch:

```qml
Row {
    spacing: 6
    Repeater {
        model: Tray.items
        delegate: Item {
            id: slot
            required property var model
            readonly property var item: Tray.get(model.id)
            visible: model.status !== "passive"
            width: 22; height: 22
            Image { anchors.fill: parent; source: model.icon; sourceSize: Qt.size(44, 44) }
            TapHandler {
                acceptedButtons: Qt.LeftButton | Qt.MiddleButton | Qt.RightButton
                onTapped: (point, button) => {
                    const at = slot.mapToGlobal(point.position.x, point.position.y)
                    if (button === Qt.RightButton) slot.item.openMenu(at.x, at.y)
                    else if (button === Qt.MiddleButton) slot.item.secondaryActivate(at.x, at.y)
                    else slot.item.activate(at.x, at.y)
                }
            }
            WheelHandler { onWheel: (wheel) => slot.item.scroll(wheel.angleDelta.y) }
            // In the window's own content, not under the icon: drawn
            // outside the icon's bounds, it would be seen and never pressed.
            Column {
                parent: slot.Window.contentItem
                visible: model.menuOpen
                readonly property point at: visible ? slot.mapToItem(parent, 0, 0) : Qt.point(0, 0)
                x: at.x
                y: at.y - height - 8
                Repeater {
                    model: slot.model.menu
                    delegate: Text {
                        required property var modelData
                        visible: modelData.visible && modelData.type !== "separator"
                        text: modelData.label
                        textFormat: Text.PlainText
                        color: modelData.enabled ? "white" : "gray"
                        TapHandler { onTapped: slot.item.clickMenu(modelData.id) }
                    }
                }
            }
        }
    }
}
```

An item has `id`, `itemId` (its own name for itself, `steam`), `title`,
`status` (`passive` asks to be hidden, `attention` to be looked at), `icon`
(what to draw now: its attention picture while it asks for attention),
`attentionIcon`, `overlayIcon`, `tooltipTitle`, `tooltipBody`, `itemIsMenu`,
`hasMenu`, `menuOpen` and `menu`. Its pictures are what the application
sent, decoded and capped by Solium (`image://solium/tray/...`), or a theme
icon (`image://solium/icon/...`).

Its calls go straight to the application: `activate(x, y)` (a primary click,
which opens the menu instead for an item that is only a menu),
`secondaryActivate(x, y)` (a middle click), `scroll(delta, orientation)`,
`contextMenu(x, y)` (the application draws its own menu), and the menu's:
`openMenu(x, y)`, `closeMenu()`, `clickMenu(entry)` and `openSubmenu(entry)`,
with `Tray.closeMenus()` for a click outside. `openMenu` turns `menuOpen` on
at once and fills `menu` when the application answers; one menu is open at a
time. An entry has `id`, `label`, `enabled`, `visible`, `type` (`standard`
or `separator`), `toggleType` (`checkmark`, `radio` or empty), `toggleState`,
`icon`, `submenu` and `children`. Labels and titles are the application's
own text: show them with `textFormat: Text.PlainText`.

Lua hears what changes, and acts the same way:

```lua
sol.on("tray", function(e)
    if e.what == "changed" and e.item.status == "attention" then
        sol.log(e.item.title .. " wants attention")
    end
end)

-- Open Steam's menu from a key.
sol.bind("super+shift+s", function()
    for _, item in ipairs(sol.tray().items) do
        if item.itemId == "steam" then sol.act("tray.open_menu", { id = item.id }) end
    end
end)
```

### Now playing

Solium follows the media players on the session bus (MPRIS: mpv with
`mpv-mpris`, Spotify, Firefox, VLC and the rest), so a shell shows what is
playing and the media keys reach the player with no `playerctl`.
`config.lua`'s `media` table turns it on, as it is in the shipped
configuration:

```lua
return { media = { watch = true, ignore = { "playerctld" }, position_interval_ms = 1000 } }
```

`Media.active` is the player the keys reach: the one playing, else the last
that played. It is never null, so a card can bind to it straight away and
hide itself on `present`. `Media.players` lists every player, in the order
they appeared; `Media.get(id)` finds one. A lock's scenes read no players:
what someone listens to is theirs. A now-playing card, as a sketch:

```qml
Row {
    readonly property var player: Media.active
    visible: player.present
    spacing: 8
    Image {
        width: 40; height: 40
        // Art Solium read itself; a scene that wants art from the network
        // fetches `artUrl` itself.
        source: player.art
        sourceSize: Qt.size(80, 80)
    }
    Column {
        Text { text: player.title; textFormat: Text.PlainText; color: "white" }
        Text { text: player.artist; textFormat: Text.PlainText; color: "gray" }
        // Moves on by itself while it plays, every `position_interval_ms`.
        Rectangle {
            width: 160 * (player.length > 0 ? player.position / player.length : 0)
            height: 2; color: "white"
        }
    }
    Text {
        text: player.playing ? "⏸" : "▶"
        TapHandler { onTapped: player.playPause() }
    }
}
```

A player has `id`, `service` (its bus name), `identity` (its own name,
`Spotify`), `desktopEntry`, `icon`, `status` (`playing`, `paused` or
`stopped`), `playing`, `active`, `title`, `artist` (its artists joined),
`album`, `art`, `artUrl`, `trackId`, `length` and `position` (in seconds),
`rate`, `volume` and `shuffle` (null when it offers none), `loopStatus`, and
what it says it can do: `canPlay`, `canPause`, `canGoNext`,
`canGoPrevious`, `canSeek` and `canControl`. `art` is
`image://solium/media/...`: a picture the player named on disk or sent
inline, read, decoded (PNG or JPEG) and capped by Solium. `artUrl` is a
picture the player names on the network, as it named it: Solium fetches
nothing, and a scene that shows it makes the request from the compositor's
own process, so only bind it when you want that. Titles and names are the
player's own text: show them with `textFormat: Text.PlainText`.

Its calls go straight to the player: `playPause()`, `play()`, `pause()`,
`next()`, `previous()`, `seek(offset)` (seconds, forward or back),
`setPosition(position)` (seconds into the track) and `setVolume(v)` (its own
volume, 0.0 to 1.0). One the player says it cannot do is not made.

Lua reads the same with `sol.media()`, each player's `position` where it is
now, hears changes with `sol.on("media")`, and acts with `media.*`, on the
active player unless an `id` names another:

```lua
sol.on("media", function(e)
    if e.what == "active" and e.player then
        sol.log("now playing in " .. e.player.identity .. ": " .. e.player.title)
    end
end)

-- Ten seconds back, and the next track, from the keyboard.
sol.bind("super+comma", function() sol.act("media.seek", { offset = -10 }) end)
sol.bind("super+period", function() sol.act("media.next") end)
```

The shipped `XF86AudioPlay`, `Pause`, `Next` and `Prev` keys send
`media.play_pause`, `media.next` and `media.previous`, at the lock too, and
spawn `playerctl` only when `media.watch` is off. Nested, players are
followed only on the bus `SOLIUM_SESSION_BUS` names.

### Bars, docks and wallpapers

A shell — bar, dock and launcher together — can be hosted inside the
compositor, in the same QML engine as the window frames, by naming its root
file:

```lua
return { shell = { scene = "~/.config/solium/shell/shell.qml" } }
```

One scene on every monitor, or on the one `shell.on` names, written against
Solium's own QML API;
[shell-boundary.md](shell-boundary.md) has how to install one and what it is
and is not given.

Two more settings are the shell's. `shell.outside_click` is what a press
outside one of its open popups does after closing it: `"swallow"`, the
default, or `"pass"`, which also clicks what is under it. A table names popups
by their `Grab`'s `name`: `{ default = "swallow", ["tray-menu"] = "pass" }`.
`shell.keyboard.bindings` is which compositor bindings still work while one of
its fields holds the keyboard: `"except_claimed"` (the default: all but the
keys the field claims), `"all"`, or `"none"`. The Ctrl+Alt escapes always
work. A scene of your own drawn with `sol.surface` takes the same two, as
`outside_click` and `keyboard = { bindings = ... }`.
[shell-boundary.md](shell-boundary.md#what-a-hosted-shell-is-given) has `Grab`
and `Solium.keyboard`.

A bar can also be an ordinary client over `wlr-layer-shell`, which means any
panel already written for that protocol works. A surface names the output it
wants and the compositor honours it, so a bar on every screen is one surface
per screen, each reserving from *that* monitor's work area. One that names no
output gets the primary monitor.

A fullscreen window in front of the workspace its monitor is showing covers
the top layer -- a client's bar, and one declared with `sol.surface` -- and
takes the clicks where the bar was, while the overlay layer (notifications, an
OSD, a launcher) stays over it. It goes over the bars as it starts to grow and
back under them once it has finished shrinking. To keep the bars over
fullscreen windows:

```lua
return { fullscreen = { covers = "none" } }
```

See **[shell-boundary.md](shell-boundary.md)** for why hosting is the design,
and for both ways a shell attaches.

### The preview shell

A fresh install is not a blank desktop. `shell.scene` above defaults to the
compositor's own preview shell -- a bar along the bottom, built the same way
any other shell is, on the public API `shell-boundary.md` describes and
nothing else. It shows:

- the workspaces of this monitor, as page dots -- the current one an accent
  pill, an occupied one filled -- and a click switches;
- the running windows of this monitor's current workspace, as chips -- icon,
  title or app id, the focused one highlighted -- and a click focuses one;
- the tray: applications' icons ([Tray](#tray)), a left click activating
  one, a middle click its second action, the wheel scrolling it, and a right
  click opening its menu above the bar, drawn in the bar's look;
- the keyboard layout, once more than one is configured;
- a clock;
- a top dock: pinned apps, then a hairline, then running ones that are not
  pinned, each a theme icon with a dot under a running one. Autohidden by
  default -- nothing is reserved, and it slides in when the pointer reaches
  the top edge, out a little after it leaves. A click on an app that is not
  running launches it (`Solium.Apps`, 03 §3.2.13); on one that is, focuses its
  most recently used window.

  ```lua
  return { preview = { dock = {
      pinned = { "org.mozilla.firefox", "kitty" }, -- desktop ids, left unset
                                                    -- the dock picks a
                                                    -- terminal, a file
                                                    -- manager and a browser
                                                    -- from what is installed
      visibility = "always",                       -- or "autohide" (default)
      icon_size = 48,                               -- default 40
  } } }
  ```

- quick search, `super+d`: a centred panel with a field, the live keyboard
  layout beside it, and results grouped into applications (matched by name
  and generic name), open windows (title or app id, with the workspace) and
  a few compositor commands that already exist -- Overview, Tile windows,
  Fullscreen window, Reload configuration, Quit Solium. Arrow keys move the
  selection, Return activates it, Escape or a click outside closes.

  ```lua
  return { preview = { search = {
      key = "super+alt+space", -- default "super+d"
  } } }
  ```

  Cut from this first version (`lua/preview/search.lua` and
  `qml/preview/Search.qml` both say why, in full): the detail pane, a row's
  own actions, Alt+digits, Ctrl+Return, the empty-query suggestions (there
  is no usage history yet to rank by), app keywords (the plain array this
  searches, `sol.apps()`, does not carry them -- a real gap, not a corner
  cut), the settings and desktop-file sources, the container transform from
  the bar's own search button (the bar has no search button yet either),
  and the "us,ru" layout-correction pass -- it needs the compositor's own
  keymap, which nothing here exposes yet.

- desktop icons: `Solium.dirs.desktop` (`$XDG_DESKTOP_DIR`, or
  `user-dirs.dirs`; nothing drawn, and nothing scanned, when it is unset or
  is `$HOME`), on their own surface -- `layer = "bottom"`, below ordinary
  windows, the same as a real desktop, and below the bar above -- 96x100
  cells with 48 px icons, column by column from the top right, folders
  first then by name. A click selects (Ctrl or Shift adds), a double-click
  opens: a regular file through its MIME type's default application, a
  `.desktop` launcher through its own `Exec=` once trusted, asked for once
  in a small card. Live through inotify: a file that appears or disappears
  on the desktop updates the icons at once, no reload needed.

  ```lua
  return { preview = { desktop = {
      from = "top-left",                   -- or "top-right" (default)
      cell = { width = 110, height = 112 }, -- default 96x100
      labels = 1,                          -- label lines, default 2
      open = "single",                     -- or "double" (default)
      show_hidden = true,                  -- default false
  } } }
  ```

  Cut from this first version (`crates/solium/src/folder.rs` and
  `qml/preview/Desktop.qml` both say why, in full): dragging and saved
  positions (there is nowhere durable to keep them yet), the right-click
  menus, a rubber band, showing the desktop (`super+ctrl+d` needs a
  window-presenting pass this compositor does not have), Open With and
  thumbnails, MIME detection by content rather than by extension, and the
  arrow-key/Return navigation the design note asks for (nothing yet gives
  the desktop the keyboard on demand, so claiming it unconditionally would
  only fight the dock and quick search for focus).

Next pieces -- the island, quick settings and previews -- need
services this compositor does not have yet, and are not here. `lua/preview/`
and `qml/preview/` are where all of it lives, so copying one file still
changes one behaviour, exactly as **[Your own frame](#your-own-frame)** and
**[Bars, docks and wallpapers](#bars-docks-and-wallpapers)** above do it.

Two ways to turn it off, or replace it:

```lua
return { preview = false }
```

turns the whole preview shell off and leaves a blank desktop, as before it
existed. Naming your own `shell.scene` -- or `SOLIUM_SHELL_SCENE` for one run
-- replaces it outright: the preview shell only ever fills in a default
nothing else set, and never overrides either one.

### Your own pointer

The pointer can be a QML scene of yours, named the way a shell is:

```lua
return { cursor = { scene = "~/.config/solium/cursor/Cursor.qml", size = 32 } }
```

A path, `~` expanded, or a bare name looked for in `~/.config/solium/qml/`
and then in the shipped QML
(`cursor::theme::tests::a_configured_scene_is_found_as_the_shells_is`).
`SOLIUM_QML_CURSOR=<file>` is the scene for one run, over the setting
(`cursor::theme::tests::the_environment_overrides_the_configured_scene`).
It draws every shape a window or the compositor asks for, ahead of any
XCursor theme, and is told which one
(`models::pointer::tests::a_configured_scene_hears_every_named_shape`).
That includes X11 windows, such as Steam: their arrow, I-beam and hand are
handed over as pictures, but the X server knows them by name (`left_ptr`,
`xterm`, `hand2`, and so on), and a cursor with a name it knows is drawn as
that shape
(`state::tests::real_client::x11_cursor::xwaylands_cursor_surface_is_drawn_as_the_shape_its_x_name_stands_for`,
`cursor::x11::tests::the_x11_names_are_the_shapes_they_mean`). A
window that sets a cursor of its own, or hides the pointer as a game does,
still does over its own surface
(`cursor::tests::a_pointer_a_client_hides_stays_hidden_with_a_scene_configured`).
`super+shift+r` applies a change and builds the scene again when its files
changed. A scene that does not load leaves the theme, and then Solium's own
arrow, to draw the pointer, says so in the log, and is tried again at the
next reload (`cursor::tests::a_reload_swaps_the_scene`).

It reads `Solium.cursor`. `shape` is the shape asked for, by its CSS cursor
name: `default` over a titlebar, `text` over a text field, `ew-resize` on a
window's left or right edge, and so on
(`models::pointer::tests::a_named_shape_reaches_solium_cursor_shape`).
`pressed` is whether a button is held, `velocity` how fast the pointer
moved over the last frame in logical pixels a second along `x` and `y`,
zero once it stops, `scale` the scale of the monitor it is on and `size` the
configured `cursor.size`
(`models::pointer::tests::the_published_pointer_is_its_buttons_its_motion_its_monitor_and_its_size`).
When the pointer is taken somewhere -- following the keyboard, or by
`sol.warp` -- the pointer is there at once and its picture flies: `moving` is
true while it does, `from` and `to` are where it set off and where it is going
in the desktop's logical pixels, kept once it lands, and `velocity` is the
flight's own, so a scene can stretch or trail the picture by it
(`models::pointer::tests::a_flight_is_published_while_it_flies_and_said_to_have_landed_after`,
`qml::pointer::tests::a_published_flight_reaches_solium_cursor`).
It says one thing back, `Solium.cursor.hotspot` on its root: the point of
its picture that sits on the pointer, its top-left corner unless it says.
Its size is its root's own `width` and `height`, up to 256 logical pixels
a side, so a glow or a shadow can reach past `size` with the hotspot still
on the point the pointer is at; a root that sets no size is `size` square
(`cursor::scene::tests::the_scene_is_drawn_at_its_own_size_with_its_hotspot_on_the_pointer`).

An arrow, an I-beam over text, and a glow that breathes:

```qml
// ~/.config/solium/cursor/Cursor.qml
import QtQuick
import QtQuick.Effects
import QtQuick.Shapes
import Solium

Item {
    id: root
    readonly property int pad: 10        // room for the glow
    readonly property int size: Solium.cursor.size
    readonly property bool text: Solium.cursor.shape === "text"
    width: size + 2 * pad
    height: size + 2 * pad
    // The arrow points with its tip, the I-beam with its middle.
    Solium.cursor.hotspot: text ? Qt.point(pad + size / 2, pad + size / 2)
                                : Qt.point(pad, pad)

    // One breath, forever: the glow's strength and a tint in the arrow.
    property real breath: 0.25
    SequentialAnimation on breath {
        loops: Animation.Infinite
        NumberAnimation { to: 1; duration: 800; easing.type: Easing.InOutSine }
        NumberAnimation { to: 0.25; duration: 800; easing.type: Easing.InOutSine }
    }

    MultiEffect {                        // the glow, behind the drawing
        source: drawing
        anchors.fill: drawing
        shadowEnabled: true
        shadowColor: "#7aa2ff"
        shadowBlur: 1.0
        shadowOpacity: root.breath
    }

    Item {
        id: drawing
        x: root.pad
        y: root.pad
        width: root.size
        height: root.size
        readonly property real u: width / 24

        Shape {                          // the arrow
            anchors.fill: parent
            visible: !root.text
            ShapePath {
                fillColor: Qt.tint("white", Qt.rgba(0.48, 0.64, 1, 0.45 * root.breath))
                strokeColor: "#1c1c1e"
                strokeWidth: 1.6 * drawing.u
                startX: 0.8 * drawing.u; startY: 0.8 * drawing.u
                PathLine { x: 0.8 * drawing.u;  y: 16.3 * drawing.u }
                PathLine { x: 5.0 * drawing.u;  y: 12.5 * drawing.u }
                PathLine { x: 7.9 * drawing.u;  y: 19.1 * drawing.u }
                PathLine { x: 11.0 * drawing.u; y: 17.7 * drawing.u }
                PathLine { x: 8.2 * drawing.u;  y: 11.3 * drawing.u }
                PathLine { x: 13.7 * drawing.u; y: 10.9 * drawing.u }
                PathLine { x: 0.8 * drawing.u;  y: 0.8 * drawing.u }
            }
        }

        Rectangle {                      // the I-beam
            visible: root.text
            anchors.centerIn: parent
            width: 3 * drawing.u
            height: 16 * drawing.u
            color: "white"
            border.color: "#1c1c1e"
        }
    }
}
```

It animates on the compositor's clock: a running animation asks for the
next frame, and a scene with nothing running asks for none
(`cursor::scene::tests::an_animating_scene_asks_for_the_next_frame_only_while_it_animates`).
This one breathes forever: while the pointer is shown it is drawn on every
refresh, and while it is not (a window hiding it or drawing its own, or the
screens off) the breath is still stepped a frame apart, every 16 ms
(`qml::wake::tests::a_timer_beside_an_undrawn_animation_fires_with_no_frame_drawn`).
To let it rest, bind the animation's `running:` to something the scene can
see, such as `Solium.cursor.pressed`, the shape, or the velocity.

`MultiEffect` and `ShaderEffect` draw on the GPU path, which a session on the
hardware uses by default (`dev/wirecheck`'s case 11 draws a pointer scene's
glow there). On the software path, which a nested session uses and
`qml.renderer = "software"` chooses, they draw nothing, and the drawing
under the glow is the whole pointer: that is why the breath is in the
arrow's tint as well. The picture is offered to the hardware cursor plane,
as a theme's is
(`cursor::scene::tests::a_scene_with_no_material_is_a_cursor_plane_element`),
and goes there when its size in device pixels fits the plane, commonly 64 or
128 pixels a side; there moving the mouse redraws nothing else. A larger one,
such as this one at `size = 32` on a 2x monitor (104 pixels a side), is
drawn with everything else where the plane is smaller.
`Solium.region` and `Solium.material` are accepted, and materials are off
until [#199](https://github.com/Lilium-Linux/solium/issues/199):
`Solium.materialState` reads `"off"`
(`qml::pointer::tests::a_region_and_a_material_are_accepted_and_materials_are_off`).

### Your own colours

`Solium.Theme` (`crates/solium/qml/Solium/Theme.qml`) is what every shipped
frame, the loading window and the keyboard pill are drawn with, and a hosted
shell that imports `Solium` can read it too. The shipped one is a dark theme
of greys only, with no hue in it at all, the maintainer's choice for now:
near-black bars, light grey text, grey buttons, and an `accent` that is a
light grey too ([decorations.md](decorations.md#colours-and-fonts) lists every
name). A shell that reads `Theme.accent` for its highlights gets that grey.
The fallback pointer and the default wallpaper do not read it: their colours
are fixed. A copy of `Theme.qml` in
`~/.config/solium/qml/Solium/` is meant to restyle the frames, the loading
window and such a shell at once, and does not work yet: the shipped module is found first
([#88](https://github.com/Lilium-Linux/solium/issues/88)). Until then, a
frame of your own ([above](#your-own-frame)) can carry colours of its own.

### Your own animation feel

Named curves — `linear`, `outCubic`, `outBack`, `inOutQuad`, `inOutCubic`,
`spring` — or four numbers, which are a cubic bezier's control points:

```lua
return {
    tiling = { motion = { duration = 220, easing = { 0.34, 1.56, 0.64, 1 } } },
}
```

Those are the same four numbers CSS calls `cubic-bezier` and every easing
generator on the internet hands out, so a feel you found elsewhere transfers
directly. y may leave 0..1 — that is what overshoot is.

Going fullscreen and maximising are timed the same way, each on its own, and
both glide by default: the window grows from where it is to cover its monitor,
or its work area, and shrinks back. `animate = false` makes one instant and
`animate = true` is the shipped motion again, and `instant` names the
applications that change at once whatever `animate` says — a game or a video
player, say:

```lua
return {
    fullscreen = {
        animate = { duration = 200, easing = "inOutCubic" },
        instant = { app_id = { "mpv", "gamescope" } },
    },
    maximize = { animate = false },
}
```

The application is told its new size the moment you press the key, and its
last picture is drawn into the new rectangle until it has drawn one at that
size — through the glide and after it, for a quarter of a second past the
landing at most, and then the window is shown at whatever size the application
has. How that picture goes in is `resize.fill`, the same setting as for a drag
or a layout's glide (below). Once it has answered the window is drawn as it is,
with nothing in between, so a fullscreen game or video can be shown directly.
An instant change is just that: nothing is moved, and the window is drawn as it
is on the next frame. An app id is what `sol.windows()` calls `app_id`.

While any window changes size — an edge or a seam you drag, a layout gliding
it to a new tile, a fullscreen or maximise glide — its application is a frame
or more behind the rectangle on screen. `resize.fill` decides what its picture
does meanwhile: `"crop"`, the default, keeps it at its own size from the
window's top-left, so text never changes size and the window beside a seam you
drag keeps its picture against the seam, as in Hyprland; `"anchor"` keeps it
against the edges that are not moving instead; `"stretch"` scales it to fill,
which is smooth but draws text up to twice as wide while the application
catches up.
`resize.background` is what shows where the picture does not reach:
`"edge"`, the default, stretches the picture's own edge over it, so a window
that is behind looks like it has more of its own background and a transparent
one stays transparent, as in Hyprland; a colour fills it instead, and `"none"`
lets you see through. `resize.patience` is how long, in milliseconds (250 by
default), the picture waits for the application once a change is over -- past
a drag's release, and past a glide's landing, where it keeps to the edges the
glide did not move -- before the window is shown at the size the application
has. `resize.tell_hz` is how often, at most, the application is told its new
size while you drag an edge: 0, the default, on every frame the drag moves, so
its picture is never more than its own drawing time behind the edge; a number
caps it at that many times a second (10 was the fixed rate before).
`resize.follow` is what a floating window is drawn at while you drag its edge:
`"app"`, the default, draws it at the size its application has drawn, against
the edges you are not moving, so its frame always wraps its content and nothing
is ever cut off or left uncovered, as in KWin; `"pointer"` draws it exactly
where the pointer is and lets `resize.fill` cover the difference, as in
Hyprland. `resize.sync` is how long, in milliseconds, the windows a tiled
drag moves wait for one another's applications: 0, the default, moves each to
the pointer at once; a number moves them in rounds, each told its new size at
once and one whose application has drawn it held where it was until the others
have too, so a seam moves as one but at the slower application's pace.
`resize.overlap` (true by default) tells a window being resized it may draw its
next size as soon as a frame showing its last starts to be drawn, so the
application draws while the compositor does instead of after it; false waits
for the screen. `resize.modes`
gives a mode a fill of its own, for every size change while it is in charge,
as `focus.modes` does for focus; a mode it does not name takes `resize.fill`:

```lua
return {
    resize = {
        fill = "anchor",
        background = "edge",
        patience = 400,
        tell_hz = 0,
        follow = "app",
        sync = 0,
        overlap = true,
        modes = { scrolling = "stretch" },
    },
}
```

Desktop mode's snap — the focused window sliding to a half, a quarter or the
whole of its monitor while no layout is in charge (#222) — has its own
`floating.snap.motion`, kept apart from `tiling.motion` for the same reason
`fullscreen.animate` and `maximize.animate` are kept apart from each other:

```lua
return {
    floating = { snap = { motion = { duration = 160, easing = "outBack" } } },
}
```

The shipped motion is `{ duration = 220, easing = "outCubic" }`. There is no
instant switch for it the way `fullscreen.animate` and `maximize.animate` have
one above: a snap is always a glide, since nothing is waiting on an
application to draw a new size first.

### Your own bindings

A `bindings` section in `user.lua`, merged like every other section:

```lua
return {
    bindings = {
        ["super+b"]       = "firefox",
        ["super+shift+s"] = { "sh", "-c", "grim -g \"$(slurp)\"" },
        ["super+n"]       = function() sol.spawn("kitty", "-e", "nvim") end,
        ["super+g"]       = false,
    },
}
```

A string is a command line split on spaces; a list is one already split, for an
argument with a space in it; a function is anything else, with the whole `sol`
API in scope; `false` removes a shipped binding outright.

A list can also say how its key behaves
([#224](https://github.com/Lilium-Linux/solium/issues/224)). `repeating = true`
runs it again while the key is held, at the keyboard's own repeat delay and
rate, as the shipped volume and brightness keys do; `locked = true` lets it fire
at the lock screen as well, as those and the media keys do:

```lua
["XF86AudioRaiseVolume"] = { "pamixer", "-i", "5", repeating = true, locked = true },
```

A binding here replaces the shipped one whole, so a volume key you rebind
repeats only if your list says so. At the lock screen a binding may run
programs and change the keyboard, but not touch a window or a workspace, or
end the session. At Solium's own lock screen it fires only for a key that
types no character there: a letter, a digit or a symbol, with Shift or
without, always goes into the password, so no binding can read it a key at a
time (`lock_auth::tests::at_the_native_lock_a_locked_binding_never_takes_a_character`). A function from your own `init.lua` asks the same through
`sol.bind`'s third argument, `{ repeating = true, locked = true }`; see the
[Lua API](https://lilium-linux.github.io/solium/generated/reference/lua-api.html).

A combination you bind belongs to the compositor from then on, and while the
session is unlocked the application with the keyboard never receives it. So
binding a plain key such as `escape`, `f1` or `return` takes that key from
every window. This is why the overview binds Escape only while it is open
([#174](https://github.com/Lilium-Linux/solium/issues/174)).
[Key bindings](https://lilium-linux.github.io/solium/generated/reference/bindings.html)
lists every shipped one, so you can see what a key does before you take it
over — `super+g` above is a demonstration that tilts a window.

**A binding here replaces a shipped one on the same combination.** On purpose:
refusing a clash would let you add a binding and forbid you to change one, and
changing one is what most people come here for. It is not silent — `solium
--check` marks every combination this section took over, and lists the ones it
removed:

```
    super+b                     config.bindings
    super+q                     config.bindings, replacing a shipped binding
    xf86audioraisevolume        config.bindings, replacing a shipped binding (repeats while held, fires at the lock)
  1 binding(s) removed by the configuration:
    super+g                     config.bindings
```

Reading that list is also how you find out an edit dropped a binding, and it is
the only way to catch `super+whoops`: a combination is whatever you press, so
nothing can check the name for you.

Writing your own `~/.config/solium/init.lua` is the other way, and is for
rewriting the session rather than adding to it — it replaces the entry point
whole. It can still `require` everything that ships, including `bindings`,
which must stay **last**: `sol.bind` lets the later call win, and that ordering
is what puts your `config.bindings` on top of the shipped ones.

```lua
require("modes")
require("tiling")

sol.bind("super+return", function() sol.spawn("kitty") end)

require("bindings")
```

### Moving around by keyboard

`super+arrows` and `super+h/j/k/l` focus the window that way,
`super+shift+arrows` and `super+shift+h/j/l` move it, and `super+alt+k` moves
it up -- `super+shift+k` is the keyboard layout. At the edge of a screen both go
on to the next one. `super+f` is fullscreen, `super+shift+m` maximised and
`super+shift+space` floats a window over the layout; each key again puts it
back, and fullscreen and maximised glide there and back. `super+m` minimizes
the focused window into a strip at the bottom of the monitor with the genie;
focusing it -- a click in the overview, its application asking -- brings it
back out. `minimize` in `config.lua` sets how long that takes, how much of the
window trails behind, and the size of the strip
(`state::tests::real_client::reflow_on_close::minimize::config_minimize_motion_is_how_long_the_genie_plays`,
`state::tests::real_client::reflow_on_close::minimize::config_minimize_spread_is_the_genies_spread`,
`state::tests::real_client::reflow_on_close::minimize::config_minimize_slot_is_the_strip_the_window_goes_into`)
([Your own animation feel](#your-own-animation-feel) times them).
`sol.keep_above(id, "toggle")` keeps a window over every window that is not,
clicks included, until it is asked again. A frame's Keep Above button sends a
name of its own with `Solium.send`, your `sol.on("frame")` listener calls
`sol.keep_above(id, "toggle")`, and the frame is told `keptAbove` to show it
is on. A focused fullscreen window still covers it. Rebind any of them in
`bindings`:

```lua
return {
    bindings = {
        ["super+ctrl+h"] = function() sol.focus_direction("left") end,
        ["super+alt+k"]  = false,
    },
    tiling = { move = "split" },
}
```

In tiling a move trades tiles with the neighbour level with it that way, and
in a grid the opposite key puts both back; `move = "split"` makes it split the
neighbour's tile instead, as Hyprland's `movewindow` does. [modes.md](modes.md#focus-and-move-by-direction)
has what each layout does with a direction.

### The pointer follows the keyboard

Moving focus with a key -- `super+arrows`, a workspace key, a move -- takes the
pointer to the window that took the keyboard, and its picture glides there.
The pointer itself is there at once, so whatever it is over is the new
window's from the first instant; moving the mouse while it glides carries on
from there, or from where you see the picture with `interrupt = "picture"`. A
key that leaves the keyboard where it was, such as scrolling's `super+r`,
takes it nowhere. A click, a bar's window chip, a window opening or focus
following the mouse leaves it where your hand put it. All of it is
`focus.cursor`, by what moved the keyboard
([modes.md](modes.md#what-a-mode-is-told) lists every source):

```lua
return {
    focus = {
        cursor = {
            follows = { binding = false, surface = true }, -- not for workspace keys, but for a bar's chip
            to = { x = 0.5, y = 0.25 },                    -- a quarter of the way down the window
            motion = { easing = "spring" },                -- or false, to be there at once
            interrupt = "picture",                         -- the mouse mid-flight goes on from the picture
            refocus = true,                                -- super+r in scrolling recentres it too
            empty_workspace = "stay",                      -- a key onto an empty workspace leaves it
        },
    },
}
```

`lua/follow.lua` is the whole of the policy, and `sol.warp` the compositor's
part of it ([modes.md](modes.md#taking-the-pointer-somewhere)). #220.

### Click, hover and an empty desktop

A press on empty desktop or the wallpaper clears keyboard focus, so a window
you clicked away from stops taking what you type. Turn that off in `focus`:

```lua
return {
    focus = { clear_on_empty_click = false },
}
```

A press on an external layer-shell bar or panel does not clear the previous
window's keyboard focus when the surface declines keyboard focus. This case
remains outside `focus.clear_on_empty_click`.

Whether moving the pointer over a window focuses it is a per-mode question,
not a single on/off switch: floating -- the desktop, with no layout in charge
and windows free to overlap -- focuses on click only by default, because a
window you are not using sits under the pointer on the way to the one you
want. Tiling and scrolling follow the pointer, the same as every tiling
compositor people arrive from: their windows never overlap, so pointing at one
is choosing it. That shipped "follow" is this machine's own answer, though
(`SOLIUM_FORM_FACTOR`, [#159](https://github.com/Lilium-Linux/solium/issues/159)):
a tablet or a phone, with no pointer to hover, stays click-only in them.

Who decides, highest first: your own entry for the mode in `focus.modes`, then
your own `focus.follow` for every mode at once, then the shipped entries, then
the machine. So `focus = { follow = false }` turns hover off in every mode,
and an entry of yours turns it back on for one, on any machine. A mode named
nowhere gets the machine's answer, never the answer of the mode you switched
from:

```lua
return {
    focus = { modes = { floating = "follow", scrolling = "click" } },
    -- or: focus = { follow = false, modes = { tiling = "follow" } },
}
```

When the window you are typing into closes, the keyboard moves on by itself
as the close lands, and `focus.after_close` says where, per mode, as a list
tried in order (#128): `"pointer"`, the window under the pointer;
`"neighbour"`, the window that takes its place -- the tile that grows into
the space, the column that slides into it -- or the nearest to where it
stood; `"recent"`, the one you used before it on that monitor; `"topmost"`,
the topmost window on any screen. Tiling and scrolling are
`{ "pointer", "neighbour", "topmost" }` and floating is
`{ "recent", "topmost" }`. An empty list leaves the keyboard on nothing:

```lua
return {
    focus = { after_close = { tiling = { "recent", "topmost" } } },
}
```

`lua/modes.lua` is what turns all of this into `sol.focus_mode`, on every mode
change as well as at load, so a mode switch applies at once and a reload
keeps whichever model the session was already in.

### Your own mode

A mode is a Lua module that reacts to events — a window opening, closing or
taking focus, a drag ending, the monitors changing, and the rest
[modes.md lists](modes.md#what-a-mode-is-told) — and asks for placements.
`tiling.lua` is Hyprland's dwindle and `scrolling.lua` is niri's model; each
decides where windows go and when, and leaves the arithmetic of the tree and
the strip to `crates/layout`. Copy either into your own directory and it takes
over.

**[modes.md](modes.md)** is the guide, with a whole working mode in forty lines
and the two mistakes everyone makes first.

### An X11 helper that was never meant to be seen

Some autostart entries exist only so a desktop portal has something to talk
to — KDE's screen-sharing fallback, `xwaylandvideobridge`, is the one this was
found against (issue #221), and it is not the only X11 program shaped this
way. Solium hides a window like it by `config.x11.hidden`, a list of `WM_CLASS`
names matched case-insensitively against either field a client sets (`xprop
WM_CLASS` on a nested window shows both). A listed window is not tiled, not
decorated, and left out of `sol.windows()` — nothing a bar or a window list
built from that call ever has to filter out by hand — the same treatment
`xwayland.rs`'s `places_itself` already gives a menu or a tooltip, for a
different reason. The default names only `xwaylandvideobridge`; add your own
in `config.lua`'s `x11.hidden` for anything else shaped this way, keeping
`xwaylandvideobridge` in the list if you still want it hidden, since a
configured list replaces the default rather than adding to it.

**This used to key off `WM_HINTS.input` instead, and hid more than it meant
to.** ICCCM reserves `input: false` for a window that never wants the
keyboard, but it also names an entirely ordinary 'Globally Active' input
model — ordinary, ICCCM-conforming applications that set `input: false` and
take the keyboard themselves through `WM_TAKE_FOCUS` — and that old rule hid
every one of them too. `WM_CLASS` is a much narrower claim: a name on `hidden`
says "this specific program", not "anything that declines this one hint".

It is still a real client with a real process, but nothing in the shipped
config can reach it to ask it to go. The shipped `super+q` only walks
`sol.windows()` looking for a window with `focused == true`, and a window
hidden this way never gets a row there at all to be found: the snapshot that
builds `sol.windows()` drops an unmanaged pane before it becomes a
`WindowInfo` (`state/snapshot.rs`), and the unmanaged path this window takes
(`state/open.rs`'s `take_unmanaged_pane`) never grants it keyboard focus
either, so `focused` could not be true for it regardless. A hung window of
this shape cannot be stopped from inside the compositor today — only from
outside it, with an ordinary `kill`, or by not letting it start in the first
place, the way just below.

The kill-after-silence mechanism `state/close.rs` added alongside this one
(`GRACE`, `x11_refused_once`) is real, and it does help — for an ordinary,
listed, managed X11 window that stops answering `WM_DELETE_WINDOW`, which is
exactly what `super+q` can still reach the normal way. It was never reachable
for this one.

Nothing here stops the autostart entry from starting in the first place —
that is Fedora's `/etc/xdg/autostart/org.kde.xwaylandvideobridge.desktop`, not
Solium's to own — so if you would rather it never ran at all, override it per
session the way any `XDG_CONFIG_DIRS` autostart entry is overridden: a copy
under `~/.config/autostart/` with `Hidden=true` added.

    mkdir -p ~/.config/autostart
    cp /etc/xdg/autostart/org.kde.xwaylandvideobridge.desktop ~/.config/autostart/
    printf 'Hidden=true\n' >> ~/.config/autostart/org.kde.xwaylandvideobridge.desktop

A copy with `Hidden=true` is what every spec-following autostart reader
treats as "do not start this one" — the original in `/etc/xdg/autostart` is
untouched, so a package update to it never undoes the override.

## Worth knowing

Each layer of a frame is drawn over its canvas: the window's outer rect, grown
by whatever `bleed` the layer declares. On the hardware that is on the GPU by
default; nested, or where the GPU trial at startup fails, it is in software on
the CPU. Bars and borders are cheap because most of that canvas is untouched,
but a frame that paints across the entire window every frame will cost you, so
favour transitions that settle over ones that loop forever.

A layer is drawn again only when Qt marks its scene dirty — something it
renders changed — or while an animation in it is running. So an idle window
costs a flag read, not a rasterisation. A decoration that animates
continuously (`pulse` does, while focused) is drawn on every frame for as long
as it animates. The pane styles' README has the rest, under
[What it costs](../crates/solium/qml/panes/README.md#what-it-costs).

## Editor completion

Solium ships a definitions file for
[lua-language-server](https://luals.github.io/),
[`lua/meta/sol.lua`](../crates/solium/lua/meta/sol.lua), which describes every
`sol.*` function with its arguments. Point the server at it and your editor
completes `sol.` and checks what you pass. A `.luarc.json` in
`~/.config/solium`, beside your `user.lua`:

```json
{
    "runtime.version": "Lua 5.4",
    "workspace.library": ["/usr/share/solium/lua"]
}
```

That is where a package puts Solium's Lua. `dev/install.sh` puts it in
`~/.local/share/solium/lua`, and in a checkout it is `crates/solium/lua`; write
the whole path either way. Naming the `lua` folder rather than `lua/meta`
also lets the server follow `require("modes")` and the other shipped modules.
The same file is the Lua API page of the documentation site.
