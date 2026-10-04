-- `preview = false` turns the whole preview shell off, and leaves no shell
-- hosted when `user.lua` names none of its own either.
--
-- Played by `scenario::tests::every_scenario_with_a_client_passes`.

return {
    user = [[ return { preview = false } ]],
    init = [[ require("preview.init"); require("shell") ]],
    steps = {
        {
            expect = function(world)
                assert(world.surfaces["shell"] == nil, "preview = false still hosted a shell")
            end,
        },
    },
}
