# The window provider

**Date:** 2026-09-06
**Status:** design, before any code moves. Branch: `window-provider`.

## Why

A window's life should begin when the user asks for the application, not when
the application's client happens to connect. Everything that follows from that
— the slot being reserved immediately, the other windows moving aside, being
able to close it while it is still loading, the application's content simply
appearing inside it — is impossible while the compositor's idea of a window
*is* `smithay::desktop::Window`, because that type requires a surface to exist.

The stand-in shipped on `main` is two objects handing over to each other. It
reads correctly for one frame and is wrong underneath: two ids, two slots, a
handover animation papering over the seam. No amount of polish fixes that,
because the seam is the architecture.

## What changes

The compositor stops working with `desktop::Window` directly. It works with a
**pane**: our own window provider, which owns an identity and a slot, and whose
*content* is one of

  * **loading** — a QML scene the compositor draws itself,
  * **client** — a mapped `desktop::Window`,
  * **leaving** — a client that has gone, drawn from what was last known of it
    while it animates out.

A pane keeps its id and its slot across all three. Adoption is the whole point:
when a client whose process matches a loading pane maps a toplevel, the
toplevel becomes that pane's content. Nothing is created, nothing is replaced,
and nothing else in the compositor notices.

QML is not a special case bolted on. A pane's content being a scene is as
ordinary as its content being a client, which is what makes a loading window, a
placeholder for a crashed application, and a purely compositor-drawn surface
the same mechanism rather than three.

## What has to move

Every path that reaches for `space.elements()` today:

| path | today | after |
|---|---|---|
| layout | scripts see mapped windows | scripts see panes, loading included |
| focus | keyboard focus needs a surface | a loading pane can hold focus and receive a close |
| render | window elements, then chrome | pane content, whatever it is |
| input | `surface_under` | pane under, then its content |
| decorations | keyed by surface id | keyed by pane id, so a frame survives adoption |

The last row is the one that makes the seam disappear: a frame drawn around a
loading pane is the *same frame*, with the same animation state, once the
client arrives.

## Order

1. **The type, alone.** `Pane`, its content enum, ids, geometry. Tested, not
   yet wired to anything.
2. **The space becomes panes.** One list of panes replaces `Space<Window>` as
   the compositor's own view; `Space` stays underneath for the mapped case,
   because its damage and output bookkeeping is not worth rewriting.
3. **Layout and snapshot.** Scripts see panes. A loading pane gets a slot, and
   the tiling arithmetic runs on the count that includes it.
4. **Adoption.** `new_toplevel` matches the client's process against loading
   panes before creating anything.
5. **Loading content and close.** The QML scene, and closing a pane whose
   application never arrived.

Each step ends with the compositor working. Step 2 is the one that touches
everything; steps 3 to 5 are where the behaviour people asked for appears.

## Risks worth naming now

* **Adoption misses.** A client that re-execs or forks past the ancestor walk
  maps a window with no matching pane. It must then open normally — a missed
  adoption is a window that appears the old way, never a window that is lost.
* **The application never arrives.** A loading pane that waits forever holds a
  slot forever. It gives up, and the layout is told, exactly as if it closed.
* **Closing mid-load.** The process is spawned and may connect *after* the pane
  is gone. The adoption path has to cope with a client whose pane no longer
  exists, again by opening normally.
* **Focus with no surface.** Keyboard focus is a Wayland concept and a loading
  pane has nothing to give it to. The pane holds focus in our model and hands
  it to the surface at adoption; until then keys reach bindings and nothing
  else, which is what a loading window should do anyway.
