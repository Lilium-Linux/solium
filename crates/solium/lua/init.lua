-- Solium's default configuration.
--
-- Copy to ~/.config/solium/init.lua to change it; the compositor prefers that
-- file when it exists. Modes live in their own scripts and register their own
-- bindings, so adding one is a `require`, and removing one is deleting a line.

require("overview")

sol.log("solium configuration loaded")
