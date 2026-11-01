-- A circular wave travelling from the window center toward every edge.
return {
    api = 1,
    duration = 1000,
    easing = "linear",
    grid = { along = 40, across = 28 },
    params = { strength = { 10, min = 0, max = 40 }, width = { 180, min = 60, max = 180 } },
    mesh = function(t, cols, rows, out)
        local p, n = 1 - t.clamped, 0
        local farthest = math.sqrt(t.from.w ^ 2 + t.from.h ^ 2) / 2
        local front = -t.width + p * (farthest + 2.5 * t.width)
        local fade = math.min(1, p * 10, (1 - p) * 10)
        if t.clamped <= 0 or t.clamped >= 1 then fade = 0 end
        for r = 0, rows do
            for c = 0, cols do
                local u, v = sol_grid(t, c, r)
                local x, y = t.from.x + u * t.from.w, t.from.y + v * t.from.h
                local dx, dy = (u - 0.5) * t.from.w, (v - 0.5) * t.from.h
                local radius = math.sqrt(dx * dx + dy * dy)
                local q = (radius - front) / t.width
                local center = radius / math.sqrt(radius * radius + 60 * 60)
                local wave = t.strength * fade * center * math.sin(math.pi * q) * math.exp(-q * q * 0.5)
                if radius > 0.000001 then
                    x, y = x + dx / radius * wave, y + dy / radius * wave
                end
                out[n + 1], out[n + 2], n = x, y, n + 2
            end
        end
    end,
}
