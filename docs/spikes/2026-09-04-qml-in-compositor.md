# Spike: hosting QML inside the compositor

**Date:** 2026-09-04
**Outcome:** works, via Qt's **software** scene graph. The GPU path is blocked
on Qt, not on us, and the block is recorded below so nobody re-derives it.

## Question

The shell and window decorations are to be authored in QML. Out-of-process was
already tried in the Hyprland fork and rejected — ~15 fps at 39% CPU after
optimisation, with the process boundary as the ceiling. So: can Qt Quick render
inside a Rust/Smithay compositor?

## What works

Yes, in-process, with no IPC:

- A small C++ shim (`crates/solium/qml/host.cpp`) behind a C ABI, compiled by
  `build.rs`. No moc step — nothing in it needs `Q_OBJECT`.
- `QQuickRenderControl` + `QQuickWindow`, no visible window, `offscreen` QPA.
- Animations driven by **the compositor's clock** through a `QAnimationDriver`
  the render loop advances by hand. Not Qt's timer: there is one clock in this
  compositor, and QML animating off a second one would drift against every
  window transform beside it.
- No Qt event loop. Nothing calls `exec()`, so `QCoreApplication::processEvents`
  is pumped once per frame or queued work never runs.

## What does not work: rendering into our own GL texture

The GPU path needs Qt to render into a texture the compositor can sample, which
means Qt must use *our* EGL context. The only way to hand Qt a foreign context
is `QNativeInterface::QEGLContext::fromNative`, and **that call is implemented
by the QPA platform plugin, not by Qt Gui.** Measured here, Qt 6.11.2:

| Platform plugin | `fromNative` |
|---|---|
| `offscreen` | returns null |
| `eglfs` | returns null |

With no plugin to adopt the context, Qt renders on a context of its own and the
texture it produces is not one we can sample — the two contexts share nothing.

Worse, and worth knowing: **`QQuickRenderControl::initialize()` makes a GL
context current on the calling thread on its way to failing.** After that, the
compositor's own `eglMakeCurrent` fails with `BAD_ACCESS` — "another window API
already has a current context" — and every subsequent frame fails to render.
The compositor looked broken; it had simply lost its thread's context to Qt.

## The path taken

Qt's **software** scene graph has no context requirement at all: QML rasterises
into a `QImage` via `QQuickRenderTarget::fromPaintDevice`, and the compositor
uploads it as a `MemoryRenderBuffer`. For chrome this is cheap — a 1600×34 bar
is 218 KB — and it is uploaded only when Qt says the scene changed.

Two traps, both of which cost real time here:

**`QQuickRenderControl::initialize()` returns false under the software
adaptation and must not be called.** It is an RHI call; the software renderer
has no RHI. Treating the `false` as failure hid a working scene graph behind a
wrong check, and calling it at all breaks the compositor's context as above.

**Do not clear the target image between frames.** Qt's software renderer
repaints only the regions it considers dirty. Clearing every frame erases
everything that has not changed, leaving *only* the parts that animate — which
presented as a bar with one moving dot and no background or text, and looked
exactly like a broken upload path. The image is cleared once, on creation and
resize, and Qt owns it after that.

**The software adaptation is selected by name**, `setSceneGraphBackend("software")`
or `QT_QUICK_BACKEND=software` — *not* by `setGraphicsApi(Software)`, which
selects between OpenGL, Vulkan, Metal and D3D and will accept `Software`
without changing the adaptation. Getting it wrong fails later, in
`initialize()`, with nothing pointing at the cause.

## Two ways back to the GPU, when it is worth it

1. **A QPA plugin that adopts EGL contexts.** `QT_QPA_PLATFORM=wayland` is a
   candidate for nested development but not for a real session, where there is
   no host compositor. Worth re-testing when the DRM backend lands.
2. **A dmabuf-backed render target.** Qt renders on its own context into a
   dmabuf; we import it with `ImportDma`. No context sharing needed, and it is
   how two processes would do it anyway — but it needs both sides on the same
   GPU and correct format/modifier negotiation, which is exactly where this
   kind of work stalls on NVIDIA.

Neither is on the critical path. Chrome is small, and the upload only happens
when it changes.

## Measured

`docs/qml-bar.png` — the bar rendered by Qt inside Solium, showing live
compositor state: window count, the focused window's xdg-shell title, and the
compositor's own clock.
