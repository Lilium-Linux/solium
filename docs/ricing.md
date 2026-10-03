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
| your colours and fonts | `~/.config/solium/qml/Solium/Theme.qml`, once [#88](https://github.com/Lilium-Linux/solium/issues/88) is fixed |
| the shipped files, to copy from | `crates/solium/qml` and `crates/solium/lua` in a checkout; `share/solium/qml` and `share/solium/lua` under the prefix of an install, which for `dev/install.sh` is `~/.local` |
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

`interactive = true` lets the pointer reach it, where its items take input:
a `MouseArea`, a pointer handler, or an item marked `Solium.input: true`.
Everywhere else the pointer goes to what is under it
(`state::tests::real_client::reflow_on_close::hosted::a_press_where_the_shell_draws_nothing_reaches_the_window_under_it`).
A surface without it holds neither a `Grab` nor the keyboard
(`state::tests::real_client::reflow_on_close::hosted::a_surface_the_pointer_does_not_reach_holds_no_grab`,
`state::tests::real_client::reflow_on_close::hosted::a_surface_the_pointer_does_not_reach_holds_no_keyboard`).
The scene sets an `action` string, the compositor takes it, and whoever is
listening is told:

```lua
sol.surface("panel", { scene = "panel.qml", layer = "overlay",
                       on = area, interactive = true })

sol.on("surface", function(name, action)
    if name == "panel" then
        sol.log("pressed " .. action)
    end
end)
```

That is the whole of how the Developer Tweaks panel works, and it is entirely
in `lua/tweaks.lua` — the compositor has no idea what a tweak is.

`reserve = { bottom = 48 }` takes those edges out of the work area of every
monitor the surface is on, whatever its size, so a bar scene drawn across the
whole monitor reserves only its strip and the windows are placed above it. The
scene can say it too, `Solium.surface.reserve.bottom: 48`, which wins for each
edge it sets; [shell-boundary.md](shell-boundary.md), "Room of its own", has
how the windows re-flow when it changes
(`state::tests::real_client::reflow_on_close::hosted::a_declared_reserve_takes_its_edge_out_of_the_work_area`).

**Place things on the `monitors` event, not at the top of your script.** Scripts
load before the screens are known — on the hardware backend, before the GPU is
even opened — so a rect computed at load time is computed against zeros. The
event fires once when the monitors are first known, again on every hotplug and
again on every reload, which is when a placement needs redoing anyway:

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

Switch layout, or press Caps Lock, and a small capsule in the accent colour
appears just below where you are typing: `⇪` while Caps Lock is on, `EN` or
`RU` for a moment after a switch, after the one macOS shows.

It is not a compositor feature. It is the shipped configuration's example of
doing something with what the compositor publishes, and it is two ways of
doing the same thing, so you can see both:

- **Inside the window** (`show = "pane"`, the default). Every shipped pane
  style ends with one line, `KeyboardPillLayer {}`, a layer above the client
  that draws the pill at the pane's `caret` — where the focused text field's
  caret is, in the pane's own space. Because it is part of the pane, it moves,
  scales and fades with its window. Your own style gets it by adding the same
  line. A window drawn with no frame — fullscreen, or one that draws its own
  decorations — has no pane style around it, so it gets the surface below at
  its caret instead: `sol.text_input()` says which, as `framed`.
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
events — and knows nothing about pills. It is configured in `config.lua`:

```lua
keyboard = { indicator = {
    show = "pane",            -- "pane", "surface", or false
    on = { layout = true, caps = true, num = false },
    caps_on_focus = true,     -- Caps' pill again when a field is focused with Caps on
    fallback = "surface",     -- with no caret: on the screen, or false
    position = "bottom",      -- where on the screen: "bottom", "center" or "top"
    duration = 1200,          -- ms a layout's pill stays
} }
```

The caret comes only from applications that say where it is through
`text-input-v3`; for the others, `fallback = "surface"` shows the pill on the
focused window's monitor instead. To try it nested, `SOLIUM_KEY_AT` presses
Caps Lock and a layout switch for you ([`dev/README.md`](../dev/README.md)).
To change the policy, copy `lua/keyboard_indicator.lua` next to your
configuration, where it is found first. To change the on-screen pill's look,
copy `qml/indicator/keyboard.qml` to `~/.config/solium/qml/indicator/`; a
pane style of your own can draw a pill of its own at `caret` in place of
`KeyboardPillLayer {}`. `KeyboardPill` itself is in the shipped `Solium`
module, which a copy cannot replace yet
([#88](https://github.com/Lilium-Linux/solium/issues/88)). To have none, take
`require("keyboard_indicator")` out of your `init.lua`.

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

A bar can also be an ordinary client over `wlr-layer-shell`, which means any
panel already written for that protocol works. A surface names the output it
wants and the compositor honours it, so a bar on every screen is one surface
per screen, each reserving from *that* monitor's work area. One that names no
output gets the primary monitor.

A fullscreen window in front of the workspace its monitor is showing covers
the top layer -- a client's bar, and one declared with `sol.surface` -- and
takes the clicks where the bar was, while the overlay layer (notifications, an
OSD, a launcher) stays over it. To keep the bars over fullscreen windows:

```lua
return { fullscreen = { covers = "none" } }
```

See **[shell-boundary.md](shell-boundary.md)** for why hosting is the design,
and for both ways a shell attaches.

### Your own colours

`Solium.Theme` (`crates/solium/qml/Solium/Theme.qml`) is what every shipped
frame and the loading window are drawn with, and a hosted shell that imports
`Solium` can read it too. The fallback pointer and the default wallpaper do
not: their colours are fixed. A copy of `Theme.qml` in
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
back. Rebind any of them in `bindings`:

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
