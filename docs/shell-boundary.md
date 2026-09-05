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
