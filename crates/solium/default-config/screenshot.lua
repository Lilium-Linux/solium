-- Screenshots: the Lua half (~/.config/solium/shell/ScreenshotTool.qml and
-- ShotThumbnail.qml are the scene's).
--
--   screenshot.screen()   the whole desktop, at once: it shrinks into the
--                         bottom-right corner and goes after ten seconds;
--   screenshot.window(id) one window, as it is on screen (the titlebar's
--                         quick settings);
--   screenshot.region()   the tool: the desktop frozen, a region or a window
--                         picked on it, or a whole monitor.
--
-- Every shot is saved to ~/Pictures/Screenshots and copied to the clipboard.
--
-- The pixels come from grim (wlr-screencopy, which Solium serves), the crop
-- from ImageMagick. Both run through sol.run, which waits for nothing, so the
-- scene learns a file is ready by loading it: it asks again until the image
-- is there.
--
-- The tool works on a picture of the desktop taken when it opens, not on the
-- live one: what is cropped is what was on screen when the rectangle was
-- drawn, whatever has moved since.
--
-- Required from user.lua while the configuration is still loading, so the
-- shipped modules are required inside the functions.

local screenshot = {}

local kept = sol.keep("screenshot", { serial = 0 })

local HOME = os.getenv("HOME")
local DIR = HOME .. "/Pictures/Screenshots"
local RUNTIME = os.getenv("XDG_RUNTIME_DIR") or "/tmp"
-- The tool's picture of the desktop: a new name each time, so the scene can
-- never load the last one while grim is still writing this one.
local function frozen_path(serial)
    return RUNTIME .. "/solium-screenshot-" .. serial .. ".png"
end

local function quote(text)
    return "'" .. tostring(text):gsub("'", "'\\''") .. "'"
end

local function sh(command)
    sol.run("sh", "-c", command)
end

-- Hand the scene the tool's state, the way taskbar.lua hands it the battery.
local function push(state)
    local config = require("config")
    config.shell = type(config.shell) == "table" and config.shell or {}
    config.shell.properties = type(config.shell.properties) == "table" and config.shell.properties or {}
    config.shell.properties.shot = state
    require("shell").apply()
end

local function exists(path)
    local file = io.open(path, "r")
    if file then
        file:close()
        return true
    end
    return false
end

-- A new file's name: the time it was taken, oldest first in a plain `ls`,
-- and a count after it if two land in the same second.
local function new_path()
    local stem = DIR .. "/Screenshot-" .. os.date("%Y%m%d-%H%M%S")
    local path = stem .. ".png"
    local n = 2
    while exists(path) do
        path = stem .. "-" .. n .. ".png"
        n = n + 1
    end
    return path
end

-- The desktop as grim takes it: the box around every monitor, in the global
-- logical space.
local function desktop()
    local x1, y1, x2, y2
    for _, monitor in ipairs(sol.monitors()) do
        local m = monitor.whole or monitor
        x1 = x1 and math.min(x1, m.x) or m.x
        y1 = y1 and math.min(y1, m.y) or m.y
        x2 = x2 and math.max(x2, m.x + m.w) or m.x + m.w
        y2 = y2 and math.max(y2, m.y + m.h) or m.y + m.h
    end
    return { x = x1 or 0, y = y1 or 0, w = (x2 or 0) - (x1 or 0), h = (y2 or 0) - (y1 or 0) }
end

-- Windows the tool can pick: on screen, not minimised, in the global logical
-- space.
local function windows()
    local list = {}
    for _, w in ipairs(sol.windows()) do
        if w.shown ~= false and not w.minimized and not w.leaving and w.w > 0 and w.h > 0 then
            list[#list + 1] = { id = w.id, x = w.x, y = w.y, w = w.w, h = w.h }
        end
    end
    return list
end

-- Saved and copied, then shown: written under another name and moved, so the
-- scene never loads half a file.
local function deliver(make, path, from, rect)
    kept.serial = kept.serial + 1
    local part = path .. ".part"
    sh(string.format("mkdir -p %s && %s && mv %s %s && wl-copy --type image/png < %s",
        quote(DIR), make(part), quote(part), quote(path), quote(path)))
    push({ serial = kept.serial, phase = "taken", path = path, from = from,
           rect = rect or desktop(), desktop = desktop() })
end

-- The whole desktop. The short wait lets the last shot's thumbnail, which the
-- scene hides the moment a new one is asked for, leave the screen first.
function screenshot.screen()
    deliver(function(part)
        return "sleep 0.15 && grim -t png " .. quote(part)
    end, new_path(), "screen")
end

-- One window, as it is on screen, after a short wait so a panel just folded
-- away over it (the titlebar's quick settings) is gone first.
function screenshot.window(id)
    for _, w in ipairs(sol.windows()) do
        if w.id == id and w.w > 0 and w.h > 0 then
            local rect = { x = w.x, y = w.y, w = w.w, h = w.h }
            deliver(function(part)
                return string.format("sleep 0.35 && grim -t png -g %s %s",
                    quote(string.format("%d,%d %dx%d", math.floor(w.x), math.floor(w.y),
                        math.floor(w.w), math.floor(w.h))), quote(part))
            end, new_path(), "window", rect)
            return
        end
    end
end

-- The tool: freeze the desktop, then let the scene draw on the picture.
function screenshot.region()
    kept.serial = kept.serial + 1
    local frozen = frozen_path(kept.serial)
    kept.frozen = frozen
    sh(string.format("rm -f %s/solium-screenshot-*.png; sleep 0.15 && grim -t png %s.part && mv %s.part %s",
        quote(RUNTIME), quote(frozen), quote(frozen), quote(frozen)))
    push({ serial = kept.serial, phase = "selecting", frozen = frozen,
           desktop = desktop(), windows = windows() })
end

sol.on("surface", function(_, action, data)
    data = type(data) == "table" and data or {}
    if action == "shot.cancel" then
        kept.serial = kept.serial + 1
        push({ serial = kept.serial, phase = "idle" })
    elseif action == "shot.take" then
        -- `crop` is in the frozen picture's own pixels; `rect` is where it
        -- was on screen, in the global logical space, for the thumbnail to
        -- start from.
        local c = type(data.crop) == "table" and data.crop or {}
        local x, y = math.floor(tonumber(c.x) or 0), math.floor(tonumber(c.y) or 0)
        local w, h = math.floor(tonumber(c.w) or 0), math.floor(tonumber(c.h) or 0)
        if w < 1 or h < 1 then
            return
        end
        local rect = type(data.rect) == "table" and data.rect or nil
        deliver(function(part)
            return string.format("magick %s -crop %dx%d+%d+%d +repage png:%s",
                quote(kept.frozen or ""), w, h, x, y, quote(part))
        end, new_path(), "region", rect)
    elseif action == "shot.open" and type(data.path) == "string" then
        sol.run("xdg-open", data.path)
    end
end)

return screenshot
