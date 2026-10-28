-- effects-check's `regions` section: regions.qml on `top` over every
-- monitor, nothing else on screen, and super+a writing `sol.anchors`
-- under "probe/" to the log, one line each, which the script compares with
-- where the capture shows each square.
sol.pane("none")
sol.surface("probe", { scene = "regions.qml", layer = "top" })
sol.bind("super+a", function()
    for _, anchor in ipairs(sol.anchors("probe/")) do
        sol.log(string.format("anchor %s %s %g %g %g %g %s", anchor.name, anchor.monitor,
            anchor.x, anchor.y, anchor.w, anchor.h, tostring(anchor.shown)))
    end
end)
