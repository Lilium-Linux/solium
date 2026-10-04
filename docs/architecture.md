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
│  Panes: what the compositor calls a window   │  Rust
├──────────────────────────────────────────────┤
│  Presentation transform + animation clock    │  Rust
├──────────────────────────────────────────────┤
│  Render: Smithay's GlesRenderer (GLES2)      │  Rust
├──────────────────────────────────────────────┤
│  Smithay: protocols, input, backends         │  crate
└──────────────────────────────────────────────┘
```

Beside the stack, three crates hold the arithmetic and depend on nothing:
`crates/animation` (curves and springs), `crates/effects` (deformations and
fragment effects) and `crates/layout` (where windows go). Each is described
below.

### A window is a pane, not a client's surface

The compositor does not work with `smithay::desktop::Window` directly. It works
with a **pane**: an identity and a slot, whose *content* is a QML scene the
compositor draws, a mapped client, or the remains of one on its way out.

The reason is that a window's life should begin when the user asks for the
application, not when the application's client happens to connect. Everything
that follows from that — the slot reserved immediately, the other windows moving
aside, being able to close it while it is still loading, the application's
content simply appearing inside it — is impossible while the compositor's idea
of a window *is* the client's, because that type requires a surface to exist.

A pane keeps its id and its slot across all three kinds of content. **Adoption**
is the point: when a client belonging to a launch we started maps a toplevel, the
toplevel becomes that pane's content. Nothing is created, nothing is replaced,
and nothing else in the compositor notices — a decoration keyed by pane id
carries its animation straight through the handover.

**Panes are how the compositor answers every interaction at once.** The
application catches up inside that answer; the compositor never waits for it
first. Opening reserves the window's slot, frame and all, before the program
has even started. Resizing makes the rectangle being dragged the truth, and the
client's last picture fills it until the client redraws. Closing reflows the
other windows the moment the close is asked for, while the window fades where
it stood and the application decides whether it can go -- and if it says no, a
layout puts it back (#128, and `docs/modes.md` for the events). A feature that
has to wait for a client before anything on screen responds is the one this
design exists to rule out.

A client is matched to its pane by an activation token first and by walking up
from its process id second. The token is what survives a launcher that forks and
exits; the process walk is the fast path for everything that does not. A window
whose pane is older than the launch's is never matched by its token: that is an
application keeping one instance, answering the launch with a window it already
had. The launch's pane dissolves instead, the window stays where it lives, and
the scripts hear `activate` (#177, and `docs/modes.md`).

What a program Solium starts inherits is decided in `launch.rs` (#175). Its
environment is the one Solium was started with: noted at the top of `main`
(`launch::remember`), before Qt, EGL or any library they load writes into it,
and taken whole rather than scrubbed name by name. `Solium::spawn`
(`state/open.rs`) then sets only the session's own variables:
`WAYLAND_DISPLAY`; `DISPLAY`, naming Solium's Xwayland or removed when there
is none; `XDG_CURRENT_DESKTOP`, which is `Lilium` unless the session named
one; `XDG_SESSION_TYPE` when it is unset; and the activation token, as
`XDG_ACTIVATION_TOKEN` and `DESKTOP_STARTUP_ID`. In the child, every
descriptor above stdio is marked close-on-exec by one `close_range`
(`close_on_exec_above_stdio`), and the descriptors Xwayland and Qt's eglfs
would otherwise pass on are marked the same way (`xwayland.rs`, `host.cpp`).
The compositor's own Qt takes no input method from the session
(`qml::keep_input_methods_out` removes `QT_IM_MODULE` and `QT_IM_MODULES`),
while the programs it starts keep the user's.

`Space` has not gone away and is not going to. It stays underneath as the
authority on stacking and damage for a mapped client, because that bookkeeping
is worth keeping and not worth rewriting. `Panes` is the view *over* it, and
`sync_panes` is the single place the two are reconciled — so they cannot drift
apart at any of the sites that map or unmap.

QML is not a special case bolted on here. A pane whose content is a scene is as
ordinary as one whose content is a client, which is what makes a loading window,
a placeholder for an application that died, and a surface the compositor draws
for its own reasons one mechanism instead of three.

The design and the order the migration went in are recorded in
`docs/spikes/2026-09-06-window-provider.md`.

### Presentation transform

Every window carries a target rect, an opacity, a 4×4 matrix turned about a
pivot, an optional deformation and a depth, which the renderer uses **instead
of** its real geometry, plus an animation driving the current value toward the
target. Rounded corners are not part of it: they belong to the pane style, and
are drawn by a fragment pass (below).

Rules:

- **Transforms never change real geometry.** Leaving a mode restores the layout
  exactly, because the layout was never touched.
- **Input hit-testing follows the transform's rectangle and opacity**, or a
  window in overview is clickable where it was, not where it is drawn. The
  depth and the matrix are drawing only: a window raised over its neighbour, or
  tilted, is still clicked where its rectangle is, and one faded to nothing is
  not clicked at all.
- **One animation clock**, ticked from the render loop. Not per window, not per
  subsystem, not per script.

### The animation engine is a separate crate

`crates/animation` holds the curves, the springs and the timing, and depends on
nothing — no Wayland, no renderer, no Solium. That is not tidiness:

> **An animation you can only judge by launching a compositor is an animation
> nobody tunes.**

There will be many animations and many settings for them, so the engine has to
be usable outside the thing it animates. `dev/preview` compiles it to
WebAssembly and embeds it in a page that animates plain boxes through the
scenarios the compositor has — opening, overview, the switcher, a drag, a
maximise — with curve, duration and spring settings live.

That page stays a *test harness* rather than a mockup. Recreating the
compositor's chrome in it would be a second copy of the design with the same
drift problem as a second copy of the curves, and motion is easier to judge
without decoration.

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

### So are the shapes

`crates/effects` is the same crate again for the other half of a transform: the
vertex deformations a rectangle and a matrix cannot express — a genie, and
whatever fold, curl or page turn comes next. Same empty `[dependencies]`, same
unit tests, same wasm preview with live sliders, and the same reason.

It has a second reason the animation engine does not, and it is the harder one:
**the damage tracker needs a deformed window's bounding box before anything is
drawn, and a shader cannot tell it one.** So a vertex function stays CPU-side
and parametric — a name and some numbers, never user code. A *fragment* effect
could one day be code a style supplies, because a bad one is a wrong picture
and a bad damage rect is a corrupt screen; today there is one, rounded
corners, and its shader is compiled into the crate. See
`docs/design/2026-09-12-panes-and-effects-design.md`.

The split with the compositor is the anchor. A deformation morphs between two
rectangles, and the far one is named rather than given: `deform = { effect =
"genie", to = { window = id } }` carries an *identity*, which `Solium::aimed_at`
resolves on the frame that draws it, and `to = { surface = name }` aims at a
`sol.surface` scene the same way. A rectangle read out of a Lua table when the
binding was pressed aims at where a dock icon was half a second ago, which is
the stale-copy failure the anchors planned in `docs/shell-boundary.md` are meant
to rule out for a hosted dock. `crates/effects` never sees the identity — it has
no idea what a pane is, which is what keeps it testable without a session.

`Deform::from_name` is what scripts bind to, exactly as `Curve::from_name` is,
and `script::shipped` checks the shipped Lua against both.

### Fragment passes

A pane style can round the client itself, with `client.radius` in its
`Pane.qml`. That is neither a transform nor QML: the client's pixels are the
application's, so the compositor draws them through a fragment program of its
own. `crates/effects/src/fragment.rs` says what an effect reads — nothing, the
node's own pixels, or what is beneath it — and keeps the rounded-corner shader
as source text. `pass.rs` compiles it, renders the client's surfaces into a
texture kept on the pane, and draws that through the program in the client's
place. An effect that reads nothing needs no pass at all.

The texture is the cost. Every visible rounded window pays a pass every frame,
which is why `render::prepare` captures no pane that no monitor shows. What is
beneath a node, the input a blur would need, is named in `fragment.rs` and
nothing constructs it yet; rounded corners are the one effect there is.

### The arrangements are a crate too

`crates/layout` is the third engine crate with nothing in `[dependencies]`:
where each window goes, given how many there are and how much room. It holds
master-stack, the tiling tree, the scrolling strip and the grid overview lays
windows out on, and scripts reach them as `sol.layout.master_stack`, `.tree`,
`.scroller`, `.strip`, `.scrolling`, `.scroll_to` and `.grid`. The preview page
arranges its boxes with the same code.

So "modes are scripts" has a precise meaning. The Lua decides which
arrangement, when, and for which windows, and keeps what it decided; the
arithmetic of each arrangement is Rust, tested without a session, and no
script has to derive it again.

### And a transform names a selection

`present::Frame` says how *one* pane is drawn. `group::Shift` says how a **named
set** of things is drawn — windows, `sol.surface` surfaces, whole monitors —
and the two compose: a member is drawn at its own transform plus every selection
it is in.

That is one mechanism rather than three. A whole-screen effect is a selection
that is an output; a set of windows moving as one is a named set; and a
workspace is its windows plus its own background, which is why the workspace
slide stops leaving the wallpaper behind. Nothing in the compositor knows what a
wallpaper is; it travels because `workspaces.lua` put it in the selection.

Two decisions worth knowing:

**Membership lives on the group**, not in a table keyed by `PaneId` beside the
panes. A group owns its members and its own in-flight transform and both leave
when it does, so a selection naming a window that has closed resolves to nothing
and costs a `u64`. That is the same rule `Pane` follows for its frame, its
timers and its capture buffer.

**A node in no selection pays nothing.** `Shift::apply` on an identity shift
returns the frame it was given, and the first thing anything asks is whether any
selection exists at all. A compositor that routed every window through new
arithmetic to support a group nobody declared would have made every frame worse.

### One stacking order, one hit test

`stack.rs` is the one list of what is drawn over what on a monitor, topmost
first: `overlay`, then a fullscreen window when one covers the bars, then
`top`, the windows, `bottom` and `background`. At each layer a client's layer
surfaces are over a script's `sol.surface` scenes, because the client was
installed on purpose. The renderer draws in that order and every hit test above
or below the windows asks in it, so what is on top is what is clicked (#141,
#142). `fullscreen.covers = "none"` keeps the bars over a fullscreen window.

A script's scene is on top only where its own items take the point. When a hit
test reaches a `sol.surface` band, the compositor asks that scene's live item
tree, which answers `Hit::Nothing`, `Hit::Hover` or `Hit::Press`
(`qml/hosted.rs`). `topmost_above` passes what the event asks for:
`Asking::Hover` for motion, `Asking::Press` for a button or the wheel. So a
strip that takes only hover leaves presses to the window under it (#173).
While a scene holds a press it took, or holds a `Grab`, no client has the
pointer (`surface_at` in `state/hit_test.rs`), and a press anywhere is the
grab's to take or to be dismissed by (`claim_under`). A touch never lands on a
scene (`touch_under`; see #181 under [Form factors](#form-factors)).
`docs/shell-boundary.md`, "What a hosted shell is given", has the behaviour.

Among the windows, one predicate says whether a window is under a point:
`owns`, in `state/hit_test.rs` — on a screen that draws it, and inside what it
paints there. `Solium::window_under` asks it from Rust and `sol.window_at` from
Lua, and both look through what is left of a window whose client has gone.

**The renderer is GLES, and the code says so.** Scaling and cross-fading
textures is unremarkable work that GLES2 does well, and Smithay has no Vulkan
renderer to choose instead. Drawing does not stay behind Smithay's generic
`Renderer` and `Frame` traits, because three things it needs are not on them:

- drawing a texture through four independent corners, for perspective and the
  genie (`warp.rs`, the only Rust file that makes raw GL calls);
- a fragment program of Solium's own, for rounded corners (`pass.rs`, which
  compiles the GLSL ES source kept in `crates/effects`);
- the EGL context and fence that QML on the GPU shares with Qt (`qml/paint.rs`,
  `surface.rs`).

So the render elements are typed on `GlesRenderer` (`render.rs`). What stays
free of any renderer is the arithmetic: `present.rs` and the animation, effects
and layout crates depend on no renderer, and the effects crate carries the
rounded-corner shader only as source text. A Vulkan backend would need its own
element set, warp, pass programs and QML import, which makes it a port rather
than a swap; the [Vulkan spike](spikes/2026-08-27-vulkan-on-smithay.md) lists
the pieces. The one reason left to pay for that is compute shaders. Explicit
sync ([#59](https://github.com/Lilium-Linux/solium/issues/59)) is a protocol
Smithay offers on GLES too, and Smithay's multi-GPU renderer
([#63](https://github.com/Lilium-Linux/solium/issues/63)) is built on GLES.

### Hosted scenes

`scripted.rs` keeps what scripts declare with `sol.surface`, and builds one
live instance of a surface on each monitor it is on, as the monitor is
placed; an instance goes with its monitor. A surface declared again writes its
changed properties into the live scene instead of rebuilding it (#161).
`surface.rs` is one hosted scene: a QML file, the area it was placed in, and
its pointer events. `qml/hosted.rs` is the compositor's half of what a scene
and the compositor say to each other: properties written in place, the
monitor the instance is on, the models' rows, pointer events, what the
scene's items claim at a point, its reserve, its grabs, its keyboard wants
and the keys it is told, and the actions it sends with `Solium.send`, whose
data `json.rs` reads. `qml/keys.rs` puts the compositor's buttons, modifiers
and keys in Qt's terms.

`state/hosted.rs` reads what the scenes report, once a pass (`settle_scenes`),
and applies it. A reserve goes into the work area (`reserved_on`, which
`work_area_on` in `state/monitors.rs` adds to the layer-shell zones) and
re-flows the layout once (#162). A `Grab` is held or dismissed
(`settle_grabs`, `dismiss_hosted_grab`). The keyboard is held for a scene, its
keys are delivered and repeated at the keymap's rate, and it is given back
(`settle_keyboard`, `deliver_scene_key`, `repeat_scene_key`,
`end_keyboard_hold`) (#163). The actions the scenes sent go to the `surface`
listeners, each in a dispatch of its own (`settle_actions`), and `sol.act`'s
verbs become the commands that do them (`act`); `state/commands.rs` tells each
attempt's `done` once the dispatch that ran it is applied.
`docs/shell-boundary.md`, "What a hosted shell is given", has the behaviour.

### Scripting

Lua, and **modes really are scripts** — `lua/overview.lua` is overview, and the
compositor contains no code that knows what overview is. The proof is that the
Rust that used to implement it was deleted, not wrapped.

The keyboard pill is the same claim for something drawn.
`lua/keyboard_indicator.lua` is the policy and reads `keyboard.indicator`,
which no Rust names. It draws with `qml/Solium/KeyboardPill.qml`, through
`KeyboardPillLayer` (`qml/Solium/KeyboardPillLayer.qml`), which every shipped
pane style adds, or on an overlay `sol.surface` (`qml/indicator/keyboard.qml`),
and taking `require("keyboard_indicator")` out of `init.lua` takes the pill
away. Its tests are scenarios: Lua files in `crates/solium/tests/scenarios/`,
played by `scenario.rs` under `cargo test`. So a feature written as
configuration is tested without Rust that knows about it.

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
  `sol.status`. Today that name is only kept and logged: nothing draws it, and a
  shell has no way to read it yet.

**No compositor config key per mode.** That is how a mode set becomes closed.

### What the compositor publishes

The models scenes read are built from the compositor's own state (`models/`),
so nothing is mirrored. Each model is diffed by key (`models/diff.rs`) and
sent to Qt as one batch, every row's values written before any row is
announced (`qml/rows.cpp`), once a frame, in `render.rs` just before
`qml::tick`. Two exist today: the monitors, read as `Solium.monitor`
(`models/monitors.rs`), and the keyboard, read as the `Keyboard` singleton
(`models/keyboard.rs`).

`text_input.rs` answers `zwp_text_input_v3` itself, not through Smithay's
module, which discards every request while no input method runs. It keeps only
which field is live and its caret, and publishes that as `sol.text_input()` and
`sol.on("text_input")` in the global space, and as `caret` in a pane's own
space; it draws nothing. `keyboard_change.rs` tells the configuration
(`sol.on("keyboard")`) and the scenes (`Keyboard.changed(what)`) when the
layout, Caps Lock or Num Lock really changes. It never fires for ordinary
typing, and does not tell the configuration while the session is locked. Every
decoration layer is also handed `values` from `sol.pane_values`, a general
channel from Lua that the compositor does not interpret, and a layer whose root
says `dormant` is not drawn, and in software lets go of its image
(`LayerScene::sleeps` in `decoration.rs`).

## Design rules

Carried from the Hyprland-fork post-mortem. Each one cost real time to learn.

**One authority for any piece of state.** Two models of window state — the
compositor's and a mirror in a helper process — produced a deadlock where a
decoration was required to learn the geometry that decided whether to create a
decoration.

**Geometry the compositor animates against must arrive with the frame that shows
it.** Never a side channel. A dock icon rect sent whenever the shell chooses and
applied whenever it arrives is a mirror, and mirrors drift. For a hosted dock
the plan is that there is nothing to send: its QML names an icon, and the
compositor reads where it is from the same engine after layout. If the dock is
a layer-shell surface committing frames, its icon rects ride along with that
commit and are applied atomically with it. Neither is built yet.

**Commands are not state.** `focus_window` is a verb; the resulting focus change
comes back through the event stream, not as a reply.

**A protocol carries one concern.** When a name is hard to choose, the interface
is doing too much.

**In-process for anything window-coupled.** Decorations, the animation engine,
the bar and mode overlays run inside the compositor. Notifications, settings
UI, launcher and media controls are ordinary clients — none of them touch
window geometry.

### One design system, one engine

**Decided 2026-09-05.** The compositor hosts a single QML engine, and every
surface the desktop draws is a scene in it: window decorations, and a hosted
shell's bar, dock and launcher. The ones that are themed import one
`Solium.Theme` singleton; the fallback pointer and the default wallpaper keep
fixed colours.

The requirement that forces this is not "consistent styling" — it is that an
object must be able to *move* from the dock into a titlebar. Two processes
painting their own pixels cannot do that; the best available would be a fake.
It is not built, and one engine does not make it a reparent: every scene has a
`QQuickWindow` of its own, so each is its own scene graph. The plan is a flight
or a morph, a third, live instance of the same component drawn over both ends
while they are hidden; one engine is what lets that instance be the component
itself. See `docs/shell-boundary.md`.

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
A QML `Timer` rides that clock too whenever anything else animates, so between
frames the event loop that serves Qt advances it, rather than going around it.
That is `qml/wake.rs`: Qt's own poll set is one descriptor in the compositor's
event loop, which turns readable when Qt's next timer is due or a descriptor Qt
waits on is ready. Qt is then served on the compositor's clock (`qml::drain`),
and a scene that changed asks for one frame and no more.

Beside `host.cpp` are three files that register native types under the same
`Solium` URI as the shipped QML module, so one `import Solium` reaches both:
`attached.cpp`, the attached `Solium` object (`Solium.monitor`,
`Solium.input`, `Solium.keyboard`, `Solium.surface.reserve`) and the `Grab`
type; `rows.cpp`, the list model that applies the keyed row batches Rust
sends; and `keyboard.cpp`, the `Keyboard` singleton. `attached.h`, `rows.h`
and `keyboard.h` declare `Q_OBJECT` types, so `build.rs` runs Qt's moc on them
(`MOC_HEADERS`; `QT_MOC` names moc when it is not found). `dev/wirecheck`
compiles the same files from its own `build.rs`, with its own copy of that
list, so a new header that declares a `Q_OBJECT` type goes into both lists,
and a new file beside `host.cpp` into both builds.

No Qt QPA plugin available here will adopt the compositor's EGL context, so
the GPU route runs the other way round: the compositor allocates a buffer
through GBM and Qt's OpenGL scene graph renders into it as an imported dmabuf.
That is the default wherever a trial render in a child process passes at
startup. Where it does not, Qt's *software* scene graph renders into a memory
buffer that is uploaded instead — chrome is small and only re-uploaded when Qt
reports it changed. Qt fixes the choice for the life of the process, which is
why it is made before Qt starts; `dev/README.md`, *QML on the GPU*, has how to
force either one. See `docs/spikes/2026-09-04-qml-in-compositor.md` for the
original measurement and the two Qt traps it hides.

**A bar reserves its height whether it is a client or hosted.** The work area
is what every layer surface's exclusive zone and every hosted surface's
`reserve` leave of a monitor, so windows are placed beside a bar, never under
it. A hosted surface's reserve is apart from its size: a whole-monitor shell
reserves only its bar's edge (`docs/shell-boundary.md`, "Room of its own").
And a fullscreen window covers the `top` layer, bars included, unless
`fullscreen.covers` says otherwise.

## Form factors

One compositor, one layout engine, different input profiles and default modes.
Only the input profile is chosen today: `SOLIUM_FORM_FACTOR` sets click and
touch focus, focus-follows-mouse, the drag modifier and natural scrolling
(`input/profile.rs`). Every form factor starts floating, and there is no app
switcher, no peek and no compositor gesture yet. Touch reaches a client's
window (checked with Firefox on a Surface Pro 7), but nothing the compositor
draws reacts to it: frame buttons, a hosted shell's scenes, overview and the
screen edges ([#181](https://github.com/Lilium-Linux/solium/issues/181)).
Gestures are E7 ([#7](https://github.com/Lilium-Linux/solium/issues/7)), after
v0.1.0. The table is the intent, for E7:

| | Primary input | Default mode, intended |
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
