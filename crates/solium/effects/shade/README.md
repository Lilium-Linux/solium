# shade

An edge shade: a thin band just inside the part's rounded outline, round all
four sides and through the corners, darkest at the very edge and gone `width`
pixels in, every line of equal shade concentric with the outline. The design
and the code are Solium's own.

## How it runs

It reads nothing of the frame, only the part's shape (tier T0): no capture,
and one pass, run once and again only when the part's size or its params
change, whatever the window draws under it: on every frame of a window
changing size, unless `effects.motion_hz` caps how many times a second (its
last result stretched over the window between). Its result is drawn as it
was between those, so a window moving, fading or zooming draws the shade it
has.

## In the `glass` pane style

The shipped `glass` pane style puts it in front of every window it frames
that does not have the keyboard, but a fullscreen one, so a window you are
not typing in is told apart by a dark edge. The rule's `fade` brings it in as
the window loses the keyboard and takes it away as the window gets it back,
240 ms on `outCubic`: the compositor blends the result it has at another
opacity on each frame, and runs nothing for it.

```lua
{ match = { fullscreen = false, focused = false }, part = "pane", slot = "front",
  effect = "shade", fade = { duration = 240, easing = "outCubic" } }
```

A rule of yours for `{ style = "glass" }`, the `pane` part and its `front`
slot wins over it: `effect = false` there takes the shade away, and
`effect = { "shade", width = 6, strength = 0.7 }` gives it other params, or
another `fade`.

## Params

| Param | Default | What it does |
|---|---|---|
| `width` | `12` | how far in it reaches, in pixels, from 0 to 256; 0 draws none |
| `strength` | `0.5` | how opaque it is at the very edge, from 0 to 1 |
| `color` | `{ 0, 0, 0, 1 }` | its colour; its alpha scales `strength` |
| `falloff` | `2` | how it fades across the band, from 0.25 to 8: 1 evenly, 2 with the square of the way in, so the edge is dark and the inside soft |
| `radius` | `-1` | the corners, in pixels, from -1 to 512. -1 keeps the part's own: a window's whole `pane` has its style's `client.radius` at every corner |

## Changing it

Copy this folder to `~/.config/solium/effects/shade/`: your copy is used in
its place, everywhere that names `shade`. `solium --check
~/.config/solium/effects/shade` checks your copy without starting anything.
