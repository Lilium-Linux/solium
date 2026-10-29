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
shell that imports `Solium` can read it too. The shipped theme is dark and has
no hue in it: near-black bars, light grey titles, grey buttons, and an
`accent` that is a light grey too. The fallback pointer and the default
wallpaper do not read it: their colours are fixed. Nor, in part, does the
keyboard pill: its capsule is `Theme.accent` and its glyph and label
`Theme.accentInk`, but its shadow is fixed black. A `Solium/Theme.qml` of your
own in `~/.config/solium/qml/` is meant to restyle all of them without
touching anything that ships, and does not yet, because the shipped module is
found first ([#88](https://github.com/Lilium-Linux/solium/issues/88)).

The styles with buttons -- `glass`, `top`, `bottom`, `left`, `reveal`,
`reactive` and `pulse` -- tell close from maximise without a colour: under the pointer each
turns its own grey (`Theme.danger`, the lighter, and `Theme.warning`) and shows
its glyph on it in `Theme.accentInk`, `×` to close and `+` to maximise.
`crates/solium/tests/scenarios/pane-buttons-drawn.lua` points at each and
reads the pixels.

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
| `client.radius` | rounds the client's own surfaces, in logical pixels. The compositor cuts the corners as it draws the client, at no extra cost: no pass and no copy of the window (see "What it costs"). `0` is no effect at all, and so is leaving the key out — which is what nine of the twelve bundles that ship do. The example fixture writes `0`, to show the key exists and costs nothing; `glass/`, `rounded/` and `flush/` are the three that round |
| `client.radiusTopLeft`, `.radiusTopRight`, `.radiusBottomLeft`, `.radiusBottomRight` | one corner each, in logical pixels. Every one of them defaults to `client.radius`, so a style that wants four the same writes one key and these never come up. **A `0` has to be written out**: squaring a corner is half of what these are for, so an absent corner follows `radius` rather than being square. `flush/` and `glass/` are the shipped examples |
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

**A layer keeps the pane's opacity while the client fades on its own.**
`sol.present`'s `opacity` fades the whole pane, layers and all, and its
`client_opacity` fades only the client's content inside it. A layer at
`behind` that draws the app's icon (it has `appId`) is then what the window
turns into as it flies home, and what a launch grows out of, because the pane
is one thing on screen the whole way. One limit, #297: while a pane is warped
(a turn or a deform), an effect on the client drawn beneath the pane erases
the layers under it at any `client_opacity`.

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
| `appId` | the window's application id, as `sol.windows()` and a rule's `app_id` read it: its xdg `app_id`, or an X11 window's class; empty while its application has not arrived. A bar shows the application's icon by it, `image://solium/icon/<appId>?size=16`, and hides the icon while `Image.status` is not `Image.Ready`. A window that went keeps its own while it fades |
| `focused` | whether it has the keyboard |
| `pointerInside` | whether the pointer is anywhere over the window |
| `contentWidth`, `contentHeight` | the client's size, inside the insets |
| `paneWidth`, `paneHeight` | the window's own outer size |

When they change, in place, and to a frame built later as well:

| property | |
|---|---|
| `caret` | where the focused text field's caret is, when this window has it: `{ valid, x, y, width, height }` in the pane's own space -- the space a layer with no bleed is laid out in -- and `valid: false` when the field goes or the keyboard leaves the window. Known from applications that say where their caret is, through `text-input-v3`. Exact while the window is at rest: it takes the client to sit just inside the insets, so while a resize places the client otherwise for a moment, it can be off by that much. Declared on `PaneStyle`; a delegated layer that wants it declares `property var caret: ({ valid: false })` |
| `values` | whatever the configuration handed every pane with `sol.pane_values{ key = value }`, as one object. Each call merges its top-level keys into what is there, a key's value replaced whole, so two scripts that hand keys of their own both reach the layer. A general channel from Lua to the frames: a setting the configuration reads reaches the layer that draws by it, and the compositor never knows what it is. A reload starts it empty, so a key the reloaded configuration no longer hands over is gone. Declared on `PaneStyle`; a delegated layer declares `property var values: ({})` |
| `windowValues` | whatever the configuration handed **this window's** frame with `sol.frame(id, { key = value })`, as one object, merged by key as `values` is: the window's own opacity for a quick-settings slider, a level, anything the configuration keeps per window. Gone with the window, and a reload starts it empty. Declared on `PaneStyle`; a delegated layer declares `property var windowValues: ({})` |
| `windowId` | the window's id, as `sol.windows()` gives it and `sol.on("frame")` hears it |
| `pid` | the window's process, as `sol.windows()` gives it: an X11 window's own, never Xwayland's; `-1` when it cannot be told |
| `maximized`, `fullscreen` | the window's state, as the compositor last set it, so the frame changes the moment it is asked and not when the application answers |
| `floating` | no layout holds the window in a tile: every window of a floating desktop, and a window a tiling layout let float. A maximised or fullscreen window is as it was before |
| `minimized` | the window is put away |
| `keptAbove` | the window is kept above the others, with `sol.keep_above`, so a Keep Above button shows that it is on |

`windowId` to `keptAbove` are declared on `PaneStyle` as `int` and `bool`
properties; a delegated layer declares the ones it reads, `property int
windowId: 0`, `property int pid: -1` and the rest. A window that went keeps
what its frame was last told while it fades.

A layer drawing at the caret is drawn with its window, so whatever the window
is doing -- moving, scaling in a thumbnail, fading -- the drawing does too.
Every layer can also read the keyboard, `Keyboard` in `import Solium`: the
live layout, its names, and whether Caps Lock and Num Lock are on
(`docs/shell-boundary.md`, "The keyboard, live"). A layer is a hosted scene
on no monitor: `Solium.send` and `Grab` work in it, as "Sending to Lua, and a
panel over the window" below says, `Solium.monitor` reads an absent row with
an empty `name`, and `Solium.surface.reserve` and `Solium.keyboard` hold
nothing (`docs/shell-boundary.md`, "The `Solium` QML module").

Once, at build, because none of them changes for a style:

| property | |
|---|---|
| `insetTop`, `insetRight`, `insetBottom`, `insetLeft` | what `Pane.qml` reserved, so a bar can size itself to its own band without a second copy of the number. `insetTop` is told again whenever the frame grows (`growTop`, below): it is always the top the frame reserves now |
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
| `action` | set to `"close"`, `"maximize"` or `"minimize"` to ask for it; cleared once taken. `"minimize"` puts the window away as `sol.minimize` does, so the configuration's `minimize` listener plays it |
| `onButton` | `true` while the pointer is over a button. A press on the frame starts a window drag unless some layer says this |
| `dormant` | `true` while the layer has nothing to draw, so it is not drawn, and in software keeps no image. An inline layer binds it on its `Layer` and `PaneStyle` hands it on; a delegated layer declares `property bool dormant` on its own root |
| `layerOpacity` | how opaque the compositor draws the layer's picture, from 0 to 1, over the window's own opacity; 1 when not declared. Applied as the layer is composited, so animating it draws no new picture: a fade by it is one blend per frame and nothing of Qt's, where a fade of an item's `opacity` draws the whole layer again on every frame of it. An inline layer binds it on its `Layer`; a delegated layer declares `property real layerOpacity` on its own root |
| `growTop` | how much more than `insets.top` this window's frame takes at the top just now, in logical pixels; 0 when not declared. The frame reserves `insets.top` plus the largest `growTop` of its layers, and every layer is told the sum as `insetTop`. Followed every frame, so a `Behavior` on it animates the window's content moving down. See "A titlebar that grows" below. An inline layer binds it on its `Layer`; a delegated layer declares `property real growTop` on its own root |
| `resizeHz` | how often, at most, the compositor draws the layer again for a new size while the window's size keeps changing (a drag on its edge, a layout's sweep), in times a second, from 5 to 1000; 0, or not declared, draws it at every size. Between, its last picture is drawn stretched to the window, as the application's own is until it has drawn at the new size, and the last size is drawn within a `1 / resizeHz` second of the window stopping. A layer with something new to draw (a title, a focus change, an animation of its own) is drawn at once. Told no new size while it is held, so a layer laid out by `paneWidth` lays out nothing it is not drawn at. An inline layer binds it on its `Layer`; a delegated layer declares `property real resizeHz` on its own root |

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
whatever that layer drew there. Each button is itself: a right press is
`Qt.RightButton`, so a `MouseArea` with `acceptedButtons: Qt.RightButton` opens
a menu on it, and a double press is a double-click. Only the left button drags
the window. The wheel over the band reaches a `WheelHandler`, and the client
hears nothing of it. A press the frame was given has the pointer until every
button is up, wherever it goes, with its release: a slider dragged off the
window follows the pointer. **Presses are narrower than hover**: they reach
the layers only inside the band the insets reserve, so a button drawn over the
client, out in the bleed, or in a style that reserves nothing cannot be
pressed -- unless a layer holds a grab over it, below. Touch reaches no layer: a finger on a frame's close or maximise
button, or on its edge, presses nothing and resizes nothing
([#181](https://github.com/Lilium-Linux/solium/issues/181)), though outside
the `tablet` and `phone` form factors the tap still focuses the window.

## Sending to Lua, and a panel over the window

A layer sends any named action, with data, to the configuration with
`Solium.send(name, data)` from anywhere in its QML, inline or delegated. Lua
hears it as `sol.on("frame", function(id, name, data) ... end)`, with the
window's id, in the order sent, in a dispatch after the one that sent it.
`"close"`, `"maximize"` and `"minimize"` are the built-in actions, as they
are for `action`, and are not handed to Lua. Lua answers with
`sol.frame(id, { key = value })`, which the layer reads as `windowValues`.

```qml
// The titlebar changes its application's volume, and the configuration
// keeps the window's opacity, which the frame shows on a slider.
WheelHandler { target: null; onWheel: (e) => Solium.send("volume", { by: e.angleDelta.y / 1200 }) }
Slider { value: frame.windowValues.opacity ?? 1
         onMoved: Solium.send("opacity", { value: value }) }
```

```lua
sol.on("frame", function(id, name, data)
    if name == "opacity" then
        sol.present(id, { opacity = data.value })
        sol.frame(id, { opacity = data.value })
    end
end)
```

A layer can drop a panel down over the window, quick settings for one, and
hold the pointer and the keys while it is open, with the same `Grab` a shell's
popup uses:

```qml
Item {
    id: panel
    y: frame.insetTop; width: frame.width; height: 160
    visible: frame.expanded
    Keys.onEscapePressed: frame.expanded = false
}
Grab { active: frame.expanded; target: panel; onDismissed: frame.expanded = false }
// and, where it opens: frame.expanded = true; panel.forceActiveFocus()
```

While a layer's grab is active:

- a press, a release, the wheel and hover inside the grab's target are the
  frame's, over the client as over the band, and the client is told none of
  it; a press there never drags the window;
- a press anywhere else, on the client, another window, the desktop or a
  bar, dismisses it (`onDismissed`) and is swallowed with its release, as a
  press outside a shell's popup is;
- every key no binding takes is the frame's, delivered to the item with its
  keyboard focus (give it `focus: true` or `forceActiveFocus()`), so Escape
  closes it; the window under it is told no key until it closes. Bindings,
  the compositor's own keys among them, still run;
- it ends when the grab is no longer active, and is dismissed when the window
  loses the keyboard, is minimized or goes, or the session locks.

One frame holds a grab at a time, as one shell scene does: a frame that
starts one dismisses the one another held.

## A titlebar that grows

A panel dropped over the window covers its content. A titlebar can instead
grow taller and push the content down, with `growTop`: the frame then
reserves `insets.top + growTop` at the top, for this window alone.

```qml
// Frame.qml, a delegated layer, under `insets.top: 32`
Item {
    id: frame
    property int insetTop: 32          // told: the top the frame reserves now
    property bool expanded: false
    property real growTop: expanded ? 120 : 0
    Behavior on growTop { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }

    MouseArea {                        // a right press on the bar opens it
        width: frame.width; height: 32
        acceptedButtons: Qt.RightButton
        onPressed: frame.expanded = !frame.expanded
    }
    Item {
        y: 32; width: frame.width; height: frame.insetTop - 32
        clip: true
        // ... the quick settings, in the room the bar has grown
    }
}
```

- **A floating window** grows taller: its top edge stays where it is and its
  content moves down by `growTop`, the size it was. The application is told
  nothing.
- **A window held in a size**, in a layout's tile or maximised, keeps its
  size, and its application is given `growTop` less height. While the value
  moves, its last picture is held in what is left, fitted as `resize.fill`
  says, and the application is configured once, on the first frame the value
  has stood still, so an animation does not resize it on every frame.
- **A fullscreen window** has no frame, so `growTop` does nothing there.

Everything follows the frame's real size from the frame it moves: the band a
press goes to the frame in (the grown part is the frame's, so a `MouseArea`
or a `Grab` target there is pressed and the application hears nothing), the
resize edges, `part = "region:titlebar"` and the slots around the pane, the
blur behind it, a capture, and the `x`, `y`, `w` and `h` of `sol.windows()`.
With several layers, the largest `growTop` wins; each is read whether its
layer is dormant or not. Shrinking it back to 0 gives the room back the same
way.

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

A rule's `part = "region:titlebar"` is read from the insets: the band along
the one side the style reserves most for, `top/`'s 32 pixels across the
whole window above the client, its two outer corners rounded as the
client's largest, or `left/`'s 34 down its side, square. A style that
reserves its most on two sides or more (`border/` and `proximity/`, the same
all round) or nothing at all has no titlebar region. Its slots go around
the style's layer named `bar`, or its first `frame` layer when none is:
`behind` just under that layer, `front` just over it. A `layer:<name>`
rule's go around that layer the same way, inside its depth.

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

As with your own rules, each is drawn in its slot as far as its tier allows
([the effects reference](../../effects/README.md#tiers)). `glass/effects.lua`
is the shipped one: the `glass` effect behind the whole pane, and the `shade`
in front of it while the window does not have the keyboard, fading with the
focus (a rule's `fade`).

## What is here

| folder | |
|---|---|
| `glass/` | the default: the window and its bar one rounded piece of dark liquid glass, the bar along the bottom, a thin shade round the edge of a window without the keyboard ([below](#glass)) |
| `top/` | a titlebar above the window, the default until `glass` |
| `left/` | a vertical titlebar down the left side |
| `bottom/` | a titlebar underneath |
| `border/` | no bar at all, just a frame around the window |
| `reactive/` | a border that lights up where the cursor is, with a bar |
| `proximity/` | a border that answers the pointer entering and leaving |
| `reveal/` | a bar that slides out of the window's edge on approach |
| `pulse/` | a bar with an animation running in it |

`none` is not a folder: `pane = "none"` draws no frame and builds no scene per
window.

Each of those nine is one `frame` layer, which is what every decoration was
before styles had layers, and the keyboard pill's layer, described below;
`glass`'s shade is not a layer but an effect its `effects.lua` puts in front
of the pane. The rest are here to show what layers add:

| folder | |
|---|---|
| `rounded/` | `client.radius`: the compositor cuts all four of the client's corners with a fragment program, and the bar -- at `behind`, reaching `clientRadius` past its band -- shows through the two it cut inside the window |
| `flush/` | the same seam the other way up: `radiusTopLeft` and `radiusTopRight` at `0`, so the client's top is square, the bar's own rounded top is the window's top, and the two meet flat. `glass/` squares the other two, for its bar along the bottom |
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

## glass

The window and its bar are one rounded piece of dark liquid glass: `effects.lua` puts the shipped [`glass`](../../effects/glass/README.md)
effect behind the whole pane, frame and client together, cut to the window's
outline with every corner at the window's radius, so there is one rim round
the window and no seam where the bar meets the client. A fullscreen window has
no frame and no glass, and keeps direct scanout.

The bar runs along the bottom with no fill, so the glass shows through it: the
buttons at its left, close at the outer edge, the title right after them, and
the application's icon at its right, `image://solium/icon/<appId>`, which
shows nothing when the window has no app id or the theme has no icon for it
(the theme's fallback icon included).

A window without the keyboard has a thin dark shade just inside its rounded
outline, on all four sides and through the corners, and a dimmer title. The
shade is the shipped [`shade`](../../effects/shade/README.md) effect, which
`effects.lua` puts in front of the whole pane, frame and client, for a window
that is not `focused`: so it darkens the window's own edge, an opaque
application's too, and takes no input. It reads only the window's shape (tier
T0), so it is one pass on the GPU, run once per size of the window and never
for what the application draws. Its rule's `fade` brings it in as the window
loses the keyboard and takes it away as the window gets it back, 240 ms on
`outCubic`: the compositor blends the edge it has at another opacity on each
frame and runs nothing for it, and once it has faded out nothing is held or
drawn for it, so the window being typed in costs nothing for it. The bar draws
one new picture per focus change, with nothing in it animated, and while the
window's size keeps changing it is drawn again at most `resizeHz` times a
second, 30, stretched with the window between.
`tests/scenarios/pane-glass-drawn.lua` reads the bar off its pixels, and
wirecheck's case 13j the shade's.

The client's top two corners are the window's, cut by the compositor; its
bottom two are square where the bar meets it, and the glass behind the bar is
rounded to the same number, which every layer is told as `clientRadius`.

Its numbers, each a line to change in a copy of the folder,
`~/.config/solium/qml/panes/glass/`:

| where | | default |
|---|---|---|
| `Pane.qml` | `client.radius`: the window's corners, in logical pixels | `16` |
| `Pane.qml` | `insets.bottom`: the bar's height | `Theme.titlebarHeight`, 32 |
| `Frame.qml` | `iconSize`: the application's icon, in logical pixels | `16` |
| `Frame.qml` | `resizeHz`: how often, at most, the bar is drawn again for a new size while the window's size keeps changing, in times a second; 0 at every size | `30` |
| `effects.lua` | the shade's `width`: how far in it reaches, in pixels, fading to nothing there | `12` |
| `effects.lua` | the shade's `strength`: how opaque it is at the window's very edge, 0 for no shade | `0.5` |
| `effects.lua` | the shade's `color` | `{ 0, 0, 0, 1 }` |
| `effects.lua` | the shade rule's `fade`: how long it takes to come and go with the focus, in milliseconds, and on which curve | `{ duration = 240, easing = "outCubic" }` |

The glass and the shade are effects, and their params are changed where a rule
names them: a rule of your own for `{ style = "glass" }` and `part = "pane"`
wins over the style's for the same slot. At `slot = "behind"`, the glass:
other params, `tint = { 1, 1, 1, 0.11 }` for light glass, or `effect = false`
for no glass. At `slot = "front"`, with `focused = false` in its `match`, the
shade: `effect = { "shade", width = 6, strength = 0.7 }`, a `fade` of its own,
or `effect = false` for no shade.

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
at least 32 wide, in `Theme.accent` with its glyph or label in
`Theme.accentInk`, with a `margin` of 14 on every side for its shadow, and it
centres the capsule in itself. `KeyboardPillLayer` hands it
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

Nothing ships as one any more: the eight in the first table besides `glass`
were single files until the pane-styles work moved them into folders. The path stays because those files
exist on people's machines, and because a style needing neither layers nor
bleed should not have to become a folder to say so.
