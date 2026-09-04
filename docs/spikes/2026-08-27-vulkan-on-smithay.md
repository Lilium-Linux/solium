# Spike: Vulkan renderer on Smithay — which path?

**Issue:** #9
**Date:** 2026-08-27
**Outcome:** start on GLES2. Vulkan is a later, optional port.

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

`ImportDma` on NVIDIA is the risky one. Every Wayland client hands us a dmabuf,
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

## Also found

**There is no Rust toolchain on this machine** — not on the host, not in the
`lilium` container. Install user-local via `rustup`, matching how the other
toolchains here are installed (sudo requires a password on this box).

That is a prerequisite for #10 and everything after it.
