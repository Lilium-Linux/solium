# Roadmap

Epics in dependency order. Each should leave the compositor running and
testable.

**E1 to E5 have landed, and E6 closed as "modes as scripts".** The compositor
boots on hardware, transforms and animates windows through one engine, is
scripted in Lua, has floating, tiling and scrolling layouts, draws its own
decorations from QML, and expresses every mode as a script over the transform
— which was the bet, and it held: no mode has code of its own in the
compositor. Overview needed no new Rust at all. Tiling and scrolling choose
among arrangements that are Rust, in `crates/layout`; the Lua decides which,
and when.

E7 to E11 are open. So is the work that turns a working compositor into a usable
one. The [`daily-drive`
label](https://github.com/Lilium-Linux/solium/issues?q=is%3Aissue%20is%3Aopen%20label%3Adaily-drive)
is that list, prioritised by whether an application can be used at all without
the thing, and [What is left](#what-is-left-to-be-a-complete-compositor) below
says why the order is what it is.

For what has to be true before anyone else can run this, and the order to do it
in, see [beta.md](beta.md).

Each epic below keeps its original acceptance criteria, because what "done"
meant at the time is the more useful record.

## E1 — Boots and shows a window

Smithay skeleton with a Vulkan renderer — the plan at the time; it shipped on
GLES instead, for the reasons in `docs/spikes/2026-08-27-vulkan-on-smithay.md`.
Winit backend for development, DRM for real hardware. One xdg-shell client
rendered on screen, keyboard and pointer input working, clean shutdown.

Done when: a terminal opens in Solium under a nested session and accepts typing.

## E2 — Presentation transform and animation clock

The core abstraction. Every window gains a target rect, opacity, radius and
z-order used instead of real geometry, with one animation clock ticked from the
render loop. Input hit-testing follows the transform.

Done when: a hardcoded call scales a window to half size in a corner, animates
smoothly, and clicking it still focuses it. Layout is unchanged on reset.

## E3 — Lua scripting surface

Embed Lua. Expose enumeration (`windows`, `monitors`, `cursor`), transforms
(`present`, `present_clear`, `animate`), and binding (`on`, `grab_input`).

Done when: **overview mode exists as a script only**, no Rust changes. A key
scales every window onto a grid, a second press restores them, clicking a
thumbnail focuses that window.

This epic is the architecture's proof. If overview needs Rust, E2 is incomplete.

**Done.** `lua/overview.lua` is overview; `src/mode.rs` was deleted rather than
wrapped. Leaving restores the layout to the pixel — measured, not asserted.

## E4 — Layout engine: floating, tiling, scrolling

Three layouts behind one interface, switchable at runtime. Scrolling follows
niri's model: an infinite horizontal strip with a viewport.

Done when: all three are usable for real work and switching between them
animates through the transform layer rather than snapping.

## E5 — Decorations, hosted in-compositor

Window frames drawn by the compositor, reserving their own space so the client
shrinks and frame plus window read as one object. Titlebar, buttons, shadows,
rounding.

Done when: windows have working titlebars whose buttons respond, and the client
area is never covered.

## E6 — Modes as scripts

App switcher, peek-on-hover, and the icon→window genie — each a script over E2,
each shipping with the compositor as a default that users can replace.

The genie needs a dock icon rect. That rect arrives with the dock's frame
commit, never on a side channel.

Done when: all three work, and none required new Rust.

**Closed as "modes as scripts"**, which is what shipped: overview, tiling and
scrolling are scripts, and `modes.lua` decides which one is in charge. The app
switcher and peek were never written. The genie is an effect, but its only
binding, `super+m`, is a development one aimed at a fixed rectangle, because
there is no dock icon to aim at. The rule about the dock's rectangle was
written for a dock outside the compositor's engine. A dock hosted in it would
name its icon from QML, which is planned and not built; only a dock that runs
as its own program would need a protocol. See
[tier 3](#3-the-part-that-is-not-parity).

## E7 — Touch, gestures, form factors

Touch input, gesture recognition, and per-form-factor input profiles and mode
defaults. niri is the reference for touch on Smithay.

Done when: the same binary is usable by touch on a tablet and by pointer on a
desktop, with no code fork.

**Status.** Open. Touch reaches applications' windows and a tap focuses the
window under it (seen on a Surface Pro 7), but nothing the compositor draws
(frame buttons, a hosted shell, the overview, window edges) reacts to touch,
and scripts hear none
([#181](https://github.com/Lilium-Linux/solium/issues/181)). Gesture
recognition is still this epic and comes after v0.1.0.

## E8 — Settings

Settings surface reading tunables declared by scripts rather than a fixed
compositor schema, so adding a mode adds its settings automatically.

Done when: changing a setting is visible immediately, with no restart.

### [E9](https://github.com/Lilium-Linux/solium/issues/84) — The transform layer and the layouts, made right

Three places hold a window's position and nothing makes them agree. Filed after
the same shape of bug arrived three times from hardware. Includes the resize
problems, and a spike reading how Hyprland and niri each solved resize.

Done when there is one answer to "where is this window", and a mode that gets
it wrong fails loudly rather than quietly.

**Status.** Open. What has landed: one function, `Solium::move_pane`, makes the
three copies agree whenever a pane is placed; during a resize the pane's slot,
not the client's size, is the authority
([#113](https://github.com/Lilium-Linux/solium/issues/113),
[#120](https://github.com/Lilium-Linux/solium/issues/120),
[#123](https://github.com/Lilium-Linux/solium/issues/123),
[#124](https://github.com/Lilium-Linux/solium/issues/124)); every hit test asks
one question, whether a pane owns the point on a screen that draws it
([#134](https://github.com/Lilium-Linux/solium/issues/134)); and the layouts
survive a reload ([#118](https://github.com/Lilium-Linux/solium/issues/118),
[#129](https://github.com/Lilium-Linux/solium/issues/129)). What is left is the
done-when itself: showing that every path goes through that one answer, and
making a wrong one fail loudly. No spike on how Hyprland and niri solved resize
is in the repository.

### [E10](https://github.com/Lilium-Linux/solium/issues/85) — Atrium, and what modes still cannot do

A stage-manager mode, named for the central hall of a Roman house rather than
for Apple's word. The point is not the mode: it is the strongest test the
architecture has been given, and whatever it turns out to need in Rust is
exactly the list of what the transform layer still lacks.

Done when atrium is a script somebody can rewrite, and nothing in it needed a
special case in `render.rs`.

**Status.** Open, and atrium is not written. Three of the pieces
[#85](https://github.com/Lilium-Linux/solium/issues/85) lists have landed:
groups (`sol.group`, `sol.present_group`), stacking from a script (`z` in
`sol.present`), and an opacity in the transform. Paths, an input grab scoped to
part of the screen, and blur behind a window
([#78](https://github.com/Lilium-Linux/solium/issues/78)) have not.

### [E11](https://github.com/Lilium-Linux/solium/issues/169) — A shell hosted inside Solium is a complete daily desktop

A shell runs inside Solium as its configuration: its QML is hosted in the
compositor's own engine, beside the decorations, rather than run as a program
of its own. What that still needs was listed by loading a real shell, written
for Quickshell, in a nested session.

Done when a shell hosted this way is the whole desktop: on every monitor,
reserving its edges, taking the input its items ask for, and driving the
windows and workspaces it shows.

**Status.** Open. Milestone 1 of the native shell platform has landed: one
instance per monitor, each reading `Solium.monitor`
([#161](https://github.com/Lilium-Linux/solium/issues/161)); input only
where its items take it
([#173](https://github.com/Lilium-Linux/solium/issues/173)); every
button, the wheel, `Grab` popups and the keyboard on demand
([#163](https://github.com/Lilium-Linux/solium/issues/163)); edges
reserved whatever the scene's size
([#162](https://github.com/Lilium-Linux/solium/issues/162)); named
actions with data, sent with `Solium.send` and done by `sol.act`; and live
`Windows`, `Workspaces` and `Monitors` models
([#166](https://github.com/Lilium-Linux/solium/issues/166)), the
workspaces as `workspaces.lua` declares them with `sol.workspaces`.
Separately, a hosted shell's timers fire on an idle desktop
([#164](https://github.com/Lilium-Linux/solium/issues/164)), and a program
it starts, through `Solium.send`, Lua and `sol.spawn`, gets the environment Solium
started with ([#175](https://github.com/Lilium-Linux/solium/issues/175)).
The Quickshell compatibility layer was removed
([#172](https://github.com/Lilium-Linux/solium/issues/172)), and
[#165](https://github.com/Lilium-Linux/solium/issues/165), about the
programs that layer's `Process` started, was closed as superseded by it. The
epic still holds
[#167](https://github.com/Lilium-Linux/solium/issues/167), the services a
shell shows (audio, notifications, media, battery, network, Bluetooth, the
tray and the list of applications), which was written against the layer that
was removed; and, for later, the effects work: a hosted dock naming its icons
with `Solium.region` for the genie, and widgets that morph into the lock
screen.

---

## What is left to be a complete compositor

The epics above describe the *architecture*, and most of it holds: one engine,
one clock, modes as scripts, decorations in-process. E9 and E10 are the parts
of it still open. A compositor is complete when a person can use it all day
without meeting something it cannot do, and that is a different list —
protocols, session plumbing and missing keys, mostly unglamorous, and each one
invisible until the day it is missing.

Three tiers, and the order matters more than the contents. They are the argument
for the order, not a status board: the live list is the [`daily-drive`
label](https://github.com/Lilium-Linux/solium/issues?q=is%3Aissue%20is%3Aopen%20label%3Adaily-drive),
and a row here is an example of its tier. **[docs/gaps.md](gaps.md)** is the
exhaustive list — every protocol not implemented and every non-protocol gap,
whether or not it is worth doing soon.

### 1. Things a desktop cannot do without

These are not features. Each one is a day where somebody stops using the
compositor and does not come back.

| | why it stops someone |
|---|---|
| [#26](https://github.com/Lilium-Linux/solium/issues/26) IME | no CJK, no emoji picker, no on-screen keyboard. Unusable for most of the world's writers. `compose:ralt` gives a compose key meanwhile |
| [#55](https://github.com/Lilium-Linux/solium/issues/55) virtual-keyboard | the other half of an on-screen keyboard, which is what a phone is |
| [#56](https://github.com/Lilium-Linux/solium/issues/56) window rules | a script can read a window's `app_id`, but there is no rules table. The first thing anybody configures |
| [#63](https://github.com/Lilium-Linux/solium/issues/63) multi-GPU | a monitor on the second card cannot be driven at all — which is docking an ordinary laptop |
| [#50](https://github.com/Lilium-Linux/solium/issues/50) foreign-toplevel | a shell that runs as its own program cannot list windows or switch to them, so it cannot have a task switcher |
| [#51](https://github.com/Lilium-Linux/solium/issues/51) output-management | monitors are arranged by the configuration and moved on a reload; no client can move one, so E8's settings surface and `kanshi` cannot |
| [#52](https://github.com/Lilium-Linux/solium/issues/52) data-control | no clipboard manager can work |

The last three were found by writing this section, which is the argument for
writing it. The shell is the reason all three matter more here than elsewhere:
a dock that cannot enumerate windows is a launcher, and a settings panel that
cannot move a monitor is a text editor with buttons.

**The session.** Two gaps between the compositor and the rest of the user
session, each a day lost; [gaps.md](gaps.md#the-session) has the detail.

| | why it stops someone |
|---|---|
| [#153](https://github.com/Lilium-Linux/solium/issues/153) logind | `loginctl lock-session` does nothing, and nothing locks before sleep unless `swayidle -w` does |
| [#157](https://github.com/Lilium-Linux/solium/issues/157) libinput settings | no tap-to-click, so tapping a touchpad does nothing |

### 2. Things that have to be true, not built

Correctness and confidence rather than capability. Nothing here adds a feature
and all of it decides whether the thing is trustworthy.

| | |
|---|---|
| [#48](https://github.com/Lilium-Linux/solium/issues/48) the seat flake | three of forty-three logged sessions got no input devices and were stopped by the watchdog; a longer watchdog would not have saved one, and the cause is not known |
| [#65](https://github.com/Lilium-Linux/solium/issues/65) a hardware soak | the compositor has never run unattended for hours on a real session; `dev/soak.sh` can now do it on a TTY, though only its clients churn there, since the TTY backend ignores the scripted key presses |
| [#66](https://github.com/Lilium-Linux/solium/issues/66) packaging | `dev/install.sh` installs from a checkout and `dev/rpm.sh` builds a Fedora 44 package of one, but there are no packages in a repository, and a preview nobody can install is a preview nobody tries |
| [#64](https://github.com/Lilium-Linux/solium/issues/64) suspend and resume | never tested once. A laptop that cannot be closed and opened is not a laptop |
| [#83](https://github.com/Lilium-Linux/solium/issues/83) portals | screen sharing is reasoning, not evidence: nothing has been run end to end |

### 3. The part that is not parity

Everything above brings Solium level with a good tiling compositor. None of it
is why this project exists.

- **[E7](https://github.com/Lilium-Linux/solium/issues/7) — touch, gestures, form factors.** One binary usable by touch on a
  tablet and by pointer on a desktop. The animation engine already takes an
  initial velocity precisely so a gesture's throw can be handed to it; nothing
  yet hands it one. Touch already reaches applications, but not what the
  compositor draws
  ([#181](https://github.com/Lilium-Linux/solium/issues/181)), and
  gestures come after v0.1.0.
- **[E8](https://github.com/Lilium-Linux/solium/issues/8) — settings.** A surface built from what scripts declare rather than a
  fixed schema, so adding a mode adds its settings.
- **The dock, and the morph.** `sol.present_from` still does what
  `docs/shell-boundary.md` records: a window grows out of the rectangle an icon
  occupied. What is missing is a dock icon to aim at. For a dock hosted in the
  compositor that is a question inside one engine, not a protocol: the plan is
  for its QML to name the icon so an animation can follow it. A dock that runs
  as its own program would need a protocol to hand the rectangle over, and
  doing that without a side channel is still open.

### What "complete" would mean here

Not "has every protocol". A compositor is complete for this project when the
same binary runs a phone, a tablet and a desktop; when every mode is a script
somebody can rewrite; and when nothing in a normal day makes the user notice
they are running something unusual. The first is E7. The second is done. The
third is tier one.

**Honest position on evidence.** Most of tiers one and two is judged from nested
runs and from reading. Some has been confirmed on the hardware: hotplug, popups
and menus, and the drag icon; and, at `dfc95ce` on an NVIDIA RTX 3070 desktop
and a Surface Pro 7 (Intel Ice Lake), QML on the GPU with animations running,
the screens going off when idle, `swaylock`, and the Caps Lock and layout
pill. On the Surface Pro 7 the screen also went off when the lid closed,
though Solium has no lid handling of its own. The hardware backend brings up
more than one monitor, picks modes and CRTCs, and has been used with real
applications on a TTY — but no soak, no leak measurement and no screencopy
test has ever run on it. That is not a small caveat and it belongs in the plan
rather than in a footnote.

---

## Sequencing notes

- **E2 and E3 are the whole bet.** If overview cannot be a script, the
  architecture is wrong and it is cheap to find out early.
- **E5 comes after E3 deliberately.** Decorations were what the Hyprland fork
  attempted first, and they turned into a plumbing project that never reached
  the interesting work.
- **E7 is late but must not be an afterthought.** Design the input layer in E1
  so touch is a profile, not a retrofit.
