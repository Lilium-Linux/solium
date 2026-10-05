# Effect folders

An effect is a folder of Lua and GLSL: `effects/<name>/effect.lua`, which says
what the effect takes and how it runs, and the shader files beside it that draw
it. No effect is written into the compositor. Solium gives every effect the
same generic pieces to build from, and the effects that ship with it are
folders like any other, here, to read, copy and change. Yours go in
`~/.config/solium/effects/<name>/` (`$XDG_CONFIG_HOME/solium/effects/` when
that is set), and a folder of yours shadows the shipped one of the same name,
name by name, as a pane style's does: copy a shipped folder there, change it,
and everything that names it uses your copy. A folder with no `effect.lua` in
it is skipped, so an empty one made and not filled yet does not cost you the
shipped effect of that name.

A name is lower-case letters, digits, `-` and `_`: `blur`, `soft-glow`,
`fade_2`. It is a name and never a path, because an effect is named in rules
and by other effects, where a path would mean nothing: `Blur`, `my/blur` and
`../blur` name no effect.

An install puts the shipped folders in `share/solium/effects/`, beside the
shipped QML and Lua: `/usr/share/solium/effects/` for the Fedora package,
`~/.local/share/solium/effects/` for `dev/install.sh`.

## Nothing changes until you name one

A folder on disk changes nothing by being there. Solium loads only the effects
your configuration names, in a rule, in `effects.on` or in a binding, and the
effects those name in turn. Every other folder is left alone, yours and the
shipped ones alike.

## What `effect.lua` returns

`effect.lua` returns one table. Every key in it is checked, and a key Solium
does not know is refused by name, with the key you probably meant, so a typo
never quietly means a default.

| Key | What it is |
|---|---|
| `api` | `1`, the version of this format |
| `inputs` | what the effect reads: `self`, `backdrop`, `shape`, `old`, or `state:<name>` of its own |
| `params` | its knobs, below |
| `frag` | a one-pass effect's shader, a file in the folder |
| `stages` | a many-pass effect's passes: a list, or a function of the params |
| `pixels` | another effect's name, whose `frag` draws this one's pixels |
| `mesh` | a geometry effect's `function(t, cols, rows, out)` |
| `grid` | a geometry effect's grid: `{ along = 48, across = 8 }`, which turns with the direction the window moves in, or a fixed `{ cols, rows }` |
| `reach` | how far beyond its part the effect reads, in pixels: a number, or a function of the params |
| `bleed` | how far beyond its part the effect draws, likewise |
| `fallback` | cheaper versions to fall back to, in order: each a table of params, or another effect's name |
| `duration` | how long it plays, in milliseconds |
| `easing` | its curve, by name: `outBack`, `inOutQuad` and the rest of Solium's curves |

An effect draws something, and says how once: `frag` or `stages`, not both,
or a `mesh`, or `pixels`. `motion` is refused until motion tokens exist; give
`duration` and `easing`. `image:` inputs are not read yet, and are refused.

### Params

```lua
params = { passes = { 3, min = 1, max = 6, int = true }, offset = { 3 } },
```

A param is `{ default, min =, max =, int = }`, and its default's kind is the
param's: a number (a whole one with `int = true`), four numbers, a boolean,
or a word. Its name is lower-case letters, digits and `_`, and it may not be
one of the words said beside params where an effect is used: `source`,
`mask`, `keep`, `effect`, `geometry`, `pixels`, `decoration`, `duration`,
`easing`, `motion`, `from`, `to`, `axis`, `reach` and `bleed`.

Where the effect is used its params can be given, as in
`{ "blur", passes = 2 }`. A param the effect does not have is refused, with
the one you probably meant; a value of another kind is refused; a whole
number is a fine value for a fractional param; and a value outside `min` and
`max` is clamped, with a warning. A function of the params, such as `reach`,
is called with the params as they were bound.

## What an effect's Lua can reach

Each effect runs in a Lua of its own, apart from your configuration and from
every other effect, with `math`, `table` and `string` and the base functions
that reach nothing outside it. There is no `sol`, `io`, `os`, `require`,
`load`, `loadfile` or `dofile`, no `pcall` or `xpcall`, no `collectgarbage`
and no `string.dump`. `print` writes a debug line to Solium's log, naming the
effect. A metatable may not have a `__gc` finaliser, because a finaliser runs
where nothing can stop it.

Every call into an effect, running `effect.lua` or a function of its params,
has 100 ms, and its Lua 16 MiB. One that runs longer, or holds more, is
stopped; an effect stopped by the clock runs nothing more until it is loaded
again. A Lua error is reported at its file and line.

## Writing a `.frag`

A shader file is GLSL ES 1.00 (`#version 100`), the language every program in
Solium is written in, the warp's included. It defines one function, which is
called for every pixel the pass draws:

```glsl
vec4 sol_effect(vec2 uv) {
    vec4 c = sol_tex(uv);
    return vec4(mix(c.rgb, vec3(c.a), p_amount), c.a);
}
```

`uv` runs from 0 to 1 across the box the pass draws, (0, 0) at its top-left.
Your file is compiled as it is, between a prelude Solium writes for the pass
and a few lines that call `sol_effect`, and it stays a source string of its
own, so the line numbers a compiler gives are your file's own lines. Do not
write `#version`, a `precision` or a `uniform` named `p_…` or `sol_…`: the
prelude has them, and a `uniform` of yours with one of those names is an
error at its line.

| Name | What it is |
|---|---|
| `sol_tex(uv)` | the pass's input: the previous pass's result, or the effect's first input in the first pass, clamped at its edge |
| `sol_texel`, `sol_size` | one texel of that input, in `uv`; the size of what the pass draws, in pixels |
| `p_<param>` | each of the effect's params, declared from `params`: a number is a `float` (an `int` with `int = true`), a boolean an `int` 0 or 1, four numbers a `vec4`; a word has none |
| `sol_<name>(uv)`, `SOL_HAS_<name>` | another texture the effect reads, by name: `sol_self`, `sol_backdrop`, or `sol_<n>` for `state:<n>`. One the pass does not list in its `uses` reads transparent and its `SOL_HAS_<name>` is 0, so one file can be written for both cases |
| `sol_shape(uv)`, `sol_sdf(px)` | the part's shape: its coverage, and its signed distance in pixels, negative inside, exact for a rounded box |
| `sol_content`, `sol_to_content(uv)`, `sol_to_uv(px)`, `sol_sdf_rrect(px, size, radii)` | the part's rectangle inside the box the pass draws, the maps between `uv` and the part's pixels, and the rounded-box distance itself |
| `sol_progress`, `sol_clamped`, `sol_direction`, `sol_seed`, `sol_time` | in a transition: its progress, which a spring may carry past 1; the same clamped to 0..1; +1 arriving, −1 leaving, 0 resizing; a random number fixed for the transition; seconds |
| `sol_noise(p)` | value noise |

A `p_` name the effect has no param for, and a `sol_` name that is none of
these, are errors at their line, with the param you probably meant; reading a
texture the pass does not list in its `uses` is a warning, since it reads
transparent. Comments are skipped.

## The shipped folders

None yet.
