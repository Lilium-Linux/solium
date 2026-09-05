# Architecture

## The one idea

Mission Control, the iOS app-switcher card stack, the iPadOS thumbnail grid,
macOS dock hover-peek, and the iOS icon→window genie look like five features.
They are one operation:

> place a window's texture somewhere other than its real geometry, and animate
> between the two

- **Overview** — every window scaled onto a grid
- **App switcher** — every window in a row, scrubable
- **Peek** — one window scaled at a cursor position
- **Genie** — one window interpolating between an icon rect and its own rect
- **Tiling / scrolling** — the same, ending at a layout slot instead of a thumbnail

Implementing these separately is why compositors end up with modes that animate
inconsistently. Solium builds the operation once and expresses every mode as a
script over it. **If a new mode needs new Rust, the transform layer is missing
something** — that is the test.

## Layers

```
┌──────────────────────────────────────────────┐
│  Scripts: modes, layouts, gestures           │  Lua
├──────────────────────────────────────────────┤
│  Script API: enumerate, transform, bind      │  Rust
├──────────────────────────────────────────────┤
│  Presentation transform + animation clock    │  Rust
├──────────────────────────────────────────────┤
│  Render: Smithay renderer traits (GLES2)     │  Rust
├──────────────────────────────────────────────┤
│  Smithay: protocols, input, backends         │  crate
└──────────────────────────────────────────────┘
```

### Presentation transform

Every window carries a target rect, opacity, corner radius and z-order that the
renderer uses **instead of** its real geometry, plus an animation driving the
current value toward the target.

Rules:

- **Transforms never change real geometry.** Leaving a mode restores the layout
  exactly, because the layout was never touched.
- **Input hit-testing follows the transform**, or a window in overview is
  clickable where it was, not where it is drawn.
- **One animation clock**, ticked from the render loop. Not per window, not per
  subsystem, not per script.

### The animation engine is a separate crate

`crates/animation` holds the curves, the springs and the timing, and depends on
nothing — no Wayland, no renderer, no Solium. That is not tidiness:

> **An animation you can only judge by launching a compositor is an animation
> nobody tunes.**

There will be many animations and many settings for them, so the engine has to
be usable outside the thing it animates. `dev/preview` compiles it to
WebAssembly and embeds it in a page that animates **mock windows** through the
scenarios the compositor has — opening, overview, the switcher, a drag, a
maximise — with curve, duration and spring settings live.

The engine is *called* from that page rather than reimplemented in it. A copy of
the curves in JavaScript would drift the first time either side changed, and the
drift would be invisible because the page would still animate plausibly. The
wasm and native answers are checked against each other instead of assumed.

The page owns the geometry — where a window starts and ends — because that is
layout and belongs to the compositor and its scripts. The engine only ever
answers "how far along?", which is exactly the split inside the compositor: it
reports progress, and the caller interpolates.

The compositor keeps only the part that needs a compositor: which rectangle a
window is travelling between. `Curve::from_name` is what scripts bind to, so a
curve added to the engine is immediately available to every mode and to the
preview without touching the renderer.

**Backend independence.** The transform layer talks to Smithay's `Renderer` and
`Frame` traits, never to GLES directly. Scaling and cross-fading textures is
unremarkable work that GLES2 does well; the reasons to want Vulkan later are
compute shaders, explicit sync and multi-GPU, none of which overview mode needs.
Reaching past the traits into GLES specifics closes that option quietly.

### Scripting

Lua, and **modes really are scripts** — `lua/overview.lua` is overview, and the
compositor contains no code that knows what overview is. The proof is that the
Rust that used to implement it was deleted, not wrapped.

The boundary is one module, `script.rs`, and it is shaped so a mode never
learns a window is a Wayland surface:

- **Reads are a snapshot.** Windows, work area and cursor are built fresh per
  dispatch and handed to Lua by value.
- **Writes are commands.** `sol.present` queues; the compositor drains the queue
  after the handler returns. A script cannot mutate the compositor directly, so
  its idea of a window's geometry cannot drift from the compositor's — and no
  borrow of compositor state is alive while Lua runs, which is what stops a
  script re-entering the seat mid-dispatch and deadlocking.
- **Ids, not indices.** A window's id is stable for its lifetime and never
  reused, so a script holding one across frames cannot address a different
  window with it.
- **The compositor does not know what modes exist.** A script names itself with
  `sol.status`, and the bar shows whatever it says.

**No compositor config key per mode.** That is how a mode set becomes closed.

## Design rules

Carried from the Hyprland-fork post-mortem. Each one cost real time to learn.

**One authority for any piece of state.** Two models of window state — the
compositor's and a mirror in a helper process — produced a deadlock where a
decoration was required to learn the geometry that decided whether to create a
decoration.

**Geometry the compositor animates against must arrive with the frame that shows
it.** Never a side channel. A dock icon rect sent whenever the shell chooses and
applied whenever it arrives is a mirror, and mirrors drift. If the dock is a
layer-shell surface committing frames, its icon rects ride along with that
commit and are applied atomically with it.

**Commands are not state.** `focus_window` is a verb; the resulting focus change
comes back through the event stream, not as a reply.

**A protocol carries one concern.** When a name is hard to choose, the interface
is doing too much.

**In-process for anything window-coupled.** Decorations, the animation engine,
the bar and mode overlays run inside the compositor. Notifications, settings
UI, launcher and media controls are ordinary clients — none of them touch
window geometry.

### Chrome is QML, hosted in-process

**Decided 2026-09-04.** The bar and window decorations are authored in QML and
rendered by Qt's scene graph *inside the compositor process* — the model KWin
uses for Aurorae. Out-of-process was tried in the Hyprland fork and measured at
~15 fps and 39% CPU, with the process boundary as the ceiling.

Concretely: a C ABI shim (`crates/solium/qml/host.cpp`), `QQuickRenderControl`
with no visible window, and animations driven by **the compositor's clock**
through an animation driver the render loop advances. That last part is not a
detail — QML animating off Qt's own timer would drift against every window
transform beside it, which is the same mistake as having two animation clocks.

Rendering goes through Qt's *software* scene graph and is uploaded as a memory
buffer, because no Qt QPA plugin available here will adopt the compositor's EGL
context. See `docs/spikes/2026-09-04-qml-in-compositor.md` for the measurement,
the two Qt traps it hides, and the two routes back to the GPU. Chrome is small
and only re-uploaded when Qt reports it changed, so this is not on the critical
path.

**The bar reserves its height.** `work_area` excludes it, so windows are placed
below it, never under it. A bar windows slide beneath is a panel; a bar that
owns its strip of screen is part of the desktop.

## Form factors

One compositor, one layout engine, different input profiles and default modes.

| | Primary input | Default mode |
|---|---|---|
| Desktop | pointer + keyboard | floating or tiling |
| Laptop | trackpad gestures | tiling with overview |
| Tablet | touch | scrolling with app switcher |
| Phone | touch | one window, switcher on gesture |

The form factor selects defaults and an input profile. It does not fork the
codebase, and it must not fork the animation engine.

## Non-goals

- **Hyprland plugin compatibility.** Abandoned deliberately. The aim is a
  compositor complete enough not to need plugins; a plugin protocol can come
  later on its own terms.
- **Reusing the Hyprland fork's code.** The ideas carried over; the code did not.
- **An out-of-process shell painting window decorations.** Tried, measured,
  rejected: ~15 fps at 39% CPU after optimisation, and the process boundary was
  the ceiling.
