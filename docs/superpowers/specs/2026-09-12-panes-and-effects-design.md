# Panes and effects: one substrate

**Status:** design. Supersedes the scope of
`2026-09-08-pane-styles-design.md`, which it absorbs rather than replaces —
every decision in that document still holds, and the parts of it quoted here
are quoted because they were right.

Three things are being built together because they are one thing:
[#87](https://github.com/Lilium-Linux/solium/issues/87) (deforms and shaders),
[#89](https://github.com/Lilium-Linux/solium/issues/89) (z-order, pivot,
output-level transforms), and pane styles (layers with depth and bleed).

The test of this design is not whether it draws a blur. It is whether the
*second* effect costs what the first one did.

## Why now

`architecture.md` sets the standard: if a new mode needs new Rust, the
transform layer is missing something. A cover-flow switcher was then written
from outside the project, in Lua only, and it worked — which is the headline
and should be said first. Searching `crates/` for the name of any mode finds
nothing. Wallpaper, overview, workspaces, tiling, scrolling and the loading
window are all script.

But the mode **came out different from its design**, and the reasons are
structural rather than missing features:

* a deck is cards that overlap; there is no `z`, so they had to be spaced
  apart until they don't
* cover flow rotates about a card's near edge; `warp.rs:142` always pivots on
  the centre
* nothing can move the desktop, so a workspace slide moves windows one at a
  time and leaves the wallpaper behind

And the genie exists as a hardcoded enum variant with one arm. It works. It is
also the proof that the current shape does not scale: the second vertex effect
would be a second arm, the fifth would be a `match` nobody wants to read, and
none of them can be tested without a running compositor.

## What is actually missing

Three independent gaps. Naming them separately matters, because each has a
different fix and conflating them is how this becomes a rewrite.

| | absent | what it costs |
|---|---|---|
| **Completeness** | `z`, `pivot` on `Frame` | overlap, hinges, page turns, cover flow |
| **Scope** | every transform is per-pane | desktop cube, workspace slide, accessibility zoom |
| **Composition** | one flat element list, one submit | blur, shadows, rounded corners |

The genie needs none of the three. That is precisely why it was buildable as a
special case, and why the next five effects are not.

## The shape

A **node tree per output**, resolved into **passes**.

```
output                      transform applies to the whole desktop
  └── pane (z-sorted)       rect, matrix, pivot, deform, opacity
        ├── layer  behind   may bleed past the pane's rect
        ├── client          the application's own surface
        ├── layer  frame
        └── layer  above
```

Three rules hold it together.

### 1. Geometry and pixels are different kinds, on purpose

**Vertex deformation is CPU-side and parametric** — a named function with
parameters, never user code. Two reasons, both hard:

* the damage tracker needs the deformed bounding box before anything is drawn,
  and a shader cannot tell it one
* an effect must be testable without a session. `crates/animation` already
  proves the pattern — an empty `[dependencies]`, unit tests, and a wasm
  preview page with live sliders. `crates/effects` is the same crate again.

**Fragment effects are arbitrary GLSL**, because they do not move the bounding
box. A bad one is a wrong picture, which is recoverable; a bad vertex function
would be a wrong damage rect, which is a corrupt screen.

This split is the safety property of the whole design. It is also why a hung
fragment shader is acceptable risk and a hung vertex function would not be: a
GPU reset is a driver problem no Rust-side rule can catch, and keeping the
geometry half in a language where a mistake is a wrong number rather than a
dead session is what makes the other half affordable.

### 2. An effect declares its inputs, and that is what creates a pass

An effect is `{ name, parameters, inputs }`. Most declare nothing and draw
inline, exactly as today.

An effect that declares `backdrop` needs what is already composited underneath
it. The renderer closes the current pass, binds its output as a texture, and
opens a new one.

That single mechanism is what turns three special cases into three
declarations:

| effect | inputs | why |
|---|---|---|
| wavy border, glow, spikes | — | draws over what is there |
| rounded corners | `self` | masks the client's own texture |
| shadow | `self` | derived from the client's silhouette |
| blur | `backdrop` | samples what is behind |

Without this, blur is not merely unimplemented — it is unrepresentable, because
a flat element list has nowhere to say "after the things below me, before the
things above me".

### 3. Identity collapses to today

A pane with no transform, no effect and no bleed emits exactly the element it
emits now, through exactly the path it takes now. The spike's rule stands and
is load-bearing:

> The identity case must stay on the existing path. Most windows most of the
> time are flat and opaque, and a compositor that renders every window through
> a mesh to support an effect nobody is currently running has made every frame
> worse to make one frame possible.

Every addition below is opt-in at the node level, and a desktop that opts into
nothing pays nothing.

## Completing `Frame`

```rust
pub(crate) struct Frame {
    rect: Rectangle<f64, Logical>,
    opacity: f32,
    matrix: Mat4,
    deform: Option<Deform>,
    /// Depth. Sorted into the element list; equal values keep list order.
    z: f32,
    /// What `matrix` turns about, normalised 0..1 of the rect. `(0.5, 0.5)`
    /// is the centre, which is what `warp.rs` hardcodes today.
    pivot: (f32, f32),
}
```

`z` is depth and nothing else. It is **not** [#84](https://github.com/Lilium-Linux/solium/issues/84),
which is about which of three places owns a window's *position*. The available
workaround today is worse than the gap: `sol.focus` restacks, so a script can
only express depth by moving the keyboard — and then every switcher sends focus
events to clients as the user scrubs through it.

`pivot` cannot be done in `script.rs` alone. Composing `translate(-p) · R ·
translate(p)` there needs the window's size, and `transform_from` only sees the
options table — so it would work when a script passed an explicit rect and
silently do the wrong thing otherwise. It belongs in `warp.rs`, at the two
lines that currently compute the centre.

## Scope: transforming more than a pane

A transform today names a pane. It should be able to name a **group**: an
output, a workspace, an arbitrary set.

```lua
sol.present_group({ output = "DP-1" }, { rotate_y = 20, perspective = 1200 })
```

The group's transform composes with each member's own, so a window that is
also individually tilted stays tilted *within* a rotating desktop. That
composition is the whole point — it is what makes a cube of workspaces
different from rotating every window by hand and hoping.

**Ruling — the pointer follows a group transform only when the transform says
so.** Not global. A desktop cube should carry the cursor with it; whole-desktop
zoom, which is an accessibility feature rather than an effect, should leave it
where the user's hand thinks it is. One flag, `carries_pointer`, defaulting to
true for rotations and false for scale-only.

This also finally makes the cursor an ordinary participant. Today it is its own
render path — `cursor.rs`, with `SIZE` and `HOTSPOT` as constants, placed ahead
of everything in `render.rs`. Every other thing the compositor draws for its own
reasons went through `sol.surface`; this one did not, so a script cannot replace
the pointer, animate it, or give a mode its own.

## Layers: pane styles, absorbed

A pane's appearance is a **style bundle** — a folder under `panes/`, looked up
user-directory-first by name. `Pane.qml` is the manifest, and it is QML rather
than Lua deliberately: **style lives in QML, Lua configures the compositor.**

```qml
PaneStyle {
    insets.top: 32

    Layer { depth: "behind"; bleed: 200; Glow { anchors.fill: parent } }
    Layer { depth: "frame";  source: "Frame.qml" }
    Layer { depth: "above";  bleed: 160; source: "Spikes.qml" }

    client.radius: 12          // an effect with inputs: self
    client.shadow.blur: 40     // an effect with inputs: self
}
```

**Ruling — three depths, not arbitrary nesting.** `behind`, `frame`, `above`.
Everything named in three months of design fits; nesting is unbounded cost for
a case nobody has asked for, and it can be added later without breaking the
format because a layer's depth is a string.

`bleed` is how far past the pane's own rect a layer may paint. Three rules,
carried unchanged from the pane-styles design because they were right:

* **Bleed follows the pane's stacking.** A background window's effects do not
  paint over the window being typed in; a focused window's reach over
  everything behind it without asking.
* **Bleed is a promise, not a request.** A layer is clipped to the canvas it
  declared. Without that, one style can force a full-screen repaint every frame
  and the cost appears as the whole desktop stuttering with nothing pointing at
  the cause.
* **Input stays clipped to the pane's outer rect.** A spike reaching over the
  next window must not eat that window's clicks. The failure mode of getting
  this wrong is a neighbour that has silently stopped responding.

`client.radius` and `client.shadow` stop being reserved keys and become
effects with `inputs: self`. The reason they were deferred is unchanged and is
now handled rather than avoided: a rounded window is no longer fully opaque, so
opaque-region culling can no longer skip what is behind it. Under this design
that is a property the node declares, and the renderer's culling reads it —
rather than a global assumption that quietly stops being true.

## The QML contract

The software scene graph does not implement `ShaderEffect`, and `Canvas`
appears not to paint on it. The GPU path does both. That means **which QML is
legal currently depends on which machine you are on**, and a style written on
one hands a white rectangle to another, silently. That is the exact failure the
rice report is about, one level up.

**Ruling — a style declares what it requires, and is refused at load when the
path cannot provide it.**

```qml
PaneStyle {
    requires: ["gpu"]     // absent means portable
}
```

The three alternatives were considered and rejected:

* **GPU-only styles.** Clean, but software is still the default path, so most
  people get nothing.
* **A lowest-common-denominator subset.** Caps every author at what the weakest
  path can do, to protect a case the author may not care about.
* **Silence.** What happens today.

Declaring and failing loudly is the only option that is both portable and
honest. It also composes with versioning below: `requires` is a list, so
`["gpu", "effects/2"]` is the same mechanism.

## Genie, and the next one

Genie becomes an entry in `crates/effects`: a named vertex function over a
grid, with parameters, unit tests, and a slider in the preview page.
`present.rs` stops knowing what a genie is.

Two things it gains in the move, both from #87:

* **A morph between two anchors**, not "minimise into a slot". The anchor is an
  identity resolved per frame by the compositor — a pane id, a chrome element —
  not a rect passed from Lua. A rect snapshotted at dispatch aims at where the
  dock icon was 500 ms ago.
* **The phase axis becomes a parameter.** It is hardcoded to `(1.0 - v)` today,
  which is why a side dock is inexpressible. Note the grid follows the axis:
  genie subdivides 48 along `v` and 8 along `u` because `v` is the phase axis,
  so a parametric axis that left the grid fixed would give a side dock eight
  steps of phase and a flipbook.

Adding *fold*, *curl*, *ripple* or *page-turn* is then a file in that crate.
That is the whole test of this document.

## Versioning, from day one

Once people write styles and effects it is a compatibility surface, and GLES2
shaders will not survive a Vulkan backend. `requires` carries the version;
the format's own version is a field in the manifest. This costs one line now
and is unaddable later.

## What this does not do

No IPC and no PipeWire, so **no effect genuinely follows what an application is
playing**. A rule-driven wave border animates because the rule matched, not
because music is playing and not on the beat. Real reactivity needs a channel
into a running compositor, which is its own subsystem and is deliberately not
started here.

Arbitrary nesting, per-layer insets, and effects on layer-shell surfaces are
all deferred, and none of them is blocked by anything above.

## Order of work

1. **`z` and `pivot`.** Smallest, unblocks the most, and proves the node model
   against a real mode — the deck already exists to test it.
2. **`crates/effects`,** with genie moved into it. No new capability; the
   capability is that the *next* one is cheap.
3. **Layers with depth and bleed.** The pane-styles plan's Tasks 1–7.
4. **Passes.** Then `client.radius`, `client.shadow` and blur, which are three
   declarations against one mechanism.
5. **Group transforms.** The desktop cube, the workspace slide, the zoom.
6. **Rules and pane ownership.** The pane-styles plan's Tasks 8–9, last on
   purpose: they touch the most call sites and gain most from knowing the final
   shape.

Each stage leaves the compositor running and testable. Stage 3 without 4 gives
layered decorations with no client effects; stage 4 without 5 gives blur on a
desktop that cannot rotate.
