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

**Done:** steps 1, 2 and 3.

Step 1 is `crates/solium/src/pane.rs`: `Pane`, `PaneId`, `Content`, `Panes`.

Step 2 put panes underneath everything — `snapshot()`, `window_id`,
decorations (re-keyed by `PaneId`), drawing, every hit test. `sync_panes` is
the single place the pane list and the space are reconciled. `Space` did not
go away and is not going to: it is still the authority on where a mapped
client is, what damage it did, and which surface is under a point *within* a
window. What changed is that nothing above that level asks it what windows
exist.

Step 3 made a pane with no client a real window to the layout:

* Presentation state — the in-flight transform, and whether a window has ever
  been on screen — moved from `Window::user_data()` onto `Pane`. It had to:
  there was nowhere else to keep it for a pane with no window, and it has to
  survive adoption untouched or a window would snap the instant its
  application arrived.
* `pane_geometry` answers for a pane the space knows nothing about. A client's
  window asks the space; a pane without one answers from its own slot.
* `place` moves a pane whether or not there is a window inside it to resize,
  and `Present`, `PresentFrom`, `Clear` and `Close` all act on a pane.
* `begin_loading` opens a window for an application that has been asked for,
  and tells scripts it **opened**. That is the half that matters: a layout
  keeps its own arrangement and adds to it on an open event, so a relayout
  alone would never put the new window in the tree.
* `settle_loading` gives up on applications that never arrive, so a window
  cannot hold a slot forever.

Everything about it is in `config.lua` under `loading`: `scene`, `patience`,
`reserves_a_slot`. `reserves_a_slot` gates the open event rather than only
the snapshot, because a layout told a window opened keeps placing it however
the snapshot is filtered afterwards.

`SOLIUM_LOADING_AT="3000:firefox"` opens one on demand — the state cannot be
reached by using the compositor normally, because every real program connects
and connects fast. It is scaffolding for steps 4 and 5, and it goes when
`spawn` starts doing this for real.

**Next:** step 4, adoption. `new_toplevel` matches the connecting client's
process against loading panes before creating anything, and the pane takes
the window as its content — same id, same slot, same frame, same animation.

What is worth knowing before starting it:

* `Pane::awaits(&[pid])` and `Pane::adopt(window)` already exist, tested, and
  are two of the things the module-level `dead_code` expect is covering.
  `Solium::ancestry(pid)` on `main` is the process walk to match against.
* `begin_loading` takes a `pid` and nothing passes one yet. `spawn` is where
  it comes from, and `spawn` is also where `begin_loading` has to replace
  `begin_launch` — which is what makes the whole thing reachable without the
  dev hook.
* A missed adoption must open a window the old way, never lose one. Likewise
  a client whose pane was closed while it was still starting.
* `sync_panes` will create a *second* pane for an adopted window if adoption
  runs after it, so adoption has to happen in `new_toplevel`, where the
  window is mapped, and not later.

**Still true and easy to forget:** everything stays configurable. When step 5
draws the loading scene, what it shows and how it animates in belong to QML
and `config.lua`, not to constants in Rust.
