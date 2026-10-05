-- Quick search starts closed, with nothing searched, the moment the preview
-- shell is hosted: `lua/preview/search.lua`'s own `search.apply()` writes
-- both onto `config.shell.properties` before `shell.lua`'s first declare,
-- the same way `preview-bar-default.lua` beside this file checks the shell
-- is hosted at all.
--
-- The rest of this scenario is the `super+d` binding itself, through the
-- real input path -- `fullscreen-animate-true.lua`'s own pattern (`{ key =
-- ... }` then an `expect` on the state that followed), the one thing
-- `preview-search-match.lua` beside this file does not cover (its own
-- comment says why). No `{ open = true }` first: `search.lua`'s binding is
-- compositor-global (`sol.bind`, intercepted before a focused client ever
-- sees the combo -- `input/mod.rs`'s own module doc), not bound to any one
-- window.
--
-- Played by `scenario::tests::every_scenario_with_a_client_passes`.

return {
    init = [[ require("preview.init"); require("shell") ]],
    steps = {
        {
            expect = function(world)
                local properties = world.surfaces["shell"].properties
                assert(properties.searchOpen == false, "search starts closed")
                assert(
                    type(properties.searchResults) == "table" and #properties.searchResults == 0,
                    "search starts with nothing searched"
                )
            end,
        },
        { key = "super+d" },
        {
            expect = function(world)
                local properties = world.surfaces["shell"].properties
                assert(properties.searchOpen == true, "super+d opens quick search")
            end,
        },
        { key = "super+d" },
        {
            expect = function(world)
                local properties = world.surfaces["shell"].properties
                assert(properties.searchOpen == false, "super+d again closes it")
            end,
        },
    },
}
