# Spike: Vulkan renderer on Smithay — which path?

**Issue:** #9
**Date:** 2026-08-27
**Outcome:** start on GLES2. Vulkan is a later, optional port.

**Since then (2026-09-30).** The outcome held: Solium draws with GLES2 through
`renderer_gl`; Smithay 0.7, which it builds against, still has no Vulkan
renderer; and niri still uses GLES. The constraint this spike set did not
hold. The [3D-presentation spike](2026-09-06-3d-presentation.md) crossed the
trait line on purpose on 2026-09-06, in one file, and GLES has since spread
past that file. So a Vulkan backend is no longer "a contained change";
[What a Vulkan port costs now](#what-a-vulkan-port-costs-now) says what it
would take. No issue schedules one. Two of the three reasons given below for
wanting Vulkan do not need it: explicit sync
([#59](https://github.com/Lilium-Linux/solium/issues/59)) is a protocol Smithay
offers on GLES, and Smithay's multi-GPU renderer
([#63](https://github.com/Lilium-Linux/solium/issues/63)) is built on GLES.
Compute shaders remain a reason. "Also found", at the end, is resolved.

## Question

Does Smithay support Vulkan rendering today, or does choosing Vulkan mean
implementing the renderer ourselves?

## Findings

### Smithay has no Vulkan renderer

`smithay::backend::renderer` ships: **GLES2** (`renderer_gl`), **Pixman**
(software, `renderer_pixman`), **Glow** (`renderer_glow`), **Multi-GPU**
(`renderer_multi`), and a test renderer. There is no Vulkan implementation.

### `smithay::backend::vulkan` exists but does not render

It is easy to mistake this module for Vulkan support. It is device enumeration
and instance initialisation only — `Instance`, `PhysicalDevice`, `AppInfo`, thin
wrappers over `ash`. Its own documentation is explicit:

> This module does not provide abstractions for logical devices, rendering or
> memory allocation. These should instead be provided in higher level
> abstractions.

So it helps you *find* a GPU. It does not help you draw with it.

### niri uses GLES, not Vulkan

The reference implementation — Rust on Smithay, with working touch and gesture
support, which is the main reason we chose this stack — enables `renderer_gl`,
`renderer_pixman` and `renderer_multi`. No `ash`, no Vulkan anywhere in its
dependencies.

### What Vulkan would actually cost

A from-scratch renderer backend implementing, at minimum:

| Trait | For |
|---|---|
| `Renderer`, `Frame` | the drawing API itself |
| `Bind`, `Offscreen` | render targets, offscreen buffers |
| `ImportDma`, `ImportDmaWl` | client buffers via dmabuf |
| `ImportMem`, `ImportMemWl` | shm client buffers |
| `ExportMem` | screenshots, screencopy |
| `Blit`, `BlitFrame` | framebuffer blitting |

`ImportDma` on NVIDIA is the risky one. Every GPU-rendering client hands us a dmabuf,
and importing those into Vulkan images with correct format and modifier
negotiation on the proprietary driver is exactly where this kind of work stalls
— and it must work before a single window appears on screen.

## Recommendation

**Build on GLES2 via `renderer_gl`.** Reasons, in order:

1. It is what the one proven Rust/Smithay compositor with the form-factor support
   we want actually uses. Following a working reference is worth more than a
   nicer API on paper.
2. It removes an unbounded task from the critical path. A renderer backend plus
   NVIDIA dmabuf import is not a prerequisite for anything we are trying to
   prove — E2 and E3 are the architectural bet, and they are about transforms
   and scripting, not about which GL API issues the draw calls.
3. Smithay's renderer traits are the abstraction boundary. If the transform layer
   is written against those traits rather than against GLES directly, swapping in
   a Vulkan backend later is a contained change.

**Constraint that follows:** E2 must not reach past the `Renderer`/`Frame` traits
into GLES specifics. If it does, the Vulkan option closes quietly.

## Correction to `docs/architecture.md`

The architecture document justified Vulkan by saying the transform-and-animate
layer "wants explicit, modern GPU control". That is weaker than it sounded.
Scaling, translating and cross-fading window textures is unremarkable work that
GLES2 does perfectly well. The honest reasons to want Vulkan later are compute
shaders, explicit synchronisation, and better multi-GPU handling — none of which
we need to build overview mode. The document has been corrected.

## What a Vulkan port costs now

Added 2026-09-30. `render.rs` says "the cost is recorded in the spike"; this
is that record. Each row is GLES or EGL that a Vulkan backend would have to
replace, on top of the renderer traits in the table above.

| What | Where | Why it is GLES |
|---|---|---|
| The render elements | `crates/solium/src/render.rs` | the element set is typed on `GlesRenderer`, because the warp element can only be GLES |
| Drawing through four corners (perspective, the genie) | `warp.rs` | raw GL through `with_context`: nothing on the traits places a texture's four corners independently |
| Rounded corners | `pass.rs`, `crates/effects/src/fragment.rs` | a GLSL ES 1.00 program compiled with `compile_custom_texture_shader`, which is GLES-only, drawn by a render element of its own |
| A closed window fading out | `remains.rs` | a render element of its own on `GlesRenderer` |
| QML on the GPU, the default since #147 | `qml/host.cpp`, `qml/paint.rs`, `surface.rs`, and the cursor and decoration scenes | Qt renders with OpenGL into a buffer the compositor allocated; the compositor then makes its own EGL context current again and waits on Qt's EGL fence |
| Everything that draws or reads back | `cursor.rs`, `decoration.rs`, `offscreen.rs`, `screencopy.rs`, `tty.rs`, `winit.rs` | each takes a `GlesRenderer` |

What would carry over: the transform (`present.rs`), the animation and layout
crates, the effects crate's vertex effects, and every script. QML's software fallback draws into
ordinary memory, so it needs only a new upload.

## Also found

At the time, the machine this spike was written on had no Rust toolchain,
neither on the host nor in the container then in use, and #10 waited on one.
**Resolved:** Solium builds natively with a `rustup` toolchain, or in the
project's build image with Rust mounted in from the user's own `rustup`
([dev/README.md, *Building*](../../dev/README.md#building)), and #10 is closed.
