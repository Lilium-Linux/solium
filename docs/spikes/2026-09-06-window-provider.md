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

**Done. All five steps.** A window's life begins when the user asks for the
application. It takes its place in the layout immediately, draws itself while
it waits, can be closed before anything has connected, and the application
appears *inside* it — same window, same id, same frame with the same
animation still running in it.

* **1** — `pane.rs`: `Pane`, `PaneId`, `Content`, `Panes`.
* **2** — panes underneath everything: `snapshot()`, `window_id`, decorations
  (keyed by `PaneId`), drawing, every hit test. `sync_panes` is the single
  place the pane list and the space are reconciled.
* **3** — a pane with no client is a real window to the layout. Presentation
  state moved onto `Pane`; `begin_loading` tells scripts the window
  **opened**, which is what puts it in a layout's tree.
* **4** — adoption, in `new_toplevel`, where the client is mapped. The old
  two-object stand-in (`Launch` and everything around it) is gone.
* **5** — the pane draws its own QML, inside its own frame, and answers the
  pointer.

`Space` did not go away and is not going to. It is still the authority on
where a mapped client is, what damage it did, and which surface is under a
point *within* a window. Nothing above that level asks it what windows exist.

Configurable, in `config.lua` under `loading`: `scene`, `patience`,
`reserves_a_slot`, `decorated`.

### What is left, and where it is written down

* **#35** — an application that forks and exits is not adopted: the ancestry
  walk cannot follow a chain through init. It degrades correctly (an ordinary
  window, nothing lost) but the feature does not reach a whole class of
  programs. The fix is an activation token, which is #25's mechanism, not a
  private one.
* **#34** — closing a window leaves keyboard focus nowhere. Pre-existing,
  found while verifying step 2.
* `window_under` still answers only for a pane with a client, so clicking the
  body of a loading window does not raise or focus it. `surface_under` stops
  there, so the click reaches nothing behind it; it simply does nothing. A
  loading window cannot hold keyboard focus anyway — that is the last of the
  four risks this document opened with, and the answer it proposed.
* `SOLIUM_LOADING_AT` is scaffolding from step 3 and outlived its purpose now
  that `spawn` does this for real. It is still the only way to reach a window
  whose application will *never* arrive, which is worth keeping for testing
  patience and closing mid-wait.

### The risks this document opened with, as they turned out

* *Adoption misses* — happens, exactly as predicted, and opens an ordinary
  window. Now #35.
* *The application never arrives* — `settle_loading`, and `patience` is a
  setting. Verified by cutting it to 60ms.
* *Closing mid-load* — works; the pane goes, its frame goes with it, the
  layout heals. Finding this needed a frame on the loading window, which is
  why step 5 gave it one.
* *Focus with no surface* — a loading pane never takes keyboard focus, and
  keys reach bindings only. As proposed, unchanged.
