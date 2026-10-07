-- Solium's default configuration.
--
-- Copy to ~/.config/solium/init.lua to change it; the compositor prefers that
-- file when it exists. Modes live in their own scripts and register their own
-- bindings, so adding one is a `require` and removing one is deleting a line.

local config = require("config")

-- Which scene graph QML renders on: the one setting a reload does not change,
-- because it is read once, before Qt starts. See `config.lua` and
-- `the_renderer_is_decided_once_per_process`.
sol.qml(config.qml)
-- Whether systemd and D-Bus are told about this session, read by the backend
-- when the session starts. See `config.lua`.
sol.session(config.session)

-- Settings the compositor itself holds, applied from the same file as
-- everything else. All of them take effect immediately when reloaded.
sol.pane(config.pane)
sol.loading(config.loading)
-- What fills a window while a resize drag is ahead of its client. An absent
-- table is not an error: a `config.lua` copied before this setting existed
-- keeps the default rather than failing the whole configuration.
sol.resize(config.resize)
-- What a fullscreen window covers: the bars, or nothing. See `config.lua`.
sol.fullscreen(config.fullscreen)
-- Where the monitors go. Applied on reload too, so moving a screen in the
-- configuration is `super+shift+r` rather than logging out.
sol.keyboard(config.keyboard)
sol.monitors(config.monitors)
-- When the screens go dark on their own. An absent table keeps the default,
-- for the reason `sol.resize` gives.
sol.idle(config.idle)
-- libinput device settings: tap-to-click, scrolling, acceleration and the
-- rest, by device type and by device, applied now and again on every
-- reload. See `config.lua`.
sol.input(config.input)
-- The pointer's theme and size. An empty table here is not "reset it": it
-- means the configuration says nothing, and `XCURSOR_THEME`/`XCURSOR_SIZE`
-- are what the pointer follows -- which is what the rest of the machine
-- already does. See `config.lua`.
sol.cursor_theme(config.cursor)

-- Only reachable when the compositor was started with --debug-mode, but the
-- entries are declared either way: what costs nothing to declare should not
-- need a conditional.
-- The wallpaper is a script like any other mode: `sol.surface` and a QML
-- file, and nothing in the compositor knows what a wallpaper is.
require("wallpaper")
-- The preview shell's default: fills in `config.shell.scene` when nothing
-- else named one. Before `shell`, which reads that setting once, here.
-- `preview.init`, not `preview` -- `package.path` has no `?/init.lua`
-- pattern, only `?.lua`, and Lua's own `require` turns the dot into the
-- directory separator before that template is tried.
require("preview.init")
require("shell")
-- The keyboard pill near the text field: `keyboard.indicator` in config.lua.
-- Configuration on the data the compositor publishes, like the two above.
require("keyboard_indicator")
-- What a hosted scene sends with `Solium.send`: the vocabulary's actions go
-- to `sol.act`, or to the file that answers them in Lua. See
-- `windows_focus_from_a_scene_focuses_the_window`. Keep it before
-- `workspaces`, which answers `workspaces.go` and `windows.send` only when
-- this file is already loaded. See
-- `a_workspaces_go_from_a_scene_switches_the_monitor_it_names`.
require("actions")

require("tweaks")

require("modes")
require("open")
-- How a window goes fullscreen or maximised, and back: `fullscreen` and
-- `maximize` in config.lua.
require("fullscreen")
require("overview")
require("workspaces")
require("tiling")
require("scrolling")
-- Focus and move by direction, in whichever layout is in charge (#150).
-- `every_shipped_binding_is_reachable_on_us` loads this file and asks for its
-- keys.
require("direction")

-- Programs. `sol.spawn` starts them as clients of this compositor, whatever
-- session the compositor itself happens to be nested in.

-- Split on spaces, because a terminal often needs arguments to open a *new*
-- window: KDE's konsole hands off to an already-running instance and exits
-- unless told `--separate --nofork`, which looks exactly like the spawn having
-- failed.
local function words(text)
    local parts = {}
    for word in string.gmatch(text, "%S+") do
        parts[#parts + 1] = word
    end
    return parts
end

-- `SOLIUM_TERMINAL` wins; otherwise the first of these that is actually
-- installed. A compositor cannot assume any particular terminal exists, and
-- picking one that does not is indistinguishable, from the keyboard, from the
-- binding being broken -- which is how the first hardware session went.
local CANDIDATES = {
    -- Plain Wayland terminals first: they open immediately. A KDE terminal
    -- waits on portal and session services that are not running under Solium
    -- and takes about twenty seconds to show a window, which reads as the
    -- binding being broken rather than as the app being slow -- so konsole is
    -- a fallback, not a preference, even on a KDE machine.
    "kitty",
    "alacritty",
    "wezterm",
    "foot",
    "konsole --separate --nofork",
    "xterm",
}

local function first_installed(candidates)
    for _, candidate in ipairs(candidates) do
        local parts = words(candidate)
        if sol.which(parts[1]) then
            return parts
        end
    end
    return nil
end

local TERMINAL = words(os.getenv("SOLIUM_TERMINAL") or "")
if #TERMINAL == 0 then
    TERMINAL = first_installed(CANDIDATES)
end
if TERMINAL then
    sol.log("terminal: " .. table.concat(TERMINAL, " "))
else
    sol.log("no terminal found -- set SOLIUM_TERMINAL to one you have")
end

local function open_terminal()
    if not TERMINAL then
        sol.log("no terminal is installed -- set SOLIUM_TERMINAL")
        return
    end
    sol.spawn(table.unpack(TERMINAL))
end

-- Both Enters. The keypad one is a different keysym (`kp_enter`), so binding
-- only `return` leaves whoever reaches for the near one pressing a key that
-- does nothing -- and nothing on screen says why. Bound separately rather than
-- folded together in the compositor, so a script can still tell them apart.
sol.bind("super+return", open_terminal)
sol.bind("super+kp_enter", open_terminal)

-- Close the focused window. A request, not a kill -- the client decides whether
-- it can go.
sol.bind("super+q", function()
    for _, window in ipairs(sol.windows()) do
        if window.focused then
            sol.close(window.id)
            return
        end
    end
end)

-- Ending the session. Ctrl+Alt+Backspace does this too and cannot be rebound,
-- because the way out has to work even when this file does not.
sol.bind("super+shift+q", function()
    sol.quit()
end)

-- The laptop keys (#151): plain bindings to the usual programs, the way any
-- compositor's default configuration has them. A key whose program is not
-- installed does nothing but log; `config.bindings` takes any of them over,
-- and `false` there unbinds one. Volume through `wpctl` (PipeWire), capped at
-- 100%, with shift for a 1% step; brightness through `brightnessctl`; media
-- through `playerctl`; Print through `grim`, shift+Print a region with
-- `slurp`, into ~/Pictures/Screenshots and onto the clipboard when `wl-copy`
-- is there. Held keys do not repeat yet: a binding fires once per press.
-- `script::shipped::every_shipped_binding_is_reachable_on_us`.
local function run(...)
    local argv = { ... }
    return function()
        sol.spawn(table.unpack(argv))
    end
end
local SINK, SOURCE = "@DEFAULT_AUDIO_SINK@", "@DEFAULT_AUDIO_SOURCE@"
sol.bind("XF86AudioRaiseVolume", run("wpctl", "set-volume", "-l", "1.0", SINK, "5%+"), "laptop keys")
sol.bind("XF86AudioLowerVolume", run("wpctl", "set-volume", SINK, "5%-"), "laptop keys")
sol.bind("shift+XF86AudioRaiseVolume", run("wpctl", "set-volume", "-l", "1.0", SINK, "1%+"), "laptop keys")
sol.bind("shift+XF86AudioLowerVolume", run("wpctl", "set-volume", SINK, "1%-"), "laptop keys")
sol.bind("XF86AudioMute", run("wpctl", "set-mute", SINK, "toggle"), "laptop keys")
sol.bind("XF86AudioMicMute", run("wpctl", "set-mute", SOURCE, "toggle"), "laptop keys")
sol.bind("XF86MonBrightnessUp", run("brightnessctl", "set", "5%+"), "laptop keys")
sol.bind("XF86MonBrightnessDown", run("brightnessctl", "set", "5%-"), "laptop keys")
sol.bind("XF86AudioPlay", run("playerctl", "play-pause"), "laptop keys")
sol.bind("XF86AudioPause", run("playerctl", "play-pause"), "laptop keys")
sol.bind("XF86AudioNext", run("playerctl", "next"), "laptop keys")
sol.bind("XF86AudioPrev", run("playerctl", "previous"), "laptop keys")
-- One shell line each, so the file name and the clipboard step stay in one
-- place a user can read and copy; the name sorts by time.
local SHOT = 'dir="${XDG_PICTURES_DIR:-$HOME/Pictures}/Screenshots"; mkdir -p "$dir"; '
    .. 'file="$dir/$(date +%Y-%m-%d_%H-%M-%S).png"; '
local CLIP = ' && { command -v wl-copy >/dev/null && wl-copy < "$file" || true; }'
sol.bind("Print", run("sh", "-c", SHOT .. 'grim "$file"' .. CLIP), "laptop keys")
sol.bind("shift+Print", run("sh", "-c", SHOT .. 'area="$(slurp)" && grim -g "$area" "$file"' .. CLIP), "laptop keys")

sol.log("solium configuration loaded")

-- Show or hide the Developer Tweaks panel. Nothing without --debug-mode.
sol.bind("super+shift+d", function()
    require("tweaks").toggle()
end)

-- Cycle the keyboard layout, when the configuration lists more than one.
--
-- xkb can do this by itself -- `options = "grp:alt_shift_toggle"` in
-- `config.keyboard` -- and that keeps working. This exists because it is
-- discoverable: it appears in `solium --check` beside every other binding,
-- and it does not require knowing that xkb has an option called `grp`.
--
-- Silent with one layout rather than an error, because that is the common
-- case and a binding that complains about the configuration every time it is
-- pressed is worse than one that does nothing.
sol.bind("super+shift+k", function()
    local keyboard = sol.keyboard()
    if #keyboard.layouts < 2 then
        return
    end
    local next_layout = keyboard.active % #keyboard.layouts + 1
    sol.keyboard({ active = next_layout })
    sol.log("keyboard layout: " .. keyboard.layouts[next_layout])
end)

-- Read this file again, without ending the session. Edit anything -- a
-- binding, a gap, a decoration, a whole layout mode -- and press it.
sol.bind("super+shift+r", function()
    sol.reload()
end)

-- Dev: the genie. `deform` names an effect from `crates/effects` -- there is
-- one so far -- and `to` is what the window is pulled into. A rect here
-- because there is no dock yet; with one, `to = { window = id }` would follow
-- its icon as the icon moves, which a rect read out of this table cannot.
--
-- `axis` is which edge leads, so a dock down the side of the screen is
-- `"left"` and the compositor turns the mesh to match. `spread` is how much of
-- the window is in motion at once: 0 pulls it in rigidly, larger values draw
-- the tail out behind it. Composes with a transform -- add `rotate_y` here and
-- the window tilts while it is sucked in.
sol.bind("super+m", function()
    local area = sol.monitor()
    for _, window in ipairs(sol.windows()) do
        if window.focused then
            sol.animate({ duration = 520, easing = "inOutCubic" })
            sol.present(window.id, {
                deform = {
                    effect = "genie",
                    axis = "down",
                    spread = 1.4,
                    to = {
                        x = area.x + area.w / 2 - 60,
                        y = area.y + area.h - 24,
                        w = 120,
                        h = 24,
                    },
                },
            })
            return
        end
    end
end)

-- A tilted window, to see the compositor draw one as geometry rather than as
-- a rectangle. `rotate_y` turns it about its own vertical axis and
-- `perspective` is the viewer distance in pixels, which is what makes the far
-- edge recede instead of merely narrowing.
sol.bind("super+g", function()
    for _, window in ipairs(sol.windows()) do
        if window.focused then
            sol.animate({ duration = 260, easing = "outCubic" })
            sol.present(window.id, { rotate_y = 35, perspective = 900 })
            return
        end
    end
end)

-- The configuration's own bindings, last.
--
-- Last on purpose, not merely at the end. `sol.bind` lets the later call win,
-- so reading `config.bindings` after every shipped binding -- this file's and
-- each mode script's -- is the whole mechanism by which a user's `super+q`
-- takes over from the one above instead of being quietly overwritten by it.
-- Moved up, this would still work for combinations nobody else uses, which is
-- the worst way for it to break: fine until somebody rebinds a key that
-- matters. Keep it last in your own `init.lua` too.
require("bindings")
