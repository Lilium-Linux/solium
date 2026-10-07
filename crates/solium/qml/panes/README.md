# Pane styles

A pane is one window and its decoration. A **style** is the folder that says
what that looks like: a `Pane.qml` naming what the frame reserves and a list of
layers, each its own QML scene, each at its own depth.

There is nothing to compile and no compositor code to touch: write a folder,
name it in your configuration, and it draws every window.

    -- ~/.config/solium/user.lua
    return { pane = "left" }

A name is one of the folders here, or one of your own in
`~/.config/solium/qml/panes/` -- yours shadows a shipped one of the same name.
A path is anywhere. `SOLIUM_PANE=left` does the same thing for one run, which is
the quicker way to try one.

Press **super+shift+r** and the running session picks up the change: the
configuration is read again, the QML cache is dropped, and every pane is
rebuilt. Windows keep their slots, and each client is resized to whatever the
new style left it.

Everything the shipped layers draw with -- colours, fonts, spacing -- comes
from `Solium.Theme`, the same singleton the loading window reads, and a hosted
shell that imports `Solium` can read it too. The fallback pointer and the
default wallpaper do not: their colours are fixed. Nor, in part, does the
keyboard pill: its capsule is `Theme.accent`, but its glyph and label are fixed
white and its shadow fixed black. A `Solium/Theme.qml` of your own in
`~/.config/solium/qml/` is meant to restyle all of them without touching
anything that ships, and does not yet, because the shipped module is found
first ([#88](https://github.com/Lilium-Linux/solium/issues/88)).

## The manifest

`Pane.qml` is the entry point, and its root is a `PaneStyle`. It declares
structure; it never draws. The compositor builds it once at 1x1, reads what it
says, and throws it away.

```qml
import QtQuick
import Solium

PaneStyle {
    insets.top: 32
    requires: []

    Layer { depth: "behind"; bleed: 24;            Rectangle { anchors.fill: parent } }
    Layer { depth: "frame";  source: "Frame.qml" }
    Layer { depth: "above";  bleed: { "top": 48 }; source: "Spikes.qml" }
}
```

| on `PaneStyle` | |
|---|---|
| `insets.top`, `.right`, `.bottom`, `.left` | what the style reserves from the client, **once, for the whole style** |
| `requires` | what the style needs from the machine. `["gpu"]` is the only term today: the software scene graph does not implement `ShaderEffect`, and `Canvas` does not appear to paint on it, so a style using either says so. A style whose terms this session cannot meet, or that names one this build has never heard of, is refused rather than drawn wrong: its windows are left bare, and the log names the term. A nested session renders in software under the default `qml.renderer = "auto"` |
| `client.radius` | rounds the client's own surfaces, in logical pixels. The compositor cuts the corners as it draws the client, at no extra cost: no pass and no copy of the window (see "What it costs"). `0` is no effect at all, and so is leaving the key out — which is what nine of the eleven bundles that ship do. The example fixture writes `0`, to show the key exists and costs nothing; `rounded/` and `flush/` are the two that round |
| `client.radiusTopLeft`, `.radiusTopRight`, `.radiusBottomLeft`, `.radiusBottomRight` | one corner each, in logical pixels. Every one of them defaults to `client.radius`, so a style that wants four the same writes one key and these never come up. **A `0` has to be written out**: squaring a corner is half of what these are for, so an absent corner follows `radius` rather than being square. `flush/` is the shipped example |
| `client.shadow` | reserved, and read by nobody: a shadow is an effect rule in the style's `effects.lua` (below) |
| the `Layer` children | the layers, in declaration order |

Insets are on the style and never on a layer. The client is placed once and
every layer sees the same client rect, so three layers each declaring insets
would be three answers to one question.

A folder without a `Pane.qml` is not a style. A bare name skips it and goes on
to the next place it would have looked, so an empty `panes/top/` of your own
does not quietly replace the shipped `top` with a window that has no frame.

In a checkout of the repository, `crates/solium/tests/fixtures/panes/example/`
is the format written out in full -- all three depths, both spellings of
`bleed`, inline content and delegated content -- and

    solium --check-qml crates/solium/tests/fixtures/panes/example/Pane.qml

says whether it still parses. It loads every layer the style names in
`source:` too, prints `ok` for each file or what Qt reported, and exits with 1
when any of them does not build, naming the file.

A style that cannot be loaded -- a layer that will not build, a `requires`
that cannot be met, a name that is nowhere -- leaves each window it was for
with no frame at all and no space reserved for one, and says so in the log
(#90). Fixing it and pressing **super+shift+r** frames the windows opened after
that; one already left bare stays bare until it is opened again.

## Layers

| on `Layer` | |
|---|---|
| `depth` | `"behind"` the client, in the `"frame"`, or `"above"` the frame. Both `frame` and `above` are drawn over the client, `above` over `frame`. A string, so a fourth value added later does not break every style already written. An unknown word draws at `frame` and says so in the log |
| `bleed` | how far past the pane this layer may paint. `24` for every side, or `{ "top": 48 }` for one. Zero by default |
| `source` | a QML file in this folder, when the content is not inline |
| `name` | for diagnostics: which layer a warning is about |
| `dormant` | `true` while the layer has nothing to draw. False unless bound. Read on an inline layer's `Layer`; a layer with `source:` declares `property bool dormant` on its own file's root instead, and one written on its `Layer` is ignored. See "What it costs" |

Content is written inline or delegated with `source:`, and the syntax does not
change between the two. Each layer is rasterised into a scene of its own and
becomes its own element in the frame, which is what lets the client's surface
sit *between* two layers of one style -- the thing a single QML file can never
do.

Layers at one depth are drawn in declaration order, the later one on top.
Pointer input goes to every layer. When more than one layer sets `action` for
the same press, the topmost is the one acted on.

A delegated layer's root is an ordinary `Item`. It is its own scene, with no
parent to read anything off, so it declares the properties it uses itself.

## What the compositor sets on every layer

Every frame:

| property | |
|---|---|
| `title` | the window's title |
| `focused` | whether it has the keyboard |
| `pointerInside` | whether the pointer is anywhere over the window |
| `contentWidth`, `contentHeight` | the client's size, inside the insets |
| `paneWidth`, `paneHeight` | the window's own outer size |

When they change, in place, and to a frame built later as well:

| property | |
|---|---|
| `caret` | where the focused text field's caret is, when this window has it: `{ valid, x, y, width, height }` in the pane's own space -- the space a layer with no bleed is laid out in -- and `valid: false` when the field goes or the keyboard leaves the window. Known from applications that say where their caret is, through `text-input-v3`. Exact while the window is at rest: it takes the client to sit just inside the insets, so while a resize places the client otherwise for a moment, it can be off by that much. Declared on `PaneStyle`; a delegated layer that wants it declares `property var caret: ({ valid: false })` |
| `values` | whatever the configuration handed every pane with `sol.pane_values{ key = value }`, as one object. Each call merges its top-level keys into what is there, a key's value replaced whole, so two scripts that hand keys of their own both reach the layer. A general channel from Lua to the frames: a setting the configuration reads reaches the layer that draws by it, and the compositor never knows what it is. A reload starts it empty, so a key the reloaded configuration no longer hands over is gone. Declared on `PaneStyle`; a delegated layer declares `property var values: ({})` |

A layer drawing at the caret is drawn with its window, so whatever the window
is doing -- moving, scaling in a thumbnail, fading -- the drawing does too.
Every layer can also read the keyboard, `Keyboard` in `import Solium`: the
live layout, its names, and whether Caps Lock and Num Lock are on
(`docs/shell-boundary.md`, "The keyboard, live"). The attached `Solium` object
and `Grab` are a hosted shell's. In a layer they build and do nothing, and
`Solium.monitor` reads an absent row with an empty `name`
(`docs/shell-boundary.md`, "The `Solium` QML module").

Once, at build, because neither ever changes for a style:

| property | |
|---|---|
| `insetTop`, `insetRight`, `insetBottom`, `insetLeft` | what `Pane.qml` reserved, so a bar can size itself to its own band without a second copy of the number |
| `bleedLeft`, `bleedTop` | where the window's own corner is inside this layer's canvas |
| `clientRadiusTopLeft`, `clientRadiusTopRight`, `clientRadiusBottomLeft`, `clientRadiusBottomRight` | what the compositor is cutting each corner of the client to, in logical pixels, so a bar or a border can hug that curve. Written on every layer of every style, zeroes included — a corner left unwritten reads back 0, which is indistinguishable from one the style really squared |
| `clientRadius` | the **largest** of the four above, for a layer that wants one number. Its use is an outward hug (`radius: clientRadius + 2`), and a hug has to clear the biggest cut; a layer that needs one particular corner reads it by name. Written on every layer of every style, zero included. `rounded/Frame.qml` is the worked example |

Of these, `PaneStyle` declares only `bleedLeft` and `bleedTop`. The four
`inset` names and the five `clientRadius` names reach only a delegated layer
that declares them on its root. An inline layer reads its manifest instead:
`insets.top` and the other three, `client.radius`, and `client.radiusTopLeft`
and the other corners, where `-1` means the corner follows `client.radius`. It
has no `clientRadius`.

Read back by the compositor:

| property | |
|---|---|
| `action` | set to `"close"` or `"maximize"` to ask for it; cleared once taken |
| `onButton` | `true` while the pointer is over a button. A press on the frame starts a window drag unless some layer says this |
| `dormant` | `true` while the layer has nothing to draw, so it is not drawn, and in software keeps no image. An inline layer binds it on its `Layer` and `PaneStyle` hands it on; a delegated layer declares `property bool dormant` on its own root |

A layer declares only the ones it uses; one that positions nothing against the
window declares no `bleedTop` and is handed a property it ignores.

A layer whose buttons do not declare `onButton` has them drag the window
instead of pressing. The shipped `top` keeps the name of the button under the
pointer in a `hovered` string of its own and derives the flag from it:

    property string hovered: ""
    readonly property bool onButton: hovered !== ""

`hovered` on its own is read by nothing. `left`, `bottom`, `pulse`, `reactive`
and `reveal` declare `hovered` and not `onButton`, so their buttons may not
press ([#158](https://github.com/Lilium-Linux/solium/issues/158)).

The pointer arrives as ordinary mouse events, in the layer's own coordinates,
while it is anywhere over the window -- including over the client, which is how
a border can follow the cursor. When it leaves, the layer is told with a
position outside itself, so `MouseArea.containsMouse` goes false on its own. A
layer with bleed is given the pointer in **its own canvas**, so a click lands on
whatever that layer drew there. **Presses are narrower than hover**: they reach
the layers only inside the band the insets reserve, so a button drawn over the
client, out in the bleed, or in a style that reserves nothing cannot be
pressed. Touch reaches no layer: a finger on a frame's close or maximise
button, or on its edge, presses nothing and resizes nothing
([#181](https://github.com/Lilium-Linux/solium/issues/181)), though outside
the `tablet` and `phone` form factors the tap still focuses the window.

## What it costs

QML renders on one of two paths: the GPU, the default on the hardware when a
trial render at startup passes, or software, which is what a nested session,
`qml.renderer = "software"` and a failed trial get. The log line that begins
`QML renderer:` says which.

On both, `anchors.fill: parent` fills the **canvas**, which is the pane grown
by the bleed this layer declared -- not the window. Bleed is a cost and not a
permission: the canvas is that much larger, and every pixel of it is drawn
again when the layer changes. A bar throwing spikes upward should ask for
`{ "top": 48 }` rather than `48`, and not pay for three sides it never touches.
It is also a promise rather than a request: the layer is clipped to the canvas
it asked for, so no one style can force a full-screen repaint every frame.

On the GPU a layer is drawn straight into a buffer the compositor allocated,
and nothing is copied or uploaded.

In software a changed layer is rasterised on the CPU, copied and uploaded. A
layer that stays inside the style's own insets has only those bands copied,
and the upload is the one box around them: for a titlebar that is about 4% of
a window, and for a style that reserves all four sides it is the whole window.
A layer that reserves space **and** paints outside it must say so, or what it
paints outside is not copied:

    property bool overlay: true

Declare it only if you need it; the alternative is usually to reserve the
couple of pixels you were painting over. A layer at `behind` or `above`, a
layer with any bleed, and every layer of a style that reserves nothing are
overlays already and need not say it. On the GPU the property changes nothing.

**A layer that is usually empty can say so.** Every layer costs what its
canvas costs whether or not anything is in it: a buffer that size, and an
element blended over the client wherever the client changes, and in software
the whole canvas copied and uploaded at every step of a resize. A layer that
binds `dormant` to "nothing to show" is left out of the frame while it is
true, so nothing of it is blended or uploaded, and on the frame it goes to
sleep it lets go of its image, so waking is a resize that draws it afresh. On
the GPU its scene keeps the buffer it was last drawn into until it is drawn
again, and one dormant from its first frame never has one larger than a
pixel. It is a general mechanism -- any layer may bind it, and nothing in
the compositor knows what a layer is for -- and the keyboard pill below is
the shipped user. Wake it from something it is told (`values`, `caret`,
`focused`): a dormant layer's own animations ask for no frames.

**A rounded client costs no pass.** `client.radius` is drawn as the client is:
each of its surfaces goes through a fragment program where it already is, so
`rounded/` and `flush/` cut the client's corners at no extra cost, with no pass
and no copy of the window, and keep them while the window is tilted or pulled
by a genie. A rounded window that fades in or out is faded surface by surface,
with no picture of it taken: where a subsurface overlaps its parent (a
player's video over its window, say) the two show through each other while the
window is translucent. An opaque window looks as it always did.

Animations need nothing declared. Qt is asked each frame whether the scene has
anything new to draw, and the compositor draws only then -- so an idle layer
costs a flag read, a transition runs at the screen's refresh rate, and a loop
with a pause in it survives the pause. Bear in mind only that a layer which
never stops animating never stops costing anything. The one measurement, from
2026-09-06 on the software path before the GPU one existed, is the whole
compositor at about a tenth of a core with `pulse` animating at the screen's
full rate. Bind an endless animation to `focused`, as `pulse` and the `wave`
demonstration do, and an unfocused window costs nothing.

## effects.lua

A style folder may hold `effects.lua` beside its `Pane.qml`: a list of effect
rules for the windows this style frames, and for no others.

```lua
-- ~/.config/solium/qml/panes/mine/effects.lua
return {
    { match = "*", part = "pane", slot = "behind", effect = { "glow", spread = 24 } },
}
```

Each rule is the rule `effects.rules` takes in your configuration, with every
key it has ([the effects reference](../../effects/README.md#rules)), and the
effects it names are effect folders, yours or the shipped ones: a style ships
rules, not effects of its own. Your own `effects.rules` win over a style's,
for the same part and slot. A window the style does not frame gets none of
them: a fullscreen window, a window that draws its own decorations, and every
window under `pane = "none"` are bare, and only your own rules reach those
(`match = { style = "none" }` names them).

The file runs in an effect's Lua, not your configuration's: `math`, `table`
and `string`, and no `sol`, no files and no `require`, stopped if it runs
longer than 100 ms. It is read when the style is applied (at startup, at each
`sol.pane`, at every reload), once for as long as it does not change, and
every effect it names is loaded and every rule bound then. What is wrong with
it is named in the top-right corner of your primary monitor, under
`style:<folder>` and at `effects.lua`'s line: a file that does not run keeps
the rules that ran before it, and a rule that cannot be bound (a key
misspelled, an effect nobody ships, a param it does not have) is named by its
number, its slot is left empty, and the rest run. `solium --check` reads the
`effects.lua` of each style of your own and fails on the same things.

As with your own rules, this build reads, checks and binds them, and does not
draw an effect in its slot yet.

## What is here

| folder | |
|---|---|
| `top/` | the default: a titlebar above the window |
| `left/` | a vertical titlebar down the left side |
| `bottom/` | a titlebar underneath |
| `border/` | no bar at all, just a frame around the window |
| `reactive/` | a border that lights up where the cursor is, with a bar |
| `proximity/` | a border that answers the pointer entering and leaving |
| `reveal/` | a bar that slides out of the window's edge on approach |
| `pulse/` | a bar with an animation running in it |

`none` is not a folder: `pane = "none"` draws no frame and builds no scene per
window.

Each of those eight is one `frame` layer, which is what every decoration was
before styles had layers, and the keyboard pill's layer, described below. The
rest are here to show what layers add:

| folder | |
|---|---|
| `rounded/` | `client.radius`: the compositor cuts all four of the client's corners with a fragment program, and the bar -- at `behind`, reaching `clientRadius` past its band -- shows through the two it cut inside the window |
| `flush/` | the same seam the other way up: `radiusTopLeft` and `radiusTopRight` at `0`, so the client's top is square, the bar's own rounded top is the window's top, and the two meet flat. The only shipped bundle whose four corners differ |
| `shadow/` | `behind` plus `bleed`: stacked rectangles standing in for a blur |

And five demonstrations, which are not shipped and are never offered by name.
They live with the test fixtures in a checkout, in
`crates/solium/tests/fixtures/panes/`, and `SOLIUM_PANE=<that path>/<name>`
puts one on every window:

| folder | |
|---|---|
| `example/` | the format written out in full, and the fixture the tests build |
| `sandwich/` | one layer behind the client and one above it, in colours that cannot be confused |
| `wave/` | sine waves flowing round the whole window, outside it, all `bleed` at `behind` and nothing reserved |
| `bleedy/` | what bleed does to hit-testing, and to the window next door |
| `frosted/` | a translucent bar, and an `effects.lua` putting an effect behind the titlebar |

## The keyboard pill

Every style here ends with one line:

```qml
PaneStyle {
    Layer { depth: "frame"; source: "Frame.qml" }
    KeyboardPillLayer {}
}
```

`KeyboardPillLayer`, in `import Solium`, is an inline layer at `above` that
draws `KeyboardPill` -- the small capsule saying Caps Lock is on, or which
layout you just switched to -- just below the pane's `caret`, or above it
when the pane has no room below. It shows what the configuration hands it,
`values.keyboard_indicator`, and nothing when that says not to: the policy is
`lua/keyboard_indicator.lua`, set by `keyboard.indicator` in `config.lua`,
and `docs/ricing.md` has the whole of it. A style of your own gets the pill by
adding the same line, and a style without it has none. A window drawn bare --
fullscreen, one drawing its own decorations, or every window under
`pane = "none"` -- has no style around it at all, and the configuration draws
its pill on a surface instead.

`KeyboardPill` shows what its `cue` says and decides nothing. `cue` is
`{ what, serial, hold, duration, after }`. `what` is `"caps"` (an outlined
⇪), `"layout"` (`Keyboard.layoutShort`), `"num"` (⇭), or `""` to hide at
once. A cue is taken only when its `serial` is new, so handing the same cue
again shows nothing. With `hold: true` it stays until the next cue. Otherwise
it hides after `duration` milliseconds (1200 when not given), or, when `after`
names one of the three, shows that one instead, held. While `accepts` is
`false`, a new cue hides it rather than showing. It is the capsule, 28 high and
at least 32 wide, in `Theme.accent`, with a `margin` of 14 on every side for
its shadow, and it centres the capsule in itself. `KeyboardPillLayer` hands it
`values.keyboard_indicator`, which the configuration sets as `{ show, cue }`,
and draws only while `show` is `true` and the pane's `caret` is `valid`.

It is a layer of its own because it draws over the client, and in software a
`frame` layer that reserves a band copies only that band. So it costs one more
scene per window. While no pill is on show it is `dormant`, which is nearly
always: not drawn, nothing blended over the client, and in software no image
kept. It is drawn only from the cue that shows a pill until that pill has
faded. A style that leaves the line out pays nothing for it at all.
`crates/solium/tests/scenarios/keyboard-pane-drawn.lua` draws it in `top`,
reads its pixels, and reads when it is dormant.

## One QML file is still a decoration

A style can also be a single `.qml` file with an `Item` at its root, under
`~/.config/solium/qml/decorations/`, named the same way. It is one layer at
`frame`, and it declares its own `insetTop`/`insetRight`/`insetBottom`/
`insetLeft` rather than being told them -- a single file has no manifest, so it
is the only place those numbers can live. It has no manifest to add
`KeyboardPillLayer {}` to either, so with `keyboard.indicator.show = "pane"`
its windows show no keyboard pill: draw one at `caret` yourself (with
`property bool overlay: true`, since it paints over the client), or set
`show = "surface"`. Nor has it a folder to keep an `effects.lua` in, so it
gives its windows no effect rules.

Nothing ships as one any more: the eight above were single files until the
pane-styles work moved them into folders. The path stays because those files
exist on people's machines, and because a style needing neither layers nor
bleed should not have to become a folder to say so.
