# Architecture

Solium is a standalone Wayland compositor. Rust provides window state,
protocols and rendering primitives; Lua provides policy; QML provides window
frames and hosted desktop scenes. Effects are user-editable Lua and GLSL
folders. A separate desktop environment is not required.

This page describes the current implementation. [Known limitations](gaps.md)
covers incomplete behavior; the [roadmap](roadmap.md) covers future work.

## The one idea

A window's layout geometry and its presentation are separate. Overview,
layout transitions and a genie transform all place a window's texture somewhere
other than its layout rectangle and animate between the two.

Lua composes these primitives into modes. **If a new mode needs new Rust, the
transform layer is missing something:** extend the generic primitive rather
than hardcoding the mode. Floating, tiling, scrolling and overview ship as Lua
modules; their geometry calculations use the layout crate.

## Layers

| Layer | Responsibility | Main sources |
|---|---|---|
| Configuration | Bindings, modes, workspaces, shell policy | `crates/solium/lua/` |
| Desktop UI | Frames, wallpaper, preview shell, reusable QML types | `crates/solium/qml/` |
| Effect definitions | Geometry and fragment effects | `crates/solium/effects/` |
| Compositor | Protocols, state, input, script API, scene hosting, frame scheduling | `crates/solium/src/` |
| Reusable calculations | Animation, layout, deformation and effect formats | `crates/animation/`, `crates/layout/`, `crates/effects/` |
| External probes | Protocol checks from a separate Wayland client | `crates/wl-probe/`, `dev/` |

The animation, layout and effects crates keep calculations independent of the
compositor. They do not choose desktop policy.

## A window is a pane, not a client's surface

A pane has a stable identity and a layout slot. Its content can be a QML scene,
a mapped client or the retained picture of a closing window. The pane can exist
before an application connects, so launching reserves its place immediately.
When the matching client maps, it takes over the same pane.

Launch matching uses activation tokens and process ancestry. Relaunching an
existing single-instance application activates its existing window and removes
the unused launch pane. X11 launch adoption still has an open limitation; see
[gaps](gaps.md#session-and-reliability).

Resizing follows the requested rectangle while the client's last buffer fills
it. Closing can release the layout slot while the old picture animates out.
An application that refuses to close must be restored; this path still has
known issues. The architecture aims to respond immediately without requiring a
client to redraw first.

Smithay's `Space` retains mapped-client stacking and damage bookkeeping.
`sync_panes` reconciles it with the pane view. See `pane.rs`, `state/open.rs`,
`launch.rs` and `state/` for lifecycle handling.

## Configuration and scripting

The shipped `lua/init.lua` loads configuration, modes and the preview shell.
`~/.config/solium/user.lua` supplies overrides; `config.lua` documents defaults.
Lua reads compositor snapshots and issues commands through `script.rs`.
Commands request changes; subsequent state and events report the result.

Presentation commands work on stable window IDs and selections. Arrangements
come from `crates/layout`; Lua decides when to apply them. Configuration reload
keeps existing layout state where supported, and a failed load keeps the
previous configuration. `solium --check` validates loading and declared QML
scenes without starting a session.

The [configuration guide](ricing.md) covers usage. The API's source reference
is [meta/sol.lua](../crates/solium/lua/meta/sol.lua).

## Presentation, animation and input

`present.rs` represents presentation separately from layout geometry.
`crates/animation` supplies curves and springs, including initial velocity for
momentum. `crates/effects` supplies mesh/deformation calculations. Animations,
QML animations and QML timers share the compositor clock.

One stacking order serves rendering and pointer targeting. `render::drawn_on`
decides which output draws a window; `state/hit_test.rs` owns the hit test;
`focus.rs` owns keyboard focus changes. Hosted scenes take input only where
their live item trees claim it, with explicit pointer grabs and keyboard
requests. This lets windows remain interactive beneath unused shell regions.

Touch uses that same stacking order and live item hit test. Each slot captures
its lock surface, hosted grab or scene, frame control, move/resize operation,
script input grab or client at touch-down and keeps it until release or cancel.
Compositor contacts never enter the client seat, and pointer capture remains
independent. Hosted scenes and frame layers receive real Qt touch events;
move/resize operations share the pointer's geometry and completion paths.
Lua observes routed touch outside seat callbacks, with lock contacts excluded
even when unlocking cancels them. System gesture recognition is separate work
in [#252](https://github.com/Lilium-Linux/solium/issues/252).

## Rendering

Solium uses Smithay's concrete `GlesRenderer`. DRM sessions use hardware
outputs; the winit backend draws a nested compositor window. This is a GLES
implementation, not a renderer-independent backend or a Vulkan compositor.

Ordinary surfaces use the direct element path where possible. Rounded corners
are per-surface fragment programs and do not require a window capture.
Mesh transforms use cached captures of the window's surface tree. A capture
keeps a stable identity and is redrawn when its inputs change. It is taken at
the resolution the transform draws the window at, in steps down to an eighth,
so a window flying into an icon is not captured at full size while it is small;
a present's pixel shader is still told the full size the capture stands for. A
capture holds what the window's style paints past it, its layers' bleed, and
its mesh is built over that larger rectangle. Popups have
separate captures so they can extend beyond the window while following its
transform. `render.rs`, `clip.rs`, `warp/`, `capture.rs` and `pool.rs` contain
these paths.

User effects load from folders in two phases: Lua definitions at configuration
load, then GL programs when a render context is available. Shipped effects
include blur, glass, shade and genie. Backdrop effects and capture-based effects
have different costs; [the effects reference](../crates/solium/effects/README.md)
documents the stages, sources, caching, budgets and fallback behavior.

GLES-specific code includes mesh drawing, surface fragment programs, user
effect passes and Qt's EGL/dmabuf imports. Another renderer would require a
port of these pieces. The [Vulkan spike](spikes/2026-08-27-vulkan-on-smithay.md)
records the earlier investigation; it is not the current implementation.

## Hosted scenes and models

Qt Quick runs inside the compositor through a C ABI shim and
`QQuickRenderControl`. There is one QML engine, but each scene has its own
`QQuickWindow` and scene graph. Shared engine state makes a common theme and
live compositor data available; it does not make cross-scene reparenting work.

On hardware, a child-process startup probe selects GPU rendering when it
succeeds. Qt renders into GBM-allocated buffers imported as dmabufs. Software
rendering uploads changed images instead; it is also the nested default.
The render loop advances Qt with the compositor clock. Qt's poll descriptors
and timers join the compositor event loop so an idle desktop can wake for a
QML update without continuous redraw.

`sol.surface` creates hosted scenes, optionally per monitor. Scene size and
reserved work area are separate: a full-monitor scene can reserve just a bar's
edge. `import Solium` exposes attached properties, input/grab types and models.
Published data includes monitors, windows, workspaces, keyboard and pointer
state, applications (`Apps`) and desktop-folder entries (`Folder`). Keyed model
updates are batched into Qt before the scene renders.

The bundled preview uses these APIs for its bar, dock, search and desktop
icons. Users can disable it or select another hosted shell. External
`wlr-layer-shell` clients remain supported, but have the protocol limitations
listed in [gaps](gaps.md#external-shells-and-desktop-tools).
[Shell hosting and integration](shell-boundary.md) is the detailed API guide.

## Backends and session services

Nested development uses winit inside an existing desktop. Hardware sessions
use DRM, libseat and libinput. XWayland supplies X11 compatibility. Native
protocol handlers live in `state/` and the corresponding protocol modules.

The installed session wrapper integrates with systemd, D-Bus and XDG autostart.
The session lock is Solium's own by default (#281): a QML scene per monitor
(`lock.scene`), built at session start and kept quiet between locks, with the
shipped scene and then a compiled-in fallback standing in for one that fails,
never the desktop. Taken, it holds input and refuses captures at once, while
its scenes come down over the still-drawn desktop for at most
`lock.enter_ms`; `locked` waits for that entry to end. The password is typed
into Rust and checked by the `solium-auth` PAM helper; only its yes, or
logind's `Unlock`, ends the lock, and the scene then plays its exit over the
unlocked desktop for at most `lock.exit_ms`. Every way of locking goes
through one function, `Solium::lock_now` (before sleep, with no entry):
logind's `Lock` and `PrepareForSleep`, `idle.lock_after`,
`sol.lock_now` and the shipped `super+ctrl+l`. It takes the native lock, or,
when `lock.command` names an external locker, runs it, and that locker holds
the lock through session-lock. Before sleep, logind's delay inhibitor is
released when either kind of lock is confirmed on every monitor. Idle
notifications, inhibitors and monitor power are distinct from locking.

Programs launched by Solium inherit the environment saved before Qt modifies
it, with the session's display and activation variables added. The current
fallback desktop identifier is still `Lilium`; it is a legacy session/portal
integration name, not a dependency on a desktop environment. The full setup is
in [session integration](shell-boundary.md#how-the-rest-of-the-desktop-starts).

## Design rules

- **One authority for each state.** Models expose compositor state rather than
  maintaining another window manager beside it.
- **One clock.** Window animation, QML animation and timers use compositor time.
- **Geometry and pixels stay together.** An animation must not use independently
  updated copies of a scene's geometry.
- **Commands are not state.** A request is followed by the resulting state/event.
- **Generic primitives, editable policy.** Modes belong in Lua, window frames
  in QML and effects in Lua/GLSL. Rust supplies the mechanism.
- **A protocol has one concern.** A difficult interface name often signals
  unrelated responsibilities.
- **Window-coupled UI is hosted.** Decorations share the compositor's state and
  scheduling. Ordinary desktop applications may run as separate clients.

See [contribution rules](../CONTRIBUTING.md#rules-learned-the-hard-way) for the
review criteria behind these boundaries.

## Form factors

The goal is one compositor for desktop, laptop, tablet and phone, with different
configuration and input profiles. `SOLIUM_FORM_FACTOR` currently selects input
defaults; it does not supply a complete phone or tablet experience.

Client touch input exists, but compositor touch interaction, configurable
gestures and gesture-driven modes remain open. Cross-scene flights, morphs and
live icon anchors are also planned rather than implemented. See the
[roadmap](roadmap.md) for these directions.

## Non-goals

- Hyprland plugin compatibility or reuse of the old fork's implementation.
- A separate process painting Solium's window decorations.
- A different compositor or animation engine for each device class.
