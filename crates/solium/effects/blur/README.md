# blur

A dual Kawase blur: `passes` down passes, each drawing at half the size of
the last and sampling five taps at `offset` texels, then as many up passes
back to the size it started from, each sampling eight. Halving the picture
is what makes it cheap: a wide blur costs a few small passes, not one large
kernel. The algorithm is Marius Bjørge's, from "Bandwidth-Efficient
Rendering" (SIGGRAPH 2015). These files are Solium's own, written for it, and
carry no third-party code.

| Param | Default | What it does |
|---|---|---|
| `passes` | `3` | how many halvings, and as many doublings back: a whole number from 1 to 6. Each one more roughly doubles how far the blur spreads |
| `offset` | `3` | how far apart each pass's taps are, in texels of what it reads: 0 or more. A larger one spreads further at the same cost |

It reads `ceil(offset · 2^(passes + 1))` pixels beyond the part it blurs, 48
at its defaults, so an edge is blurred from what lies past it rather than
from nothing. Its `fallback` lists two cheaper versions, `{ passes = 2 }`
and then `{ passes = 1 }`, for when a frame runs late; falling back to them
on a late frame arrives with X2.7.

Its input is the `backdrop`, what lies behind the part. Reading that needs
xray, which arrives with X2.1, and until then a rule blurring the backdrop
is refused, by name. Give `source = "self"` to blur the part's own pixels
instead:

```lua
{ match = { app_id = "mpv" }, part = "client", slot = "replace",
  effect = { "blur", source = "self", passes = 3 } }
```

To change it, copy this folder to `~/.config/solium/effects/blur/`: your
copy is used in its place, everywhere that names `blur`. `solium --check
~/.config/solium/effects/blur` checks your copy without starting anything.
