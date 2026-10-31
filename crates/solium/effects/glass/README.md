# glass

Liquid glass: a slab of frosted glass over the part. What lies behind the
part, the backdrop, is blurred and veiled with a tint, dark by default: a
black veil at 0.3, and `tint = { 1, 1, 1, 0.11 }` for light glass. Across the
rim, a band `bezel` pixels wide inside the part's rounded outline, the glass
bends what is behind it: it magnifies toward the centre and pulls in from the
edge, most at the very edge, so the backdrop curves round the edge, and with a
strong lens mirrors there. A thin highlight runs along the edges facing the
light, and a fainter one along the far edges with a soft shade under it. The
design and the code are Solium's own.

A rule puts it behind a part. Behind a translucent window it frosts and bends
what lies behind it:

```lua
{ match = { app_id = "kitty" }, part = "client", slot = "behind", effect = "glass" }
```

## How it runs

It reads the backdrop, from xray (tier T2), and draws itself to the part's
rounded outline, from the part's own box and corners. Its stages
save the backdrop as `sharp`, run the shipped [`blur`](../blur/README.md) on
it (`soften_passes` halvings at `soften_offset`) and save that as `soft`,
then draw `refract.frag` over the two, once per part. The blur reads only
the backdrop, so it is run once per monitor and shared by every part on it
([the memo](../README.md#the-memo)); each window runs only its own refract
pass, and again only when it moves, its params change or what is behind it
is drawn again. With `soften_passes = 0` there is no blur at all: clear
glass, one pass.

It reads `ceil(soften_offset · 2^(soften_passes + 1)) + 1` pixels beyond the
part, 49 at its defaults and 1 with no blur, so the frost at the edge is made
from what lies past it.

## Params

By family, as `effect.lua` lists them. A value outside a param's range is
clamped, with a warning; a param it does not have is refused, with the one
you probably meant.

### Shape

| Param | Default | What it does |
|---|---|---|
| `radius` | `-1` | the corners, in pixels, from -1 to 512. -1 keeps the part's own: a window's whole `pane` has its style's `client.radius` at every corner |
| `bezel` | `28` | the rim's width, in pixels, from 1 to 256; at most half the part's thickness is used |

### Refraction

| Param | Default | What it does |
|---|---|---|
| `zoom` | `0.15` | how much the rim magnifies toward the centre, from 0 to 0.9: the share of the way to the centre the very edge reads from (the lens) |
| `bend` | `8` | how far the rim pulls straight in from the edge, in pixels, from 0 to 256; at most half the part's thickness is used |
| `falloff` | `4` | how fast the pull fades across the rim, from 0.5 to 16: a larger one keeps it nearer the edge |
| `dispersion` | `0` | the colours pulled apart at the rim, a prism, from 0 to 0.3; 0 for none |

### Blur & grain

| Param | Default | What it does |
|---|---|---|
| `soften_passes` | `3` | the blur's halvings, a whole number from 0 to 5; 0 for clear glass |
| `soften_offset` | `3` | how far apart the blur's taps are, in texels, from 0 to 8 |
| `haze` | `0.85` | how much of the blur is mixed in across the whole glass, from 0 to 1; 1 is fully frosted |
| `rim_haze` | `0.35` | the frost at the very edge, from 0 to 1, fading to `haze` across the rim; a lower one lets the bend show sharp |
| `grain` | `0` | a fine grain fixed to the glass, from 0 to 0.25 |

### Glow

| Param | Default | What it does |
|---|---|---|
| `light_angle` | `270` | where the light comes from, in degrees, from 0 to 360: 0 the right, 90 above, 270 below |
| `shine` | `0.55` | the highlight along the edges facing the light, from 0 to 2 |
| `shine_back` | `0.5` | the highlight along the edges facing away, from 0 to 2 |
| `shine_color` | `{ 1, 1, 1, 1 }` | the highlights' colour, its alpha their strength |
| `glint_band` | `3` | the highlight's width, in pixels, from 0 to 256; 0 is the bezel's |
| `shade` | `0.5` | how dark the band under the far edges is, from 0 to 1 |
| `shade_band` | `14` | that band's width, in pixels, from 0 to 256; 0 is the bezel's |
| `edge_line` | `0` | a fine bright line along every edge, from 0 to 1 |
| `brightness` | `1` | the whole glass brighter or darker, from 0.5 to 2 |
| `tint` | `{ 0, 0, 0, 0.3 }` | a veil over it: a colour, its alpha how much of it. Black at 0.3 is dark glass, the default; `{ 1, 1, 1, 0.11 }` is light glass |

## Changing it

Give params where a rule names it:

```lua
{ match = { app_id = "kitty" }, part = "client", slot = "behind",
  effect = { "glass", zoom = 0.25, bend = 10, haze = 0.55, rim_haze = 0.15 } }
```

Light glass in place of dark is one param, `tint = { 1, 1, 1, 0.11 }`.

To change the effect itself, copy this folder to
`~/.config/solium/effects/glass/`: your copy is used in its place,
everywhere that names `glass`. `solium --check
~/.config/solium/effects/glass` checks your copy without starting anything.

## In the `glass` pane style

The shipped `glass` pane style puts it behind the whole of every window it
frames but a fullscreen one, the window and its bar together, so each window
is one piece of glass with one rim round its outline:

```lua
{ match = { fullscreen = false }, part = "pane", slot = "behind", effect = "glass" }
```

It shows where the window is translucent: through the bar, which draws no
fill, and behind a terminal whose background is clear. An opaque application
covers it, except in its bar. Your rules win over a pane style's for the same
part and slot: a rule of yours for `{ style = "glass" }`, `part = "pane"` and
`slot = "behind"` gives those windows other params, light glass with
`tint = { 1, 1, 1, 0.11 }`, and `effect = false` there takes the glass away.
The style's other rule puts the [`shade`](../shade/README.md) in front of a
window without the keyboard.

While a window glides, is dragged or changes size, its refraction runs again
on every frame it moved on, so what shows through the glass is where the
window is on every frame. `effects.motion_hz` caps how many times a second
it runs (0, no cap, unless you change it), for a GPU too weak for that: the
last result is then drawn where the window is between, and what shows
through trails the window by up to that long's worth of its motion.
