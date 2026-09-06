# Drawing a window as geometry, not a rectangle

## The wall we hit

A presentation transform can currently say exactly two things:

```rust
pub(crate) struct Frame {
    rect: Rectangle<f64, Logical>,
    opacity: f32,
}
```

A rectangle and an opacity. Every effect built on that is necessarily a move,
a scale, or a fade, because the vocabulary has no other words. The genie
attempt proved it: asked for a window that tapers into a dock icon, the honest
best a rectangle can do is shrink toward it, and shrinking toward something is
not what a genie looks like.

The three things wanted are each impossible in that model, and impossible in
different ways:

* **Genie / suck** — the window's *sides curve* and its width varies down its
  length. Different rows of pixels land at different widths. No single
  rectangle describes it.
* **Stage Manager** — the window is a card in perspective: parallel edges
  converge. That is a projective transform, and a projective transform is not
  affine, so no amount of scale-and-translate reaches it.
* **Fold / curl** — the surface leaves the plane entirely.

## What replaces it

A window is drawn as **geometry with a texture on it**. The rectangle becomes
one case of that rather than the only case.

```rust
struct Frame {
    /// Where the window would be with no deformation. Still the anchor for
    /// hit-testing, for the frame, and for what a layout thinks it moved.
    rect: Rectangle<f64, Logical>,
    opacity: f32,
    /// A 4x4 applied about the rect's centre. Identity means flat.
    matrix: Mat4,
    /// A per-vertex deformation over a grid. None means four corners.
    deform: Option<Deform>,
}
```

Three layers, each strictly more expensive than the last, and a window pays
only for what it uses:

| Layer | Cost | Reaches |
|---|---|---|
| rect + opacity | one quad, existing path | move, scale, fade |
| rect + matrix | one quad, projected corners | perspective, tilt, card flip, Stage Manager |
| rect + matrix + deform | an N×M grid | genie, fold, curl, ripple |

The identity case must stay on the existing path. Most windows most of the
time are flat and opaque, and a compositor that renders every window through a
mesh to support an effect nobody is currently running has made every frame
worse to make one frame possible.

## Why hit-testing does not move

`rect` remains the truth for input. A window drawn in perspective is still
clicked where the layout put it — the alternative is inverting a projective
transform per pointer event and then explaining to a script why the window it
placed is not where clicks land. Modes that need clicks to follow the drawn
shape already invert their own transform through `to_window_space`; that stays
their business.

## What this needs from the renderer

Smithay offers all three pieces, at different depths:

* `Offscreen` — render a window's surface tree into a texture, so a deformed
  window is deformed *as a whole* rather than per subsurface. A window with a
  popup or a subsurface would otherwise tear apart along its own seams.
* `with_context` — raw GLES, for drawing arbitrary geometry. Nothing above it
  can express a projected quad.
* `compile_custom_texture_shader` — fragment effects within Smithay's element
  system, for anything that is per-pixel rather than per-vertex.

## The cost, stated plainly

`render.rs` carries a note that it is written against Smithay's `Renderer`
traits and nothing lower, because reaching into GLES is what would quietly
close the door on a Vulkan backend. This crosses that line.

It is worth crossing, and the reason is that the alternative is worse: without
it the compositor can only ever offer effects that are rectangles, which rules
out the entire class of thing this project exists to do. But the line should be
crossed in *one place*. Everything GLES lives in `warp.rs`; the rest of the
renderer keeps talking to traits. A Vulkan backend then has one file to
reimplement rather than a habit to unpick.

## Order of work

1. The vocabulary: `matrix` and `deform` on `Frame`, identity fast path kept.
2. `warp.rs`: a textured quad with projected corners, in GLES.
3. Perspective from a script — tilt and depth, which is Stage Manager.
4. Subdivision: the grid, and genie as a vertex function over it.
5. Fragment shaders by name, for the per-pixel effects.
