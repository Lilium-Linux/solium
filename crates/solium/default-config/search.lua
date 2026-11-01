-- Quick search: apps, open windows and a few compositor commands, matched
-- and ranked here, shown by shell/Search.qml. The panel sends what is typed
-- with `Solium.send("search.query", { text = ... })`, and this file answers
-- with the ranked rows (and the section headers they are drawn under) as
-- the shell's `searchResults` property, through `shell.lua`'s generic
-- `properties` passthrough; `searchOpen` opens and closes it.
--
-- Matching is on an app's name and generic name, and on a window's title:
-- `sol.apps()` carries no keywords. A query typed in the wrong keyboard
-- layout is not corrected.
--
-- Required from user.lua, while the configuration is still loading, so
-- `config` and `shell` are required inside the functions rather than up
-- here, and nothing is pushed until the key is first pressed: the shell's
-- own defaults are a closed, empty panel.

local search = {}

-- The key that opens and closes the panel.
search.key = "super+d"

-- `workspaces.lua` is required on use: this file loads while the
-- configuration does, before the shipped `actions` that `workspaces` needs.
local function workspace_of(window_id, monitor)
    return require("workspaces").at(window_id, monitor)
end

-- A few compositor commands, mapped only onto actions that already exist.
-- 04-ui.md's own COMMANDS row is the source, trimmed to that: Lock screen
-- is its own example of one to leave out, and Show desktop, Suspend,
-- Restart, Power off, Screenshot region, Edit configuration and Keys have
-- no Lua or `sol.*` entry point this file can call without inventing one.
local COMMANDS = {
    {
        key = "command.overview",
        title = "Overview",
        subtitle = "Super+Space",
        run = function() require("overview").toggle() end,
    },
    {
        key = "command.tile",
        title = "Tile windows",
        subtitle = "",
        run = function() require("tiling").toggle() end,
    },
    {
        key = "command.fullscreen",
        title = "Fullscreen window",
        subtitle = "",
        run = function()
            for _, window in ipairs(sol.windows()) do
                if window.focused then
                    sol.act("windows.fullscreen", { id = window.id })
                    return
                end
            end
        end,
    },
    {
        key = "command.reload",
        title = "Reload configuration",
        subtitle = "Super+Shift+R",
        run = function() sol.reload() end,
    },
    {
        key = "command.quit",
        title = "Quit Solium",
        subtitle = "Super+Shift+Q",
        run = function() sol.quit() end,
    },
}

-- `string.lower` only folds ASCII -- Lua has no locale-aware case folding --
-- so a Cyrillic query matches a Cyrillic candidate only when the case
-- already agrees. Fine for app names and window titles, which are
-- overwhelmingly Latin, and never wrong, only sometimes case-sensitive
-- where a fully folding match would not be; see this file's own module doc.
local function fold(text)
    return string.lower(text or "")
end

-- `query` (already folded) against `text` and each of `more` -- a generic
-- name, an app id -- trying every candidate for a prefix match before
-- trying any of them for a substring one, so a prefix match anywhere beats
-- a substring match everywhere: 0, or 1, or nil for no match at all.
-- 04-ui.md's own "best first" is a sort on this, ties broken by title
-- (`search.match`, below).
local function score(query, text, more)
    if query == "" then
        return nil
    end
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

local function sorted(scored)
    table.sort(scored, function(a, b)
        if a.s ~= b.s then
            return a.s < b.s
        end
        return a.row.title < b.row.title
    end)
    return scored
end

-- Apps, windows and commands, matched and ranked, with a section header
-- before each non-empty group -- flat, because that is what a `ListView`
-- over a plain array wants (shell/Search.qml's `rows`). Only meaningfully called once a real snapshot exists -- like
-- `sol.apps()` and `sol.windows()` themselves, empty at the very top of a
-- `.lua` file's first run (their own module docs).
function search.match(query)
    local folded = fold(query)

    local apps = {}
    for _, app in ipairs(sol.apps()) do
        local s = score(folded, app.name, { app.generic_name })
        if s then
            apps[#apps + 1] = {
                s = s,
                row = {
                    kind = "app",
                    key = "app:" .. app.id,
                    id = app.id,
                    title = app.name,
                    subtitle = app.generic_name,
                    icon = app.icon ~= "" and app.icon or app.id,
                },
            }
        end
    end
    sorted(apps)

    local windows = {}
    for _, window in ipairs(sol.windows()) do
        local s = score(folded, window.title, { window.app_id })
        if s then
            local desk = workspace_of(window.id, window.monitor)
            windows[#windows + 1] = {
                s = s,
                row = {
                    kind = "window",
                    key = "window:" .. window.id,
                    windowId = window.id,
                    title = window.title ~= "" and window.title or window.app_id,
                    subtitle = "Desk " .. tostring(desk),
                    icon = window.app_id,
                },
            }
        end
    end
    sorted(windows)

    local commands = {}
    for _, command in ipairs(COMMANDS) do
        local s = score(folded, command.title, {})
        if s then
            commands[#commands + 1] = {
                s = s,
                row = { kind = "command", key = command.key, title = command.title, subtitle = command.subtitle },
            }
        end
    end
    sorted(commands)

    local rows = {}
    local function append(header, scored)
        if #scored == 0 then
            return
        end
        rows[#rows + 1] = { kind = "header", title = header }
        for _, entry in ipairs(scored) do
            rows[#rows + 1] = entry.row
        end
    end
    append("APPLICATIONS", apps)
    append("WINDOWS", windows)
    append("COMMANDS", commands)

    return rows
end

function search.run(key)
    for _, command in ipairs(COMMANDS) do
        if command.key == key then
            command.run()
            return
        end
    end
end

-- Whether the panel is open, Lua's own copy -- shell/Search.qml
-- holds the one the panel actually draws from, and tells this one apart
-- only through `search.closed` (below) and the next key press, never read
-- back from the scene itself (docs/shell-boundary.md: a hosted scene's own
-- runtime property values are not something Lua can read, only what it last
-- declared).
local open = false

-- Writes `config.shell.properties` and redeclares the surface.
local function push(now_open, results)
    local config = require("config")
    config.shell = type(config.shell) == "table" and config.shell or {}
    config.shell.properties = type(config.shell.properties) == "table" and config.shell.properties or {}
    config.shell.properties.searchOpen = now_open
    config.shell.properties.searchResults = results
    require("shell").apply()
end

function search.apply()
    open = false
    sol.bind(search.key, function()
        open = not open
        push(open, open and search.match("") or {})
    end)
end

-- shell/Search.qml sends these three outside the action vocabulary
-- (`lua/actions.lua`'s own comment: anything else "is left to whoever
-- listens for the surface by name", the way `tweaks.lua` listens for its
-- own). `apps.launch` and `windows.focus` never come through here -- the
-- panel sends those straight, so activating a row costs no extra dispatch.
sol.on("surface", function(_, action, data)
    if action == "search.query" then
        local text = type(data) == "table" and data.text or ""
        push(open, search.match(text))
    elseif action == "search.closed" then
        open = false
        push(false, {})
    elseif action == "search.run" then
        local key = type(data) == "table" and data.key or nil
        if key then
            search.run(key)
        end
    end
end)

search.apply()

return search
