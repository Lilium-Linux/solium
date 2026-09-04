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

**Backend independence.** The transform layer talks to Smithay's `Renderer` and
`Frame` traits, never to GLES directly. Scaling and cross-fading textures is
unremarkable work that GLES2 does well; the reasons to want Vulkan later are
compute shaders, explicit sync and multi-GPU, none of which overview mode needs.
Reaching past the traits into GLES specifics closes that option quietly.

### Scripting

Lua. Modes are scripts, so their settings belong to the scripts; the compositor
supplies only primitives — default easings, gesture bindings, and which script
is bound to which trigger.

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

**In-process for anything window-coupled.** Decorations, the animation engine
and mode overlays run inside the compositor. Notifications, settings UI,
launcher and media controls are ordinary clients — none of them touch window
geometry. Dock and bar sit between: separate processes that publish geometry,
never pixels.

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
