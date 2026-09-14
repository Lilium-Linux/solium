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

Everything the layers draw with -- colours, fonts, spacing -- comes from
`Solium.Theme`. Drop your own `Solium/Theme.qml` into `~/.config/solium/qml/`
and every pane and every shell surface follows it, without touching anything
that ships.

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

    Layer { depth: "behind"; bleed: 24;            Glow { anchors.fill: parent } }
    Layer { depth: "frame";  source: "Frame.qml" }
    Layer { depth: "above";  bleed: { "top": 48 }; source: "Spikes.qml" }
}
```

| on `PaneStyle` | |
|---|---|
| `insets.top`, `.right`, `.bottom`, `.left` | what the style reserves from the client, **once, for the whole style** |
| `requires` | what the style needs from the machine. `["gpu"]` is the only term today, and a style naming one this build has never heard of is refused rather than drawn wrong |
| `client.radius` | rounds the client's own surface, in logical pixels. A non-zero one is an offscreen pass per window per frame. `0` is no effect at all, and so is leaving the key out — which is what twelve of the fourteen bundles that ship do. `example/` writes `0`, to show the key exists and costs nothing; `rounded/` is the only one that asks for the pass |
| `client.shadow` | reserved for the shadow cast by the client's silhouette; declared, and read by nobody yet |
| the `Layer` children | the layers, in declaration order |

Insets are on the style and never on a layer. The client is placed once and
every layer sees the same client rect, so three layers each declaring insets
would be three answers to one question.

A folder without a `Pane.qml` is not a style. A bare name skips it and goes on
to the next place it would have looked, so an empty `panes/top/` of your own
does not quietly replace the shipped `top` with a window that has no frame.

`panes/example/` is the format written out in full -- all three depths, both
spellings of `bleed`, inline content and delegated content -- and

    solium --check-qml crates/solium/qml/panes/example/Pane.qml

says whether it still parses. Run it on **each file** of a bundle you write:
that is what catches a layer whose content will not build, which is otherwise a
blank rectangle and a line in the log.

## Layers

| on `Layer` | |
|---|---|
| `depth` | `"behind"` the client, in the `"frame"`, or `"above"` the client. A string, so a fourth value added later does not break every style already written. An unknown word draws at `frame` and says so in the log |
| `bleed` | how far past the pane this layer may paint. `24` for every side, or `{ "top": 48 }` for one. Zero by default |
| `source` | a QML file in this folder, when the content is not inline |
| `name` | for diagnostics: which layer a warning is about |

Content is written inline or delegated with `source:`, and the syntax does not
change between the two. Each layer is rasterised into a scene of its own and
becomes its own element in the frame, which is what lets the client's surface
sit *between* two layers of one style -- the thing a single QML file can never
do.

Layers at one depth are drawn in declaration order, the later one on top. A
press goes to the topmost layer that wants it.

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

Once, at build, because neither ever changes for a style:

| property | |
|---|---|
| `insetTop`, `insetRight`, `insetBottom`, `insetLeft` | what `Pane.qml` reserved, so a bar can size itself to its own band without a second copy of the number |
| `bleedLeft`, `bleedTop` | where the window's own corner is inside this layer's canvas |
| `clientRadiusTopLeft`, `clientRadiusTopRight`, `clientRadiusBottomLeft`, `clientRadiusBottomRight` | what the compositor is cutting each corner of the client to, in logical pixels, so a bar or a border can hug that curve. Written on every layer of every style, zeroes included — a corner left unwritten reads back 0, which is indistinguishable from one the style really squared |
| `clientRadius` | the **largest** of the four above, for a layer that wants one number. Its use is an outward hug (`radius: clientRadius + 2`), and a hug has to clear the biggest cut; a layer that needs one particular corner reads it by name. Written on every layer of every style, zero included. `rounded/Frame.qml` is the worked example |

Read back by the compositor:

| property | |
|---|---|
| `action` | set to `"close"` or `"maximize"` to ask for it; cleared once taken |
| `hovered` | the name of the button under the pointer, or `""` |

A layer declares only the ones it uses; one that positions nothing against the
window declares no `bleedTop` and is handed a property it ignores.

`hovered` decides whether a press starts a window drag, so a layer whose buttons
do not set it will have its buttons dragging the window instead.

The pointer arrives as ordinary mouse events, in the layer's own coordinates,
while it is anywhere over the window -- including over the client, which is how
a border can follow the cursor. When it leaves, the layer is told with a
position outside itself, so `MouseArea.containsMouse` goes false on its own. A
layer with bleed is given the pointer in **its own canvas**, so a click lands on
whatever that layer drew there.

## What it costs

`anchors.fill: parent` fills the **canvas**, which is the pane grown by the
bleed this layer declared -- not the window. Bleed is a cost and not a
permission: the canvas is that much larger, and every pixel of it is
rasterised, uploaded and repainted when the layer changes. A bar throwing
spikes upward should ask for `{ "top": 48 }` rather than `48`, and not pay for
three sides it never touches. It is also a promise rather than a request: the
layer is clipped to the canvas it asked for, so no one style can force a
full-screen repaint every frame.

A layer that stays inside the style's own insets has only those bands copied
and uploaded when it changes -- a titlebar is about 4% of a window, and copying
the other 96% every frame is most of what an animating decoration costs. A
layer that reserves space **and** paints outside it must say so:

    property bool overlay: true

Declare it only if you need it; the alternative is usually to reserve the
couple of pixels you were painting over. A layer at `behind` or `above`, a
layer with any bleed, and every layer of a style that reserves nothing are
overlays already and need not say it.

Animations need nothing declared. Qt is asked each frame whether the scene has
anything new to draw, and the compositor draws only then -- so an idle layer
costs a flag read, a transition runs at the screen's refresh rate, and a loop
with a pause in it survives the pause. Bear in mind only that a layer which
never stops animating never stops costing anything: on the software path it is
rasterised on the CPU, so a full-width gradient moving at 260Hz is about a
tenth of a core. Bind an endless animation to `focused`, as `wave/` does, and
an unfocused window costs nothing.

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

Each of those eight is one `frame` layer, which is what every decoration was
before styles had layers. The rest are here to show what layers add:

| folder | |
|---|---|
| `example/` | the format written out in full, and the fixture two tests build |
| `rounded/` | `client.radius`: the compositor cuts the client's corners with a fragment program, and the bar hugs the same curve with `Rectangle.radius` |
| `sandwich/` | one layer behind the client and one above it, in colours that cannot be confused |
| `wave/` | a border that physically waves, upward past the pane, using `bleed` |
| `shadow/` | `behind` plus `bleed`: stacked rectangles standing in for a blur |
| `bleedy/` | what bleed does to hit-testing, and to the window next door |

## One QML file is still a decoration

A style can also be a single `.qml` file with an `Item` at its root, under
`~/.config/solium/qml/decorations/`, named the same way. It is one layer at
`frame`, and it declares its own `insetTop`/`insetRight`/`insetBottom`/
`insetLeft` rather than being told them -- a single file has no manifest, so it
is the only place those numbers can live.

Nothing ships as one any more: the eight above were single files until Task 7
of the pane-styles work and are folders now. The path stays because those files
exist on people's machines, and because a style needing neither layers nor
bleed should not have to become a folder to say so.
