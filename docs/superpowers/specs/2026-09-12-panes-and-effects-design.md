# The scene: panes, surfaces, groups and effects

**Status:** design. Absorbs `2026-09-08-pane-styles-design.md`, whose decisions
all still hold, and answers
[#84](https://github.com/Lilium-Linux/solium/issues/84),
[#85](https://github.com/Lilium-Linux/solium/issues/85),
[#87](https://github.com/Lilium-Linux/solium/issues/87) and
[#89](https://github.com/Lilium-Linux/solium/issues/89) as one thing.

`docs/modes.md` states the standard this design is measured against:

> **If a new mode needs new Rust, the transform layer is missing something**,
> and that missing thing is the bug rather than your mode.

Three modes have now been written against that standard by people who were not
the author. Each worked, and each came out different from its design. **What
they could not say is this document's requirements list.**

## The evidence

**A cover-flow deck** (`rice/prism/deck.lua`, written from outside the project
in Lua only). Worked. But there is no `z`, so cards had to be spaced until they
never overlap; and `warp.rs:142` always pivots on the centre, so they could not
rotate about a near edge. The available workaround for depth is worse than the
gap: `sol.focus` restacks, so a script can only express depth by moving the
keyboard — and then a switcher sends focus events to clients as the user scrubs.

**Atrium** (#85), not yet written, and deliberately chosen as the hardest test.
It needs a window at arbitrary scale (there), several windows moving as one
(not there), stacking from a script (not there), a swap where two windows
exchange places **along a path** rather than cross-fading (not there), and a
grab scoped to the strip while the centre stays live (not there).

**Workspaces**, which ship. `workspaces.lua` moves every window individually in
a loop, and the wallpaper, the bars and the layer surfaces do not come with
them — because `scripted::Surface` has `name`, `layer`, `pointer` and
`area_on`, and no transform at all. A surface is **drawn** but not
**addressable**.

## What is missing, named separately

Four gaps. Each has a different fix, and conflating them is how this becomes a
rewrite rather than a series of landable stages.

| | absent | costs |
|---|---|---|
| **Completeness** | `z`, `pivot`, per-node alpha | overlap, hinges, page turns, a receding strip |
| **Address** | only panes can be transformed | wallpaper left behind, no groups, no desktop |
| **Motion** | a transform is a destination | no arcs, no swaps, no paths |
| **Composition** | one flat element list, one submit | blur, shadows, rounded corners |

The genie needs none of the four. That is exactly why it was buildable as a
hardcoded enum arm with one variant — and why the next five effects are not.

## The model

**One addressable scene. A transform names a selection. Selections compose.**

That is the whole idea, and everything below is a consequence of it.

### What is addressable

```
output          every node drawn on it
pane            a window and its chrome
surface         a sol.surface instance: wallpaper, bar, dock, scrim
layer           one depth within a pane's style
group           a named set of any of the above
```

A **group** is not a new kind of thing. It is a name for a selection, and a
transform on it composes with each member's own. That composition is the point:
a window individually tilted inside a rotating desktop stays tilted *within* it,
which is what makes a cube of workspaces different from rotating every window by
hand and hoping.

```lua
sol.group("desk-2", { workspace = 2, surfaces = { "wallpaper" } })
sol.present_group("desk-2", { x = -2560 }, { duration = 300 })
```

One call, one animation, one target. The wallpaper comes because it is in the
selection, not because the compositor knows what a wallpaper is — and it does
not; `sol.surface` is one primitive doing five jobs and that stays true.

This single mechanism answers #89's output-level transform (a selection that is
an output), #85's groups (a named set), and the workspace slide (windows plus
surfaces), without three features.

### What every node carries

```rust
struct Node {
    rect: Rectangle<f64, Logical>,   // where it lives; still the truth for input
    opacity: f32,
    matrix: Mat4,
    deform: Option<Deform>,          // vertex, CPU-side, parametric
    effects: Vec<Effect>,            // fragment, GPU, declares its inputs
    z: f32,                          // depth. Equal values keep list order
    pivot: (f32, f32),               // what `matrix` turns about, 0..1 of rect
}
```

`z` is depth and nothing else. It is **not** #84, which is about which of three
places owns a window's *position*.

`pivot` cannot be done in `script.rs` alone. Composing `translate(-p) · R ·
translate(p)` there needs the window's size, and `transform_from` only sees the
options table — so it would work when a script passed an explicit rect and
silently do the wrong thing otherwise. It belongs at the two lines in `warp.rs`
that compute the centre today.

### Hit-testing does not move

`rect` remains the truth for input. A window drawn in perspective is still
clicked where the layout put it. The alternative is inverting a projective
transform per pointer event and then explaining to a script why the window it
placed is not where clicks land. Modes that need clicks to follow the drawn
shape already invert their own transform through `to_window_space`; that stays
their business.

## Motion: paths, not just destinations

`sol.present` takes a destination rectangle, so a window can only ever move in a
straight line to it. Atrium's swap — the outgoing window travelling out to the
strip while the incoming one grows from where it sat — is the difference between
*"it moved"* and *"it was put there"*.

```lua
sol.present(id, { rect = centre, via = { arc = 0.4 } }, { duration = 320 })
```

`via` is a named path function with parameters, resolved the same way an easing
is — `crates/animation` already proves the pattern, and a path is a curve
through space exactly as an easing is a curve through time. Absent means a
straight line, which is what every mode does today and what it keeps costing.

The same reasoning that keeps vertex deformation parametric applies: a path
moves the bounding box, so the damage tracker has to be able to ask where it
will be.

## Effects: geometry and pixels are different kinds

**Vertex deformation is CPU-side and parametric** — a named function with
parameters, never user code. The damage tracker needs the deformed bounding box
before anything is drawn, and a shader cannot tell it one. An effect must also
be testable without a session: `crates/effects` is `crates/animation` again —
empty `[dependencies]`, unit tests, a wasm preview with live sliders.

**Fragment effects are arbitrary GLSL**, because they do not move the bounding
box. A bad one is a wrong picture, which is recoverable. A bad vertex function
would be a wrong damage rect, which is a corrupt screen.

This is also why a hung fragment shader is acceptable risk and a hung vertex
function would not be: a GPU reset is a driver problem no Rust-side rule can
catch, and keeping the geometry half in a language where a mistake is a wrong
number is what makes the other half affordable.

### An effect declares its inputs, and that is what creates a pass

```
effect { name, parameters, inputs }
```

Most declare nothing and draw inline, exactly as today. One that declares
`backdrop` needs what is already composited beneath it: the renderer closes the
current pass, binds its output as a texture, and opens a new one.

| effect | inputs | why |
|---|---|---|
| wavy border, glow, spikes | — | draws over what is there |
| rounded corners | `self` | masks the node's own texture |
| shadow | `self` | derived from the node's silhouette |
| blur | `backdrop` | samples what is behind |

Without this, blur is not merely unimplemented — it is **unrepresentable**,
because a flat element list has nowhere to say "after the things below me,
before the things above me".

`offscreen::capture` already renders a node's surface tree into a texture and is
how `self` is obtained. It currently allocates per frame per warped pane, forty
lines below a comment explaining why that is hundreds of megabytes a second;
that is a prerequisite, not a follow-up, because an effect system multiplies it
by the number of animating nodes.

## Layers and bleed

A pane's appearance is a **style bundle** — a folder under `panes/`, looked up
user-directory-first. `Pane.qml` is the manifest, and it is QML rather than Lua
deliberately: **style lives in QML, Lua configures the compositor.**

```qml
PaneStyle {
    insets.top: 32
    requires: ["gpu"]

    Layer { depth: "behind"; bleed: 200; Glow { anchors.fill: parent } }
    Layer { depth: "frame";  source: "Frame.qml" }
    Layer { depth: "above";  bleed: 160; source: "Spikes.qml" }

    client.radius: 12          // an effect with inputs: self
    client.shadow.blur: 40     // an effect with inputs: self
}
```

**Ruling — three depths, not arbitrary nesting.** `behind`, `frame`, `above`.
Everything named in months of design fits; nesting is unbounded cost for a case
nobody has asked for, and can be added later without breaking the format because
a depth is a string.

Three rules on bleed, carried unchanged because they were right:

* **Bleed follows the pane's stacking.** A background window's effects do not
  paint over the window being typed in; a focused window's reach over everything
  behind it without asking.
* **Bleed is a promise, not a request.** A layer is clipped to the canvas it
  declared, or one style can force a full-screen repaint every frame and the
  cost appears as the whole desktop stuttering with nothing naming the cause.
* **Input stays clipped to the pane's outer rect.** A spike over the next window
  must not eat that window's clicks. The failure mode is a neighbour that has
  silently stopped responding.

`client.radius` and `client.shadow` stop being reserved keys and become effects
with `inputs: self`. The reason they were deferred is unchanged and now handled
rather than avoided: a rounded window is no longer fully opaque, so
opaque-region culling cannot skip what is behind it. Under this design that is a
property the node declares and the renderer's culling reads, rather than a
global assumption that quietly stops being true.

## Input, scoped

`script_grab` is all-or-nothing. Atrium wants the strip to take clicks while the
centre window stays live, and an interactive `sol.surface` currently swallows
**every** press inside its rectangle whether or not the QML under the pointer
wanted it — which is why Prism's dock has to be sized to its contents and
re-placed on every window open and close.

Both are the same missing thing: a grab, and a surface, should be able to say
*which* input it wants. The scene already knows whether a `MouseArea` accepted;
the answer is simply not propagated.

## The QML contract

The software scene graph does not implement `ShaderEffect`, and `Canvas` appears
not to paint on it; the GPU path does both. So **which QML is legal depends on
which machine you are on**, and a style written on one hands a white rectangle
to another, silently.

**Ruling — a style declares what it requires and is refused at load when the
path cannot provide it.** `requires: ["gpu"]`, absent meaning portable. The
alternatives are GPU-only styles (software is still the default, so most people
get nothing), a lowest-common-denominator subset (caps every author to protect a
case they may not care about), or silence (what happens today). `requires` is a
list, so `["gpu", "effects/2"]` is the same mechanism as versioning.

## What stays cheap

A node with no transform, no effect and no bleed emits exactly the element it
emits now, through exactly the path it takes now. The spike's rule is
load-bearing:

> a compositor that renders every window through a mesh to support an effect
> nobody is currently running has made every frame worse to make one frame
> possible.

Every addition above is opt-in per node, and a desktop that opts into nothing
pays nothing.

## Genie, and the one after it

Genie becomes an entry in `crates/effects`: a named vertex function over a grid,
with parameters, unit tests and a preview slider. `present.rs` stops knowing what
a genie is. It gains two things in the move:

* **A morph between two anchors**, not "minimise into a slot". The anchor is an
  identity resolved per frame by the compositor — a pane id, a chrome element —
  not a rect passed from Lua. A rect snapshotted at dispatch aims at where the
  dock icon was 500 ms ago.
* **The phase axis becomes a parameter.** It is hardcoded to `(1.0 - v)` today,
  which is why a side dock is inexpressible. The grid must follow the axis:
  genie subdivides 48 along `v` and 8 along `u` *because* `v` is the phase axis,
  so a parametric axis with a fixed grid gives a side dock eight steps and a
  flipbook.

Adding *fold*, *curl*, *ripple* or *page-turn* is then a file in that crate.
**That is the whole test of this document.**

## Out of scope

No IPC and no PipeWire, so **no effect genuinely follows what an application is
playing**. A rule-driven wave border animates because the rule matched, not
because music is playing and not on the beat. Real reactivity needs a channel
into a running compositor; it is its own subsystem and is deliberately not
started here.

Arbitrary layer nesting, per-layer insets, and effects on layer-shell surfaces
are deferred, and none is blocked by anything above.

## Order of work

Each stage leaves the compositor running and testable, and each is landable
alone.

1. **`z`, `pivot`, node alpha.** Smallest, unblocks the most, and the deck
   already exists to test it against.
2. **Address: surfaces and groups become transformable.** The workspace slide
   stops leaving the wallpaper behind, which is a visible win on a shipped mode.
3. **`crates/effects`,** genie moved into it. No new capability; the capability
   is that the next one is cheap.
4. **Paths.** `via`, resolved like an easing.
5. **`offscreen::capture` caching, then passes.** Then `client.radius`,
   `client.shadow` and blur, which are three declarations against one mechanism.
6. **Layers with depth and bleed.** The pane-styles plan's Tasks 1–7.
7. **Input scoping.** The surface press question and the scoped grab.
8. **Rules and pane ownership.** The pane-styles plan's Tasks 8–9, last on
   purpose: they touch the most call sites and gain most from knowing the final
   shape.

Atrium is the acceptance test. It is not on this list because it should need
nothing that is not — and if it does, that thing is the next entry.
