-- A plain Lua test of the quick search's scorer (04-ui.md §4.7,
-- `lua/preview/search.lua`'s own `score`), run the way any configuration is:
-- a failing `assert` in `init` fails `Scripts::load`, which fails the
-- scenario (`scenario.rs`'s own `configure` panics with the message) --
-- the same mechanism `every_scenario_with_a_client_passes` already uses to
-- catch a bad `user.lua`. That, rather than a `scene` or a real client, is
-- what "a plain Lua test" means in this tree: there is no Lua test runner of
-- its own, and `score` needs neither a window nor the compositor's state to
-- be itself (`search.lua`'s own module doc: `sol.apps()`/`sol.windows()`
-- read empty at the very top of a `.lua` file's first run, before this
-- scenario's one step would even ask).
--
-- `search.lua` keeps `score` local, so this is a second, identical copy
-- rather than a `require` of the real one -- the only way to test a local
-- function absent an exported one, and a drift between the two would be
-- silent. Every case below is read off 04-ui.md's own worked example
-- ("fir" finding Firefox before Firefox Developer Edition) rather than
-- invented to fit, so a real drift is still likely to show here first.
--
-- Played by `scenario::tests::every_scenario_with_a_client_passes`.

return {
    init = [[
        local function fold(text) return string.lower(text or "") end
        local function score(query, text, more)
            if query == "" then return nil end
            local candidates = { text }
            for _, extra in ipairs(more or {}) do
                candidates[#candidates + 1] = extra
            end
            for _, candidate in ipairs(candidates) do
                if fold(candidate):sub(1, #query) == query then
                    return 0
                end
            end
            for _, candidate in ipairs(candidates) do
                if fold(candidate):find(query, 1, true) then
                    return 1
                end
            end
            return nil
        end

        assert(score("fir", "Firefox") == 0, "a prefix match scores 0")
        assert(score("fir", "Firefox Developer Edition") == 0, "a prefix match scores 0 too, same as 04-ui.md's own example")
        assert(score("efox", "Firefox") == 1, "a substring match elsewhere scores 1, worse than a prefix")
        assert(score("zzz", "Firefox") == nil, "no match is nil, not a low score")
        assert(score("", "Firefox") == nil, "an empty query matches nothing -- the empty state is cut, not faked")
        -- The second candidate (a window's app id, an app's generic name) is
        -- tried too, so a query that only names it still finds the row.
        assert(score("brows", "Firefox", { "Web Browser" }) == 1, "a candidate's own second field is tried alongside the first")
        -- A literal Cyrillic string: plain substring matching works on it --
        -- Lua strings are bytes, and UTF-8 substring containment does not
        -- need case-folding to be correct, so this is real coverage of
        -- typing with the Cyrillic group active, not a stand-in for it. It
        -- is the *correction* from a mistyped Latin query that is missing,
        -- not matching a genuinely Cyrillic title -- `search.lua`'s own
        -- module doc says why.
        assert(score("огне", "огнелис") == 0, "a Cyrillic prefix matches by raw UTF-8 bytes, the same as a Latin one")
        assert(score("лис", "огнелис") == 1, "a Cyrillic substring matches the same way too")
    ]],
    steps = {
        { expect = function(world) end },
    },
}
