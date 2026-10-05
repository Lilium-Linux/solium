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

## The shipped folders

None yet.
