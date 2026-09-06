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

**Done:** steps 1 through 4. The behaviour the design was for now works; what
is left is what it looks like.

* **1** — `pane.rs`: `Pane`, `PaneId`, `Content`, `Panes`.
* **2** — panes underneath everything: `snapshot()`, `window_id`, decorations
  (keyed by `PaneId`), drawing, every hit test. `sync_panes` is the single
  place the pane list and the space are reconciled. `Space` stays as the
  authority on where a mapped client is and what damage it did.
* **3** — a pane with no client is a real window to the layout. Presentation
  state moved onto `Pane`; `place` moves a pane with or without a window
  inside it; `begin_loading` tells scripts the window **opened**, which is
  what puts it in a layout's tree. `config.lua` gained `loading`.
* **4** — adoption. A client whose process a window is waiting for becomes
  that window's content, in `new_toplevel`, where it is mapped. The old
  stand-in (`Launch` and everything around it) is gone.

**Next:** step 5, the loading pane's own content — the QML scene, drawn as the
pane rather than beside it, and a frame so it can be closed while it waits.

What is worth knowing before starting it:

* `qml/loading/window.qml` is unwired but intact, and `pane::loading_source`
  already resolves which scene by name or path with the user's directory
  shadowing the shipped one. `Content::Loading` carries the resolved `source`.
* `surface::ShellSurface` is how QML is drawn in-process; the old stand-in
  used it and `set_int("waited", …)` is kept alive by an `expect` for exactly
  this. A `ShellSurface` will want to live in `Content::Loading` beside the
  path.
* `on_screen()` yields `(PaneId, Window)` and skips a pane with no client.
  Widening it is the change: `render::elements` is where a loading pane draws.
* The hit tests skip a client-less pane too. `surface_under` skipping one is
  wrong once it is visible — a click would fall through to a window behind it.
* A loading pane has no frame, because `decorate` runs when a *client*
  negotiates. Giving one a frame is what makes it closable while it waits, and
  it is the payoff of keying decorations by pane in step 2: the frame it gets
  is the frame it keeps after adoption, with its animation intact. Watch for
  the flicker case — a client that then chooses client-side decorations, whose
  frame has to go.
* Closing a pane mid-load already works in the code (`close_pane` and
  `settle_closing` both handle a pane with no client) but could not be tested,
  because nothing can reach a loading pane to close it. A frame with a close
  button is what makes that testable.

**Still true and easy to forget:** what the loading scene shows and how it
animates in belong to QML and `config.lua`, not to constants in Rust.
