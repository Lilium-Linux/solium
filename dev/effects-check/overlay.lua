-- effects-check's `overlay` section: nothing but the overlay and a reload
-- binding, so the only thing that can appear in the top-right corner is it.
require("problems")
sol.pane("none")
sol.bind("super+shift+r", function() sol.reload() end)
