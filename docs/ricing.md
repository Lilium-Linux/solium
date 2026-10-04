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

It also reports the quieter failure: a setting the defaults do not define.
`tilling = { split = 0.6 }` is perfectly good Lua and merges in perfectly
cleanly, and then nothing ever reads it — so `--check` names it, suggests what
you probably meant, and exits non-zero.

```
  2 unrecognised setting(s) -- written, merged, and read by nothing:
    open.scal                   did you mean open.scale?
    tilling                     did you mean tiling?
```

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

Eleven styles ship, and
[the pane styles' README](../crates/solium/qml/panes/README.md#what-is-here)
lists them with what each one is for. Reload and every open window is
re-framed.

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
declares as `property var values: ({})`; and it can read the keyboard's layout
and locks as the `Keyboard` singleton. That is how `KeyboardPillLayer {}` draws
the keyboard pill: the panes README's
[What the compositor sets on every layer](../crates/solium/qml/panes/README.md#what-the-compositor-sets-on-every-layer)
has each of them.

### Rounded corners

```qml
// Pane.qml
client.radius: 12
```

A style may round the client's own corners. It is the one effect that reads the
window's *own* pixels, so a pane that declares it is rendered to a texture first
and then drawn back through a fragment program — one extra pass per frame, for
that window only. `client.radius: 0` is no effect at all rather than a radius of
nothing, so a style that does not want it pays for none of this, and nine of
the eleven shipped bundles do not want it.

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
around a *square* client sets only its own `radius` and buys no pass at all.

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
hotplug and again on every reload, which is when a placement needs redoing
anyway. A surface whose `on` is `"every-monitor"`, `"primary"` or a connector
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
built-in one off so the two do not both do it:

```lua
return { idle = { screens_off_after = 0 } }
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
- the running windows of this monitor's current workspace, as chips -- title
  or app id, the focused one highlighted -- and a click focuses one;
- the keyboard layout, once more than one is configured;
- a clock.

Next pieces -- the dock, the island, search, quick settings, a tray, and
previews -- need services this compositor does not have yet, and are not
here. `preview.lua` and `qml/preview/` are where all of it lives, so copying
one file still changes one behaviour, exactly as **[Your own
frame](#your-own-frame)** and **[Bars, docks and
wallpapers](#bars-docks-and-wallpapers)** above do it.

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
(`models::pointer::tests::a_configured_scene_hears_every_named_shape`). A
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
last picture is stretched until it has drawn one at that size — through the
glide and after it, for a quarter of a second past the landing at most, and
then the window is shown at whatever size the application has. Once it has
answered the window is drawn as it is, with nothing in between, so a
fullscreen game or video can be shown directly. An instant change is just that:
nothing is stretched or moved, and the window is drawn as it is on the next
frame. An app id is what `sol.windows()` calls `app_id`.

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
back, and fullscreen and maximised glide there and back
([Your own animation feel](#your-own-animation-feel) times them). Rebind any
of them in `bindings`:

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
