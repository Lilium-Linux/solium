# Where Solium ends and Lilium begins

Solium is the compositor. Lilium is the desktop — its bar, its dock, its
launcher. This is the line between them, and the line is not where a Wayland
tutorial would put it.

## The requirement that decides the architecture

> An object should be able to move from the dock into a window's titlebar.

Not "look similar in both". *Move* — one object, travelling, unbroken.

That single requirement rules out the conventional answer. If the shell is a
separate process painting its own pixels, then a dock icon and a titlebar are
two scene graphs in two processes with two stylesheets, and an object cannot
cross between them; the best available is a fake — dissolve one, appear in the
other — which is exactly the kind of thing this project exists not to build.

The same requirement, in a weaker form, rules it out again: "the shell changes
colour, so the decorations change too" is trivial when there is one theme
object, and a synchronisation problem forever when there are two.

## So: one engine

The compositor hosts **one QML engine**. Window decorations are scenes in it.
The shell's surfaces — bar, dock, launcher — are scenes in it. They import the
same `Solium.Theme` singleton, because there is only one of it.

That gives, in order of how hard they would otherwise be:

- **One design system.** `qml/Solium/Theme.qml` is read by every surface the
  desktop draws. Changing a colour there changes the titlebars and the dock
  together, with no rebuild, because they are the same object and not two
  copies.
- **Objects that travel.** An item lifted from the dock into a titlebar is a
  reparent inside one scene graph. It keeps its colours because it never left
  the design system, and it can be animated across because both ends are on the
  compositor's clock.
- **No protocol for shell geometry.** The dock's icon rectangles are not
  published to the compositor; the compositor *has* them. The genie animation
  reads a rectangle out of the same engine that drew the icon, so it cannot be
  animating against a stale copy — the failure mode a side-channel protocol
  would have made possible.

This is the arrangement Apple has, and it is unavailable to anyone configuring
an existing compositor. It is the reason for writing one.

## What is still a client

Ordinary applications, over `xdg-shell`.

**And, today, the bar and the dock too** — over `wlr-layer-shell`. The
in-compositor dock described below existed, proved the morph, and was then
taken back out: it made the compositor own a design, a font stack and a layout
it had no reason to, and it made the interesting question — how does a
*replaceable* shell attach? — disappear rather than get answered. `layer.rs`
records that in full.

So the boundary as it stands is: window-coupled things are scenes in the
compositor's engine, and everything that merely sits on a screen is a client
that anchors itself. The one-engine argument above is not withdrawn — it is
what the titlebars are built on, and it is what a dock would come back inside
for if the morph is ever wanted for real. It is a claim about where the line
*can* be drawn, and drawing it there is a later decision than this file
originally assumed.

### Where a dock goes, with more than one monitor

A layer surface names the output it wants, and the compositor honours it. That
is how a shell puts a bar on every screen: one surface per output, each with
its own exclusive zone, each reserving from *that* monitor's work area.

A surface that names no output gets the **primary** monitor — the one
`primary = true` picks out in `monitors`, or the first if nothing does. Not the
monitor the pointer is on, which is what it briefly was: a dock connects once,
at startup, and pinning it to whichever screen the mouse happened to be over
then means it appears somewhere different depending on where the mouse was
left. That looks like the compositor placing it at random, because it is.

`sol.monitors()` gives a script the list, with `primary` and `focused` flags,
so a shell written in Lua can decide for itself which screens get a bar.

`SOLIUM_SHELL_SCENE` still hosts one QML scene in-process, on the primary
monitor. It is a development affordance for exercising the QML host, not the
shell — a shell that wants a bar per screen writes layer surfaces.

## What this costs, honestly

The shell cannot crash independently of the compositor. A separate process can
be restarted; a QML error in the dock takes the session with it unless the
compositor is careful. So:

- Every scene is loaded defensively. A scene that fails to load is skipped and
  logged; it never stops the compositor starting.
- Scene errors are contained per surface — a broken dock must not take the
  window frames with it.

That is the trade being made deliberately: robustness through care inside one
process, in exchange for a desktop that can actually do what the design asks
for.

## Summary

| | Where it runs | How it reaches the compositor |
|---|---|---|
| Applications | Clients | `xdg-shell` |
| Window frames | Compositor's QML engine | directly |
| Loading windows, the pointer | The same QML engine | directly |
| Bar, dock, launcher, wallpaper | Clients | `wlr-layer-shell` |
| Which monitor a bar is on | The client names an output | `zwlr_layer_surface_v1` |
| Colours and metrics | `Solium.Theme`, one singleton | imported by every scene |
| What an animation *does* | Lua script | `sol.present_from`, `sol.on("open")` |


## What the dock proved, before it was removed

Kept because it is the evidence for the architecture, not because the code is
still there — `shell.rs` and `sol.dock` are gone, and a dock is a layer-shell
client now.

It was a QML scene rendered by the same host that draws the window frames,
importing the same `Solium.Theme`, reserving its own strip out of `work_area`.
The morph was the whole argument made concrete: pressing an icon fired a `dock`
event carrying **the rectangle that icon occupies**, a script spawned the
program and handed that rectangle to `sol.present_from`, and the window grew
out of it. Captured frame by frame in `docs/morph.png` — 460x208 near the dock,
then 928x504, 1192x672, settled at full size about a third of a second later.

`sol.present_from` is still there and still does that. What is missing is a
dock to give it a rectangle, and `config.lua` still carries `dock.morph` for
the day one exists. A layer-shell dock could hand over an icon rect through a
protocol, and that is exactly the side-channel the one-engine argument says
will go stale — so if the morph is wanted for real, this is the decision to
reopen rather than route around.

None of that is reachable from a separate process. A client dock can pass a
rectangle over IPC, but by the time the window exists the two are separate
scenes and nothing holds both at once to interpolate between them. That is why
Quickshell was the wrong tool here, and it is worth saying plainly: the shell
being a client is what makes an icon and a window unable to be the same thing.
