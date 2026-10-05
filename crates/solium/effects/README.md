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

## The shipped folders

None yet.
