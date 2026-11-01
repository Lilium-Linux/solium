-- How windows and the wallpaper look, beyond what config.lua offers:
--
--   * every window is a little see-through: the one with the keyboard at
--     ACTIVE, the others at INACTIVE, easing between them over FADE as focus
--     moves (a slot crossfades from one rule to the next, #389); a
--     fullscreen one (a game, a video) is solid. Only the window's own pixels
--     fade (effects/alpha), not the blur behind it: a group's opacity faded
--     both, and the sharp wallpaper showed through the blur;
--   * one frame, rounded-violet, tiled and floating: its titlebar folds
--     into the border as a window goes into a tile and unfolds as it floats
--     again, animated with the window's glide (qml/panes/rounded-violet/
--     Frame.qml). The style is never switched, so nothing is rebuilt at a
--     switch and nothing stalls;
--   * every window has a border, the ones that draw their own titlebar too
--     (Claude, a browser), which no pane style frames: for those an effect
--     draws it just outside the window (effects/outline), and another cuts
--     their corners round (effects/corners), as the frames do -- rounded at
--     12 tiling and floating alike, violet with the keyboard and dim without;
--   * what is behind every window is blurred, so a see-through window is
--     frosted rather than showing what is under it sharp, and what is behind the bar
--     (a surface of its own, bar.lua) is blurred under its see-through
--     colour;
--   * switching from floating to a tiling layout remembers where every
--     window was, and switching back glides each one there again, so a
--     desktop arranged by hand comes back as it was;
--   * the wallpaper is told which workspace each monitor shows, and whether
--     the workspace stack (deskstack.lua) is open, so it can pan with the
--     workspaces and settle back behind the stack (qml/wallpaper.qml).
--
-- Required from user.lua while the configuration is still loading, so the
-- shipped modules are reached inside functions and the hooks go in once the
-- scripts have loaded (`monitors` at startup, `restore` after a reload).

local looks = {}

-- What is behind every window: "blur", frosted; or "glass", the shipped
-- liquid glass (effects/glass), frosted too, its rim bending what is behind
-- it, lit along its edges. The bar has the glass either way.
local MATERIAL = "blur"
local ACTIVE = 0.88
local INACTIVE = 0.80
local FADE = { duration = 200, easing = "outCubic" }
local PANE = "rounded-violet"
-- The titlebar's height (Theme.titlebarHeight): what rounded-violet's frame
-- takes above a floating window. Tiled, it folds to a band (Frame.qml),
-- rounded-violet's `insets.top`.
local TITLEBAR = 32
local BAND = 1
local TILING_MODES = { tiling = true, scrolling = true }

local VIOLET = { 0.576, 0.427, 1, 1 }   -- #936dff, the frames' focused border
local DIM = { 0.184, 0.18, 0.2, 1 }      -- #2f2e33, their border without the keyboard
-- The corners, the same tiling and floating: both pane styles' client.radius.
local RADIUS = 12

-- Where each window was while floating, by id, kept across a reload, from the
-- moment a layout takes over until floating comes back.
local kept = sol.keep("looks", { floated = {} })
local RETURN = { duration = 320, easing = "outCubic" }

-- The groups an earlier version of this file faded windows with, one per
-- window ("dim-<id>"): taken away, so nothing fades the blur behind a window.
local function ungroup()
    for _, w in ipairs(sol.windows()) do
        sol.group("dim-" .. w.id, false)
    end
end

local pane_now

-- The effect rules: blur behind every window, every window's own pixels a
-- little see-through, and the self-drawn ones bordered and rounded.
local function rules()
    local radius = RADIUS
    -- A blur that dithers its own last pass against banding
    -- (effects/blur/last.frag): one chain, run once per monitor and shared by
    -- every window on it.
    local blur = { "blur", passes = 5, offset = 6, dither = 2 }
    -- Its blur is the windows' own (the same passes and offset), so the one
    -- blur per monitor still serves every window and the bar; the rim, the
    -- light and the shade are each window's own pass.
    local glass = { "glass", radius = RADIUS, soften_passes = 5, soften_offset = 6, haze = 1, rim_haze = 0.7 }
    local behind = MATERIAL == "glass" and glass or blur
    local list = {}
    for _, style in ipairs({ PANE }) do
        list[#list + 1] = {
            match = { style = style, fullscreen = false },
            part = "pane", slot = "behind",
            effect = behind,
        }
        for _, has_keyboard in ipairs({ true, false }) do
            list[#list + 1] = {
                match = { style = style, fullscreen = false, focused = has_keyboard },
                part = "client", slot = "replace", source = "self",
                effect = { "alpha", amount = has_keyboard and ACTIVE or INACTIVE },
                fade = FADE,
            }
        end
    end
    -- Behind each monitor's bar.
    local bar = package.loaded["bar"]
    for _, surface in ipairs(bar and bar.names() or {}) do
        list[#list + 1] = {
            match = "*", part = "surface:" .. surface, slot = "behind",
            -- The bar's glass: its rim narrower, as the bar is 40 px tall; its
            -- blur the windows' own, so one blur per monitor serves both.
            effect = { "glass", soften_passes = 5, soften_offset = 6, bezel = 14, bend = 4,
                       shine = 0, shine_back = 0, shade = 0 },
        }
    end
    -- A window Solium draws without a frame (`style = "none"`: one that
    -- draws its own titlebar, a browser, Claude, ChatGPT) gets from effects
    -- what a frame gives the others: its corners cut round, the same
    -- see-through, the blur behind it and a border. Every such window, by
    -- what it is rather than by name; a fullscreen one is left alone.
    --
    -- Square while it fills its area, as a framed window's are (Pane.qml):
    -- maximised, or snapped to a half or a quarter (snap.lua hands it
    -- `snapped`, the zone's name; a key never handed matches `false`).
    local shapes = {
        { match = { maximized = false, values = { snapped = false } }, radius = radius },
        { match = { maximized = true }, radius = 0 },
    }
    for _, zone in ipairs({ "left", "right", "top_left", "top_right", "bottom_left", "bottom_right" }) do
        shapes[#shapes + 1] = { match = { maximized = false, values = { snapped = zone } }, radius = 0 }
    end
    for _, shape in ipairs(shapes) do
        local function bare(more)
            local match = { style = "none", fullscreen = false }
            for key, value in pairs(shape.match) do
                match[key] = value
            end
            for key, value in pairs(more or {}) do
                match[key] = value
            end
            return match
        end
        local cut = shape.radius > 0 and { { "corners", radius = shape.radius } } or {}
        for _, has_keyboard in ipairs({ true, false }) do
            local client = {}
            for _, step in ipairs(cut) do
                client[#client + 1] = step
            end
            client[#client + 1] = { "alpha", amount = has_keyboard and ACTIVE or INACTIVE }
            list[#list + 1] = {
                match = bare({ focused = has_keyboard }),
                part = "client", slot = "replace", source = "self",
                effect = client,
                fade = FADE,
            }
            list[#list + 1] = {
                match = bare({ focused = has_keyboard }),
                part = "pane", slot = "front",
                effect = { "outline", radius = shape.radius, width = 1, color = has_keyboard and VIOLET or DIM },
                fade = FADE,
            }
        end
        -- The blur behind it, cut to the same corners (the glass cuts its own).
        local behind = blur
        if MATERIAL == "glass" then
            behind = glass
        elseif shape.radius > 0 then
            behind = { blur, { "corners", radius = shape.radius } }
        end
        list[#list + 1] = { match = bare(), part = "pane", slot = "behind", effect = behind }
    end
    return list
end

-- For `solium --check`, which loads the configuration but runs no hook.
looks.rules = rules

-- Handed over with the rest of `config.effects`, which these replace the
-- rules of.
local function effects()
    local settings = {}
    for key, value in pairs(require("config").effects or {}) do
        settings[key] = value
    end
    settings.rules = rules()
    settings.load = { "desktop-flight" }
    sol.effects(settings)
    -- A rule the compositor refuses keeps the whole set out, and says so only
    -- here: in the log too, so it is not missed.
    for _, problem in ipairs(sol.problems() or {}) do
        sol.log("looks: effects problem: " .. tostring(problem.message)
            .. " (" .. tostring(problem.file) .. ":" .. tostring(problem.line or "") .. ")")
    end
end

-- Again, as the bars come and go with the monitors (bar.lua).
function looks.refresh()
    if pane_now then
        effects()
    end
end

function looks.pane()
    if pane_now ~= PANE then
        pane_now = PANE
        sol.pane(PANE)
        effects()
    end
end

-- What a window's frame takes above it once it floats: a titlebar in this
-- configuration's frame, and what it takes now in any other (a window that
-- draws its own titlebar has no frame at all).
local function floating_top(w)
    if w.style == PANE then
        return TITLEBAR
    end
    return w.insets and w.insets.top or 0
end

-- How much more a window's frame will take above it than it does now: what
-- its titlebar still has to unfold by, which a window's height grows by at
-- its bottom as it does. Nothing for a window whose bar is out already (one
-- that floated over the tiles), or that has none.
--
-- What the frame takes now is the compositor's (`insets`, #435). A build
-- from before that says nothing of it, and then `folded` says what to take
-- it for: a window of this frame folded (coming back from a tile) or not
-- (floating), and any other with no frame -- a window that draws its own
-- titlebar is never shortened.
local function to_unfold(w, folded)
    local now
    if w.insets then
        now = w.insets.top
    elseif w.style == PANE then
        now = folded and BAND or TITLEBAR
    else
        now = 0
    end
    return math.max(0, floating_top(w) - now)
end

-- Where every window is, as a layout is about to tile them, as it is with its
-- titlebar all the way out: a bar still unfolding from a quick switch back is
-- counted whole, so what is kept does not depend on how quick the switch was.
function looks.remember()
    kept.floated = {}
    for _, w in ipairs(sol.windows()) do
        if not w.leaving and not w.minimized and not w.fullscreen and w.w > 0 and w.h > 0 then
            kept.floated[w.id] = { x = w.x, y = w.y, w = w.w, h = w.h + to_unfold(w, false) }
        end
    end
end

-- And every one of them back there, gliding, as floating takes over again;
-- a window opened while tiling stays where the layout left it. Placed short
-- of what its titlebar still has to unfold by: the frame unfolds it on top,
-- the window's content moving down by as much, and the window ends where it
-- was -- whatever the bar had folded by, and for a window with no bar to
-- unfold (one that floated over the tiles, or draws its own) by nothing.
function looks.put_back()
    local open = {}
    for _, w in ipairs(sol.windows()) do
        if not w.leaving and not w.minimized and not w.fullscreen then
            open[w.id] = w
        end
    end
    sol.animate(RETURN)
    for id, w in pairs(open) do
        -- One opened while tiling stays where its tile was, its bar unfolding
        -- inside that rather than past its bottom.
        local r = kept.floated[id] or { x = w.x, y = w.y, w = w.w, h = w.h }
        sol.place(id, { x = r.x, y = r.y, w = r.w, h = r.h - to_unfold(w, true), tile = false })
    end
    kept.floated = {}
end

-- The wallpaper's view of the desk: which workspace each monitor shows, or,
-- while the workspace stack is open, the one chosen there.
local stack = { open = false, monitor = nil, chosen = nil }

function looks.wallpaper(open, monitor, chosen)
    if open ~= nil then
        stack = { open = open, monitor = monitor, chosen = chosen }
    end
    local config = require("config")
    local setting = config.wallpaper
    -- Only a single image is one surface named "wallpaper"; a list is one per
    -- desk, carried by the workspaces themselves.
    if type(setting) ~= "string" then
        return
    end
    local workspaces = require("workspaces")
    local desks = {}
    for _, m in ipairs(sol.monitors()) do
        desks[m.name] = (stack.open and stack.monitor == m.name and stack.chosen) or workspaces.on(m.name)
    end
    sol.surface("wallpaper", {
        scene = "wallpaper.qml",
        layer = "background",
        on = "every-monitor",
        properties = {
            source = require("wallpaper").source(setting),
            desks = desks,
            count = workspaces.count(),
            stack = stack.open,
            stackMonitor = stack.monitor or "",
        },
    })
end

local hooked = false

local function hook()
    if hooked then
        return
    end
    hooked = true

    local modes = require("modes")
    local use = modes.use
    modes.use = function(name, ...)
        local before = modes.current()
        local floating_before = not TILING_MODES[before]
        if floating_before and TILING_MODES[name] then
            looks.remember()
        end
        use(name, ...)
        if not floating_before and not TILING_MODES[modes.current()] then
            looks.put_back()
        end
        looks.pane()
    end

    -- Every workspace change goes through `apply`.
    local workspaces = require("workspaces")
    local apply = workspaces.apply
    workspaces.apply = function(...)
        local result = apply(...)
        looks.wallpaper()
        return result
    end

    -- The groups earlier versions of this file dimmed with.
    sol.group("inactive", false)
    ungroup()

    looks.pane()
    looks.wallpaper()
end

sol.on("monitors", hook)
sol.on("restore", hook)
sol.on("close", function(id)
    kept.floated[id] = nil
end)

return looks
