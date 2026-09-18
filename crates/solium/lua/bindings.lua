-- What `config.bindings` actually does.
--
-- The shipped bindings are `sol.bind` calls, spread across this file's
-- neighbours: `init.lua` has the terminal and the session keys, `scrolling.lua`
-- has the strip's, `workspaces.lua` builds `super+1` through `super+9` in a
-- loop. That is the right place for them -- a mode's keys belong with the mode
-- -- but it left a configuration with nowhere to put one of its own except a
-- copy of `init.lua`, which means adopting two hundred lines of somebody else's
-- session in order to open a browser. `config.bindings` is the settings-table
-- answer to that, and this file is the twenty lines that make it real (#117).
--
-- Required from the *last* line of `init.lua`, and that is load-bearing rather
-- than tidy. `sol.bind` lets the later call win, so being read last is the
-- whole mechanism by which a configured binding takes over a shipped
-- combination. Required earlier, `scrolling.lua` would quietly bind over the
-- user instead, and which of them won would depend on the order of the
-- `require`s above -- the exact kind of thing nobody would think to check.

local config = require("config")

-- Split on spaces, the same way and for the same reason `init.lua` does: a
-- terminal often needs arguments to open a *new* window, and `"kitty -e nvim"`
-- should mean what it looks like it means. An argument that itself contains a
-- space is what the list form is for.
local function words(text)
    local parts = {}
    for word in string.gmatch(text, "%S+") do
        parts[#parts + 1] = word
    end
    return parts
end

-- Turn one entry of the table into something `sol.bind` will take.
--
-- Raises rather than skipping. A binding written as a number is a mistake, and
-- the alternative -- log it and carry on -- is the failure this whole change is
-- about: a line in a configuration file that has no effect and says nothing.
-- Raising is safe here in a way it would not be deeper in: `config.lua`'s own
-- contract is that a broken user file costs `--check` an error message and
-- costs a reload nothing at all, because the running session is kept when the
-- new one fails to load.
local function handler(combo, what)
    if type(what) == "function" then
        return what
    end

    local argv
    if type(what) == "string" then
        argv = words(what)
    elseif type(what) == "table" then
        argv = what
    else
        error(("bindings[%q] must be a command, a list, a function or false, not %s")
            :format(combo, type(what)), 0)
    end

    if #argv == 0 or type(argv[1]) ~= "string" then
        error(("bindings[%q] has no program to run"):format(combo), 0)
    end

    -- Asked once, here, rather than discovered by pressing the key. A binding
    -- that spawns a program this machine does not have is indistinguishable
    -- from a binding that is broken -- which is how the first hardware session
    -- went, and why `init.lua` picks its terminal by asking. Not an error: the
    -- program may be installed later, and refusing the whole configuration over
    -- a typo'd program name would be worse than saying so.
    if not sol.which(argv[1]) then
        sol.log(("bindings: %s runs %s, which is not installed"):format(combo, argv[1]))
    end

    return function()
        sol.spawn(table.unpack(argv))
    end
end

for combo, what in pairs(config.bindings or {}) do
    -- Asked *before* binding, because afterwards the answer is always yes. The
    -- note is what `solium --check` prints beside the combination, and it is
    -- the reason replacing a shipped binding is a decision rather than a
    -- surprise: see the `bindings` comment in `config.lua`.
    local taken = sol.bound(combo)
    if what == false then
        -- Only worth reporting when there was something there. Unbinding a
        -- combination nothing had bound is a no-op, and calling that "removed"
        -- in `--check` would be a line about nothing.
        if taken then
            sol.unbind(combo, "config.bindings")
        end
    else
        sol.bind(combo, handler(combo, what),
            taken and "config.bindings, replacing a shipped binding" or "config.bindings")
    end
end
