# Pane styles: layered decorations in QML

**Status:** design, approved to plan. Not implemented.

A pane's appearance becomes a *style bundle* — a folder of QML declaring one or
more layers, each of which may sit behind the client, around it, or over it,
and each of which may paint past the pane's own edge. Styles are selected per
pane by rule, so one application can look nothing like another.

The target is a capability no compositor currently has: a window whose border
reaches beyond its own rectangle and reacts, while the application inside it
neither moves nor knows.

## Why

Today a decoration is one QML file rendered into one buffer the size of the
window's outer rect, drawn at one depth. That fixes three things a style author
cannot get around:

* **It cannot draw behind the client.** One element, one depth, and the client
  is composited over it.
* **It cannot draw outside the window.** The buffer is the outer rect, so paint
  beyond it has nowhere to go.
* **It is one file for the whole look.** A style with a glow, a titlebar and an
  overlay is one file doing three jobs, or it is impossible.

The last of these also blocks the thing that makes ricing worth doing: a look
should be a folder you can zip and hand to somebody.

## What a style is

A folder under `panes/`, looked up the way decorations already are — the user's
directory first, then the one that ships, by name:

```
~/.config/solium/panes/neon/
    Pane.qml          the manifest, and for a simple style the whole thing
    Frame.qml         a layer, when one grows big enough to deserve a file
    Spikes.qml
    Theme.qml         this style's own colours, shadowing Solium.Theme
    assets/
```

`Pane.qml` is the entry point, and it is QML rather than Lua deliberately:
**style lives in QML, and Lua configures the compositor.** The manifest is a
description of how a thing looks, so it belongs on the QML side of that line.

```qml
import QtQuick
import Solium

PaneStyle {
    // Reserved from the client, once, for the whole style. Not per layer:
    // the client is placed once and every layer sees the same client rect,
    // so three layers each declaring insets would be three answers to one
    // question.
    insets.top: 32

    Layer {
        depth: "behind"
        bleed: 200
        Glow { anchors.fill: parent }
    }

    Layer {
        depth: "frame"
        source: "Frame.qml"
    }

    Layer {
        depth: "above"
        bleed: 160
        source: "Spikes.qml"
    }
}
```

A layer's content is written inline or delegated with `source:`, and the syntax
does not change between the two. A plain titlebar is one file with one inline
layer; a style with three animated layers is a folder. There is no threshold to
cross and no format to migrate between.

### `Layer`

| property | | |
|---|---|---|
| `depth` | `"behind"` \| `"frame"` \| `"above"` | where the client's surface sits relative to this layer. Default `"frame"` |
| `bleed` | int, or `{top,right,bottom,left}` | logical pixels this layer may paint past the pane's outer rect. Default `0` |
| `source` | string | a QML file in the bundle, when the content is not inline |
| `name` | string | for diagnostics only — which layer a warning is about |

Per-side `bleed` exists because a bar that throws spikes upward should not pay
for three sides it never touches: the canvas is the cost, and it is the product
of the sides asked for.

### `PaneStyle`

Holds the layers, the insets, and later the client treatment (see *Reserved*).

It is also where a rule's `options` land: they are set as properties on the
`PaneStyle` root, and layers read them from there —

```qml
PaneStyle {
    property real hue: 200            // default, overridden by a rule
    Layer { depth: "above"; Spikes { color: Qt.hsla(parent.hue / 360, 1, 0.6, 1) } }
}
```

— rather than being set on every layer. One object owns a property, layers
reference it, and a style with three layers does not have three copies of `hue`
that can disagree.

### What the compositor sets on every layer

Unchanged from the current contract, and now per layer rather than per
decoration: `title`, `focused`, `pointerInside`, `contentWidth`,
`contentHeight`. Read back: `action`, `hovered`.

A layer additionally gets `paneWidth`, `paneHeight` — the outer rect — and
`bleedLeft`, `bleedTop`, so QML can position against the window's own corner
rather than the canvas corner, which is offset by the bleed.

## Rendering

Each layer is rasterised into its own texture and becomes its own element in
the frame's element list. The client's surface is placed between the `behind`
elements and the `frame`/`above` ones, which is the whole reason layers are
separate scenes rather than one image: nothing else lets a client be
sandwiched inside something a single QML file produced.

Element order for one pane, topmost first, is `above`, `frame`, client,
`behind`. The pane's own position in the list is unchanged, so:

**Bleed follows the pane's stacking.** A layer that paints past its pane
overlaps whatever is *below* that pane and is covered by whatever is above it.
A background window's effects do not paint over the window being typed in, and
a focused window's effects reach over everything behind it without asking.

**Bleed is a promise, not a request.** A layer is clipped to the canvas it
declared. Without that, damage is unbounded: one style could force a
full-screen repaint every frame and the cost would appear as the whole desktop
stuttering, with nothing pointing at the style that caused it.

**Damage includes the bleed.** A layer's damage rect is its canvas, not the
pane, or an animating bleed leaves trails over its neighbours.

**Input stays clipped to the pane's outer rect**, whatever the bleed. A spike
reaching over the next window must not eat that window's clicks. The failure
mode of getting this wrong is a neighbour that has silently stopped responding,
with nothing on screen to explain it — which is much worse than an effect that
is merely decorative.

### Cost

Three layers with bleed is several times the rasterisation of today's single
banded frame, per window, per frame. The current band optimisation — copying
only the inset strips when a frame stays inside them — still applies to a
`frame` layer with no bleed, and cannot apply to a bleeding one.

This is why the GPU render target is a prerequisite rather than an improvement.

## Selection

Lua selects; QML describes. The global default and a rule per application:

```lua
pane = "neon",

rules = {
    { app_id = "spotify", pane = "wave", options = { hue = 280, speed = 1.4 } },
    { app_id = "mpv",     pane = "none" },
},
```

`options` are set as properties on the `PaneStyle` root — see above — so one
`wave` bundle serves many looks without a folder each.

This is a slice of [#56](https://github.com/Lilium-Linux/solium/issues/56)
(window rules). Only the style-selecting predicate is in scope here; placement,
floating and workspace assignment stay with that issue.

### Migration from `decoration`

`decoration` is replaced by `pane`. The eight shipped decorations each become a
one-layer bundle.

A rice should be one folder and one word. Keeping both concepts — "a decoration
is a file, a pane style is a folder" — means every doc page and every dotfiles
repo has to explain which is in use and why, permanently, which is how
configuration formats become the thing people complain about.

`decoration = "top"` is accepted as a silent alias for `pane = "top"`, and
`SOLIUM_DECORATION` for `SOLIUM_PANE`. That costs one line and breaks nobody.
Converting the shipped eight is a second gain: they stop being special cases
and become eight readable examples of the format an author is about to write.

## Pane ownership

`Decorations` holds two parallel tables keyed by `PaneId` — `frames` and
`bare` — reconciled by hand in `sync_panes` against a set of live panes. Two
tables answering one question ("does this pane have a frame?") can disagree:
today `insets_of` checks `frames` first, so a pane in both has `bare` silently
ignored. Nothing tests that.

With three layers per pane it becomes three entries to keep in step instead of
one. The pane should own its frame state as a single value:

```rust
enum Frame {
    Pending,          // a client is coming; keep reserving its insets
    None,             // client-side decorations, or override-redirect. Never a frame
    Styled(PaneStyle),
}
```

`insets_of` becomes a pure function of the pane, the decoration `retain`
disappears, and the illegal state stops being representable. `closing` and
`asked` are per-pane timers `retain`ed for the same reason and move in with it.

`hovered_frame` stays where it is: it is a property of the pointer — which pane
is hovered — not of a pane, and moving it in would mean a bool on every pane
and a scan to find the one that is true.

**This does not fix [#33](https://github.com/Lilium-Linux/solium/issues/33).**
That leak was isolated to the GL/EGL buffer import path. Unifying these
lifetimes removes a *class* of leak that has bitten here before — a client that
crashed used to leave its frame behind forever — but crediting it with #33
would be wrong.

## Prerequisite: the GPU render target

QML currently rasterises on the CPU. Qt's software scene graph was chosen
because context adoption is impossible on this platform plugin —
`QNativeInterface::QEGLContext::fromNative` is unimplemented — and the
consequence is recorded in `docs/spikes/2026-09-04-qml-in-compositor.md`: a
full-width gradient animating at 260 Hz costs about a tenth of a core, inside
one window.

Three layers, each on a canvas larger than the window, animating, is several
times that per window. It does not fit on the CPU.

The route is the spike's option 2, the dmabuf-backed render target: allocate
through GBM, give Qt `QQuickRenderTarget::fromOpenGLTexture` over an `EGLImage`
of that buffer, import the same dmabuf on our side, fence with
`EGL_ANDROID_native_fence_sync`, and restore our EGL context after every Qt
render.

**Stated risk, accepted.** The spike names this as where the work stalls: this
is an NVIDIA machine, dmabuf round-trips and cross-context fences are where
that driver is least forgiving, and a wrong fence fails intermittently rather
than loudly. The decision was to design the whole thing now and carry the risk
rather than spike it first. If the fence proves unreliable, everything below
the prerequisite still stands — it would need the ambition bounded to what the
CPU can carry, which is roughly one animated layer with modest bleed.

## Reserved, not built

`PaneStyle` declares client treatment from day one so that styles are written
against the final format, and the compositor ignores it until it is built:

```qml
PaneStyle {
    client.radius: 12
    client.shadow.blur: 40
}
```

Rounding a window's corners and drawing its shadow are not decoration layers:
they change how the *client's own* surface is composited. That drags in damage
tracking, opaque-region culling — a rounded window is no longer fully opaque,
so what is behind it can no longer be skipped — and subsurface clipping. Each
can regress performance in ways that only appear on real hardware.

It gets its own design after this one. Reserving the key now means a style
folder written today does not change shape when it arrives.

## Out of scope

No IPC or control socket. No PipeWire. No per-window audio levels, and
therefore **no effect that genuinely follows what an application is playing** —
a rule-driven wave border animates because the rule matched, not because music
is playing, and not on the beat. Real reactivity needs a channel into a running
compositor, which is its own subsystem and is deliberately not started here.

`pid` is not exposed to scripts. It would be the way to match an audio stream
to a window, and nothing in this design needs it.

## Testing

**Layers and z-order are visual and capturable.** Two windows overlapping, the
lower one carrying a `behind` layer with bleed: assert the upper window's
pixels win where they overlap, and that the bleed is visible outside the lower
window where nothing covers it. The compositor reads back its own framebuffer,
so this is a frame comparison rather than a screenshot tool.

**Existing decorations must be byte-identical.** Every shipped style converted
to a one-layer bundle, rendered, and compared frame-for-frame against the
current build. A conversion that changes a pixel is a bug in the conversion.

**Input clipping needs its own check.** A window with a large bleed over a
neighbour, a scripted drag into the bleed region, and the neighbour must
receive the press. `SOLIUM_DRAG_AT` goes through the real pointer path;
`SOLIUM_CLICK_AT` does not and would pass while proving nothing.

**Bleed clipping.** A layer that paints deliberately outside its declared
canvas: assert nothing appears beyond it.

**The GPU render target cannot be fully verified nested.** The dmabuf path
needs the DRM backend and real hardware. Nested runs on the host's GPU through
winit and will not exercise the same allocation or the same fence.

## Order of work

1. **GPU render target.** The prerequisite and the risk. Nothing below it is
   affordable without it.
2. **`PaneStyle` and `Layer` types, layered rendering.** The capability, with
   the shipped decorations still in their current form.
3. **`panes/` bundles, and converting the shipped eight.** The format, proven
   by migrating every existing style to it.
4. **Rules.** Per-pane selection.
5. **Pane ownership.** Last on purpose: it touches the most call sites and
   gains most from knowing the final shape.

Each stage is landable alone. Stage 2 without 3 gives layered decorations in
today's single-file form; stage 3 without 4 gives bundles with one global
style.

**Stage 1 wants its own implementation plan.** It shares nothing with the rest
but a dependency: it is GBM allocation, EGL image binding, cross-context
fencing and context restoration, none of which touches panes, QML contracts or
Lua. Planning it together with stages 2–5 would produce one plan where half the
steps cannot be checked by anyone reading the other half. Stages 2–5 are one
plan; stage 1 is another, and it goes first.
