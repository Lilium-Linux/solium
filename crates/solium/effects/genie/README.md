# genie

The window pulled into its target like a sheet through a letterbox: the edge
nearest the target goes first and the rest follows, so the sheet bends on its
way in rather than shrinking. Played backwards, the same file draws a window
out of its target. It is a geometry effect, a `mesh` and nothing else: it
moves the window's picture and leaves its pixels as they are.

`super+m` plays it, and so does the Developer tweaks panel's genie, through
`sol.present`: `deform = { effect = "genie", axis = "down", spread = 1.4,
to = … }`.

| Param | Default | What it does |
|---|---|---|
| `spread` | `1.4` | how much of the window is in motion at once: 0 or more. At 0 the window is pulled in rigidly; a larger one starts the far edge later, drawing the tail out behind the lead |

It plays for 520 ms on `inOutCubic`, over a grid of 48 steps along the way
the window is pulled and 8 across it, which turns with that way: 48 rows for
a window pulled down or up, 48 columns for one pulled sideways.

## Its axis

The axis is not a param: where the genie is played says which way the window
is pulled, and the file reads it as `t.axis`. `down` leads with the bottom
edge (a dock along the bottom), `up` with the top, `left` and `right` with
those sides. `auto` picks one of the four once, when the window starts to
move, from the side of the window its target's centre lies on: the farther
of the two, each measured against the window's own half-width or
half-height, the vertical one on a tie, and `down` when the target's centre
is inside the window.

## Held to the Rust genie

Solium's first genie was written in Rust (`Deform::Genie`, in
`crates/effects`), and this folder is held to it: Solium's tests compare
every point of this file's grid with the Rust genie's, on all four axes, at
several spreads and progresses, past the window's edges too, and they must
agree within 10⁻⁹ of a pixel. The effects crate's preview page
(`crates/effects/preview/`, built by `dev/preview`) draws that Rust genie,
the oracle, so the shape it shows is the shape this file draws.

To change it, copy this folder to `~/.config/solium/effects/genie/`: your
copy is used in its place wherever an effect named `genie` is played.
`solium --check ~/.config/solium/effects/genie` checks your copy without
starting anything.
