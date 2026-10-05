-- Quick search (04-ui.md §4.7): apps, open windows and a short list of
-- compositor commands, matched and ranked here rather than in
-- `qml/preview/Search.qml` -- see that file's own module doc for why. It
-- sends what is typed with `Solium.send("search.query", { text = ... })`;
-- this file answers with `search.match`, and the ranked rows -- including
-- the section headers the panel draws them under -- go back onto the
-- shell's root item as a property, the same round trip
-- `preview/dock.lua` already uses for its own pins
-- (`shell.lua`'s generic `properties` passthrough).
--
-- **Ordering, at load.** `search.apply()` only ever *writes*
-- `config.shell.properties` -- it never calls `shell.apply()` itself, the
-- same restraint `preview/dock.lua`'s module doc explains for the same
-- reason: `preview.init.lua` runs before `require("shell")` in `init.lua`,
-- so calling it from here would declare the surface on a `config.shell`
-- `preview.init.lua` has not finished filling in yet. Every *later* call --
-- a key press, a query, a close -- runs long after startup, once
-- `require("shell")` has already cached the real module, so `push` below
-- calling it there is safe.
--
-- **What is cut for this first version**, and why (04-ui.md's own Showcase/
-- P1 marks, plus what "small" asked for beyond them): the detail pane, a
-- row's own actions (Tab), Alt+digits, Ctrl+Return, the settings and
-- desktop-file sources, and the container transform from the bar's own
-- search button -- the bar has no search button yet either. The
-- empty-query suggestions ("never blank") are cut the same way
-- `preview/dock.lua`'s own first-run pins are: `sol.store` is not here yet,
-- so there is no usage to rank by, and faking "recent" with nothing real
-- behind it would be worse than an honestly empty panel. A command row also
-- carries no `icon` (unlike an app or window row), so `Search.qml`'s own
-- delegate draws it with no leading glyph -- 04-ui.md's own mockup shows
-- only a generic glyph there too, but this file has no generic icon name of
-- its own to hand through the provider, so it is left off rather than
-- invented.
--
-- **The layout-correction pass ("us,ru correction with no table") is not
-- here.** 04-ui.md's own words are the tell: it wants this done "because
-- Solium holds the keymap" -- the compositor's own xkb state, mapping a
-- physical key through *another* layout's keysyms, which is exactly what
-- neither `sol.*` nor the QML `Keyboard` singleton exposes (`Keyboard` says
-- which layout is live, not how a key reads under one that is not).
-- Hand-writing a Latin/Cyrillic character table here would be the one
-- thing that quote says this should not need, so it is left for later,
-- against a real keymap accessor -- a genuine platform gap, not a corner
-- cut for time. `search.match` still matches a genuinely Cyrillic title
-- against a Cyrillic query (plain substring matching does not care which
-- alphabet it is), as `tests/scenarios/preview-search-match.lua` checks;
-- only the *correction* -- reading a mistyped query in the other layout --
-- is missing.
--
-- **Keywords are not matched for apps.** `Apps`, the QML model, carries
-- each app's `keywords` (docs/shell-boundary.md); `sol.apps()`, the plain
-- array this file actually iterates, does not -- `AppInfo` in
-- `script.rs` has no such field. That is the same "QML model built for
-- live delegates, not a script-plain array" gap this file's own module doc
-- opens with, just on the data rather than the iteration, and it is a
-- genuine generic-API gap rather than a reason to reach into the QML side
-- from here: matching is on an app's name and generic name only.

local config = require("config")

local search = {}

-- `workspaces.lua`, lazily: `require`d from here at the top level, this file
-- would load it -- and run its own top-level `actions.override("workspaces.go",
-- ...)`/`actions.override("windows.send", ...)` calls -- the moment
-- `preview.init.lua` requires this module, which is long before `init.lua`
-- gets to `require("actions")`. `actions.override` needs `actions.lua`'s own
-- `sol.on("surface", actions.run)` already registered to mean anything
-- (`init.lua`'s own comment on why it requires `actions` before
-- `workspaces`), so that would route `workspaces.go` and `windows.send`
-- around `sol.act` entirely -- measured directly: it is exactly what broke
-- `a_workspaces_go_from_a_scene_switches_the_monitor_it_names` and the two
-- `windows.send` tests beside it the first time this file required
-- `workspaces` at its own top level. Every call below runs long after
-- startup (a key press, at the earliest), by which point `require` only
-- ever returns the cached module, so asking for it here costs nothing.
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
-- over a plain array wants (`qml/preview/Search.qml`'s own `rows`, the same
-- `QVariantList` convention `preview/dock.lua`'s module doc already
-- covers). Only meaningfully called once a real snapshot exists -- like
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

-- Whether the panel is open, Lua's own copy -- `qml/preview/Search.qml`
-- holds the one the panel actually draws from, and tells this one apart
-- only through `search.closed` (below) and the next key press, never read
-- back from the scene itself (docs/shell-boundary.md: a hosted scene's own
-- runtime property values are not something Lua can read, only what it last
-- declared).
local open = false

-- Writes `config.shell.properties` and redeclares the surface -- safe here,
-- unlike inside `search.apply()`; see this file's own module doc.
local function push(now_open, results)
    config.shell = type(config.shell) == "table" and config.shell or {}
    config.shell.properties = type(config.shell.properties) == "table" and config.shell.properties or {}
    config.shell.properties.searchOpen = now_open
    config.shell.properties.searchResults = results
    require("shell").apply()
end

local function settings()
    local preview = config.preview
    if type(preview) == "table" and type(preview.search) == "table" then
        return preview.search
    end
    return {}
end

function search.apply()
    open = false
    config.shell = type(config.shell) == "table" and config.shell or {}
    config.shell.properties = type(config.shell.properties) == "table" and config.shell.properties or {}
    config.shell.properties.searchOpen = false
    config.shell.properties.searchResults = {}

    sol.bind(settings().key or "super+d", function()
        open = not open
        push(open, open and search.match("") or {})
    end)
end

-- `qml/preview/Search.qml` sends these three outside the action vocabulary
-- (`lua/actions.lua`'s own comment: anything else "is left to whoever
-- listens for the surface by name", the way `tweaks.lua` listens for its
-- own). `apps.launch` and `windows.focus` never come through here -- the
-- panel sends those straight, the same way `qml/preview/DockIcon.qml`
-- already does, so activating a row costs no extra dispatch.
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
