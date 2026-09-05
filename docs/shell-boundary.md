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

Ordinary applications, over `xdg-shell`. And `wlr-layer-shell` stays
implemented so that *foreign* panels and wallpapers can attach if someone wants
them — but Lilium's own shell does not use it, and the compositor's work area
is computed from whatever the hosted shell reserves as readily as from a layer
surface.

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
| Bar, dock, launcher | The same QML engine | directly |
| Colours and metrics | `Solium.Theme`, one singleton | imported by every scene |
| What an animation *does* | Lua script | `sol.present_from`, `sol.on("open")` |
| Foreign panels, wallpapers | Clients | `wlr-layer-shell` |


## What arrived with the dock

The first piece of shell drawn by the compositor, and the reason the boundary
sits where it does.

`shell.rs` holds a dock: a QML scene rendered by the same host that draws the
window frames, importing the same `Solium.Theme`. `sol.dock` says what it
holds, because which programs belong on a dock is not the compositor's
opinion, and it reserves its own strip out of `work_area` the way a
layer-shell client would with an exclusive zone.

The morph is the whole argument made concrete. Pressing an icon fires a `dock`
event carrying **the rectangle that icon occupies**; a script spawns the
program and hands that rectangle to `sol.present_from`; the window grows out
of it. Captured frame by frame in `docs/morph.png`: 460x208 near the dock,
then 928x504, 1192x672, and settled at full size about a third of a second
later.

None of that is reachable from a separate process. A client dock can pass a
rectangle over IPC, but by the time the window exists the two are separate
scenes and nothing holds both at once to interpolate between them. That is why
Quickshell was the wrong tool here, and it is worth saying plainly: the shell
being a client is what makes an icon and a window unable to be the same thing.
