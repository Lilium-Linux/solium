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

## Where this got to

**Done:** steps 1 and 2.

Step 1 is `crates/solium/src/pane.rs`: `Pane`, `PaneId`, `Content`, and
`loading_source` making the loading scene configurable the way decorations
are.

Step 2 put it underneath everything. `Panes` is a list of panes, bottom to
top, and it is now what the compositor means by "its windows":

* `snapshot()` — the list scripts place — is built from panes.
* `window_id` is the pane's id, not the surface's. That is the whole point:
  it can exist before the surface does, so a script told about a window
  while its application is starting is talking about the same window
  afterwards, because nothing was replaced.
* `Decorations` is keyed by `PaneId`, so a frame survives adoption with its
  animation still running rather than being rebuilt.
* Drawing (`on_screen`), hit-testing (`window_under`, `surface_under`,
  `frame_under`, `decorated_under`, `resize_target`) and the window counts
  all walk panes. `frame_under` and `decorated_under` answer *with* a pane.
* `sync_panes` is the single place the pane list and the space are
  reconciled — a client with no pane gets one, a pane whose client has gone
  is retired, and the order is brought back in line with the stacking
  `Space` keeps. It is called where the space is refreshed, once per frame.

`Space` did not go away and is not going to. It is still the authority on
where a mapped client is, what damage it did, and which surface is under a
point *within* a window. What changed is that nothing above that level asks
it what windows exist.

Two things fell out of the re-keying rather than being aimed at:

* A frame is dropped in `sync_panes`, with its pane. Keyed by surface that
  could not happen there, because nothing knew the set of live windows — so
  it was done where a window was seen leaving tidily, and a client that
  crashed left its frame behind. Same for a pending close.
* `sync_panes` reports whether the window set changed, and both backends
  redraw when it did. A window that vanished by any route is a different
  screen; without this the last frame it was in could sit there until
  something unrelated caused damage.

**Next:** step 3, layout and snapshot — a loading pane getting a real slot,
and the tiling arithmetic running on a count that includes it. Most of the
cost was paid in step 2: `snapshot()` already returns whatever the pane list
holds, so the work is letting a pane with no client through the `client()?`
filter in `snapshot`, `on_screen` and the hit tests, and giving it geometry
of its own instead of asking the space.

What is worth knowing before starting it:

* A pane's `slot` is authoritative only for a pane with no client. For a
  mapped one, `sync_panes` overwrites it from the space every frame, because
  the space is the authority there. A loading pane's slot has to be written
  by the layout and left alone — so the two assignments cannot become one.
* Panes with no client currently sort to the top of the list, which is where
  a window just asked for belongs. If step 5 ever wants a loading pane to
  hold a position among the others, `Panes::sync` is the one place that
  decides it.
* `window_under` and friends return a `Window`. Every one of them will need
  to answer for a pane that has none; `frame_under` and `decorated_under`
  already answer with the pane, and are the shape the rest should take.

**Still true and easy to forget:** everything above has to stay
configurable. The loading scene already is. When a loading pane gets a slot,
how long it waits and whether it takes a slot at all belong in `config.lua`,
not in constants.
