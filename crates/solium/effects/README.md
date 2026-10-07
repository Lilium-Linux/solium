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

## Rules

A rule puts an effect in a slot of a part of a window, or of a surface. Rules
go in your configuration, in `effects.rules` (in `user.lua`, a list that
replaces the shipped empty one whole), and `init.lua` hands them over with
`sol.effects`:

```lua
return {
    effects = {
        rules = {
            { match = { app_id = "mpv" }, part = "client", slot = "replace",
              effect = { "blur", source = "self", passes = 2 } },
        },
    },
}
```

| Key | What it takes |
|---|---|
| `match` | `"*"`, or a table of what to match: `app_id`, `title`, `monitor`, `style` (a bare window's is `"none"`), and `focused` and `fullscreen` as `true` or `false`, for a window's parts; `surface` for a scripted surface's; `layer_shell` for a layer surface's. A word is exact, `"*"`, or a prefix ending in `*` (`"org.gnome.*"`); there are no Lua patterns. A key of another kind of part's is refused. `focused` is whether the window is drawn focused, `fullscreen` whether Solium last told it to be, and `sol.windows()` gives every one of a window's keys, so you can see what a rule will match |
| `part` | `pane` (the window's frame and client, not its popups), `client`, `popup` (all of a window's popups at once), `layer:<name>` (a pane style's layer), `region:titlebar` (the band along the one side a pane style reserves most for: the window's full width above or below the client, its two outer corners rounded as the client's largest, or square down a side between those; a style that reserves its most on two sides or more, such as `border`, or nothing, and a bare window have none), `surface:<name>` (one scripted surface on one monitor) or `layer_shell:<namespace>` |
| `slot` | `behind` the part, in `front` of it, or `replace` it. A `pane`'s go around its frame and client and under its popups, so a menu stays on top; a `client`'s between the pane style's layers and the client; a `popup`'s around the popups; `region:titlebar`'s around the pane style's layer named `bar` (else its first `frame` layer) and a `layer:<name>`'s around that layer; a scripted or layer surface's just under and over it |
| `effect` | an effect's name; a link, `{ "blur", passes = 3 }`, with the effect's params beside its name; a chain of links, run in order, each reading the last one's result, `{ { "blur", source = "self" }, { "tint", amount = 0.1 } }`; or `false`, which empties the slot |
| `source` | what the chain's first input reads in place of its own: `"self"`, the part's own pixels. On the rule or in its first link, never a later one |
| `mask` | how the result is cut: `"shape"`, the part's own shape (the default: the client's own rectangle and rounded corners for `client`; the whole window rounded at its largest corner for `pane`; the band, as above, for `region:titlebar`; the rectangle for the rest), or `"alpha"`, the alpha of the part's own pixels, which needs `source = "self"` |

A link may also give `reach` and `bleed`, in pixels, in place of the
effect's own. A chain reads as far around its part as its first link
reaches, and draws as far beyond it as all its links bleed together.

A pane style may give rules of its own, for the windows it frames, in an
`effects.lua` beside its `Pane.qml`
([pane styles](../qml/panes/README.md#effectslua)). For one part and one slot
the later rule wins: a pane style's rules first, then yours, so yours win
over a style's, and a later one of yours over an earlier one. `effect = false` in a later rule empties the slot an earlier one
filled. The part itself is always drawn: `behind` and `front` only add, and a
`replace` whose effect fails draws the part as if no rule named it.

Rules are applied whole. When your configuration loads, every effect a rule
names is loaded and every link bound with its params, and one rule that
cannot be (a key misspelled, a part or a slot that does not exist, an effect
nobody ships, a param it does not have, a shader that fails its checks) keeps
the whole set out: the rules that ran before stay, and what is wrong is on
the overlay, by the rule's number and key, or at the effect's own file and
line when the effect is what failed. `solium --check` fails on it too.

Some of what a rule can say waits for a later part of Solium, and is refused
until then, by name: an effect that reads the `backdrop` from what is behind
the window (blur's own input) needs xray, which arrives with X2.1, so give
`source = "self"` to blur the part's own pixels; the live backdrop
(`source = "live"` or `"auto"`) arrives with X4.1; regions other than the
titlebar, a `region:titlebar` rule whose chain reads the band's own pixels
(`self`), `part = "output"`, `keep`, a `surface` match naming a plane, and an
alpha mask without `source = "self"` likewise. This build reads, checks and
binds rules, names a broken one on the overlay, and draws an effect in its
slot. One that reads the part's own pixels runs again only when the part
commits or the effect's params or the part's size change. One that reads
nothing of the frame, only `shape` and its states (a border, a glow, a
gradient), needs no capture: it runs once, and again only when its params or
the part's size change, whatever the window draws under it; its `sol_tex`
reads a transparent texel.

## When it is read, and when it compiles

An effect's folder is read when your configuration loads, and again at every
reload, with no GPU: its `effect.lua`, its params and the checks of its
shaders below. Its shaders are compiled on the GPU just before the next frame.
An effect whose files have not changed is kept as it is, compiled and all, so
a reload that changes something else rebuilds nothing. A new version that
fails, in its Lua, its checks or its compile, leaves the version that ran
before it running; an effect that has never compiled is left out, as if it
were not named. A shader that failed to compile is not compiled again until
its file changes, or until a reload, which tries every failure once more.

## When something is wrong

What is broken is named with its file and line in the top-right corner of
your primary monitor, for as long as it is broken: a Lua error in an
`effect.lua`, a check of a shader that fails, a compile the GPU refused, a
rule that cannot be applied (above), a pane style's `effects.lua` that does
not run or a rule of it that cannot be bound. A
configuration that fails to reload is listed there too, at its own line, and
the configuration that was running before it keeps running. The list goes
when a reload leaves nothing broken. A version of an effect that fails keeps
the one that ran, as above, and a reload tries every recorded failure once
more.

The list is configuration like the rest. The compositor hands the rows to Lua
as `sol.problems()` and tells a `problems` listener when they change;
`lua/problems.lua` and `qml/problems.qml` draw them, and a configuration that
leaves out `require("problems")` has no overlay.

## Checking a folder

```sh
solium --check ~/.config/solium/effects/<name>
```

checks one effect folder and starts nothing: its `effect.lua` in a Lua of its
own, every key in it, the checks of its shaders below, and the effects it
names in turn (`pixels`, a `fallback` naming an effect, a `use` stage),
looked for beside it
first and then among the shipped folders, as when it runs, so a copy of a
shipped effect that names another shipped one passes. Errors are printed at
their file and line and exit 1; warnings are printed and pass. `solium
--check .` inside the folder does the same.

Its shaders are compiled on this machine's GPU, on the first render node, and
only there: a compile says what this machine's driver accepts and nothing
more. NVIDIA's accepts GLSL that Mesa's refuses, such as a loop bound that is
not a constant, or an `int` where a `float` is wanted, so an effect meant for
other machines is worth checking on one with Mesa as well. The same GPU is
asked whether it can draw into `rgba16f`, so an effect whose every version
needs it fails there. With no render node it says `shaders not compiled: no
render node` and `formats not checked: no render node`, keeps every version,
and that is not a failure.

Plain `solium --check` checks every folder in your `effects/` this way,
named or not, and every effect your configuration's rules name, and then
binds each rule as the compositor would, so a rule it would refuse fails
there too. Rules that do not parse fail by their number and key, and are not
bound: it says `rules not checked: effects.rules did not parse`. A folder with no `effect.lua` fails, and so does one whose name
cannot name an effect, such as `Blur`, since nothing could use it. The
`effects.lua` of each pane style of your own is read too, and fails on what
the overlay would name.

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
| `stages` | a many-pass effect's passes: a list, or a function of the params, below |
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

### Fallbacks

```lua
fallback = { { passes = 2 }, { passes = 1 }, "frost" },
```

Wherever the effect is used, its stages are put in place for the params it
was given, and again for each `fallback` in order: a table of params given
over those, or another effect at its defaults. That happens when the
configuration loads, never once a frame. A GPU that cannot draw into
`rgba16f` drops every version with a pass or a state in that format, so the
effect starts at the first version that does not; if none is left, the
effect is not drawn and is named on the overlay. Until Solium's first frame
has asked the GPU, every version is kept. An effect a `fallback` names is
loaded with it, and an effect draws only once the shaders of every version
compiled.

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

### Stages

A many-pass effect lists its passes in `stages`, each a table that begins
with its kind:

```lua
stages = function(p)
  local s = {}
  for i = 1, p.passes do s[#s + 1] = { "pass", "down.frag", scale = 0.5 } end
  for i = 1, p.passes do s[#s + 1] = { "pass", "up.frag", scale = 2 } end
  return s
end,
```

`stages` is a list, or a function of the params that returns one. The
function is called when the effect is loaded and where it is used with other
params, never once a frame, so a loop over `p.passes` costs nothing while the
effect draws.

| Stage | What it does |
|---|---|
| `{ "pass", "<file>.frag" }` | draws one shader into a texture of its own. `sol_tex` reads the last result, or the effect's first input in the first pass. `scale =` sizes it against what it reads, rounded up to whole pixels, 1 when not given; `format =` is `"rgba8"`, the default, or `"rgba16f"`; `uses = { … }` lists the other textures it reads, by name, at most seven, since a pass reads eight textures with its `sol_tex`; `input =` names what `sol_tex` reads in place of the last result |
| `{ "repeat", over = { 64, 32, … }, as = "<name>", <stages> }` | runs the stages after its kind once for each number in `over`, with `p_<name>` set to that number. The name is a new one, not one of the effect's params |
| `{ "save", "<name>" }`, `{ "get", "<name>" }` | names the last result; makes a named result the last one again |
| `{ "use", "<effect>", <param> = <value>, … }` | runs another effect's stages here, on the last result, with those params. The names it saves are its own, and where it reads its first input by name it reads what it was given. It is put in place when the effect is loaded, so it costs nothing while the effect draws |
| `{ "state", "<name>", depends = "…", stages = { … } }` | a texture kept from one run to the next, made by its own `stages` from the effect's inputs and the states before it, and made again only when what it `depends` on changes: `"shape"`, `"params"`, or `"self"`, a commit of the part's own surface; and when its size changes with the part's, or a state it reads is made again. Later stages read it by its name. `format =` and `scale =` as for a pass |

A pass with a `scale` above 1 after passes below 1 is the size of what the
matching smaller pass read, so a blur that halves a window three times and
doubles it three times comes back to its exact size, odd or not. Every stage
is checked as `effect.lua`'s own keys are, and a key a stage does not take is
refused with the one you probably meant. A name an effect saves or makes a
state is a new GLSL name, because every pass reads it as `sol_<name>`:
lower-case letters, digits and `_`, not one of the `sol_` names below, and
not another of its names followed by `_box` or `_sampler`. An effect may not
use itself, directly or through others, and one effect runs at most 256
passes, its states' included. `depends = "region"`, an outline a region publishes, waits
for regions, and is refused.

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
Every texture a pass reads is sampled linearly, so a read between two texels
blends them, as a blur's taps want, and `sol_tex` past its edge reads the
edge texel itself.
Your file is compiled as it is, between a prelude Solium writes for the pass
and a few lines that call `sol_effect`, and it stays a source string of its
own, so the line numbers Solium reports from the compiler are your file's own
lines, however the driver counts them. Do not
write `#version`, a `precision` or a `uniform` named `p_…` or `sol_…`: the
prelude has them, and a `uniform` of yours with one of those names is an
error at its line.

| Name | What it is |
|---|---|
| `sol_tex(uv)` | the pass's input: the previous pass's result, or the effect's first input in the first pass, clamped at its edge |
| `sol_texel`, `sol_size` | one texel of that input, in `uv`; the size of what the pass draws, in pixels |
| `p_<param>` | each of the effect's params, declared from `params`: a number is a `float` (an `int` with `int = true`), a boolean an `int` 0 or 1, four numbers a `vec4`; a word has none |
| `sol_<name>(uv)`, `SOL_HAS_<name>` | another texture the effect reads, by name: `sol_self`, `sol_backdrop`, a name it saved, or a state's. One the pass does not list in its `uses` reads transparent and its `SOL_HAS_<name>` is 0, so one file can be written for both cases |
| `sol_shape(uv)`, `sol_sdf(px)` | the part's shape: its coverage, and its signed distance in pixels, negative inside, exact for a rounded box |
| `sol_content`, `sol_to_content(uv)`, `sol_to_uv(px)`, `sol_sdf_rrect(px, size, radii)` | the part's rectangle inside the box the pass draws, the maps between `uv` and the part's pixels, and the rounded-box distance itself |
| `sol_progress`, `sol_clamped`, `sol_direction`, `sol_seed`, `sol_time` | in a transition: its progress, which a spring may carry past 1; the same clamped to 0..1; +1 arriving, −1 leaving, 0 resizing; a random number fixed for the transition; seconds |
| `sol_noise(p)` | value noise |

A `p_` name the effect has no param for, and a `sol_` name that is none of
these, are errors at their line, with the param you probably meant; reading a
texture the pass does not list in its `uses` is a warning, since it reads
transparent. Comments are skipped.

## The shipped folders

| Folder | What it is |
|---|---|
| [`blur`](blur/README.md) | a dual Kawase blur, `passes` halvings and as many doublings back at `offset` texels; its input is the backdrop, so until xray a rule gives it `source = "self"` |
