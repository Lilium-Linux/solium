-- Quick search starts closed, with nothing searched, the moment the preview
-- shell is hosted: `lua/preview/search.lua`'s own `search.apply()` writes
-- both onto `config.shell.properties` before `shell.lua`'s first declare,
-- the same way `preview-bar-default.lua` beside this file checks the shell
-- is hosted at all.
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
    },
}
