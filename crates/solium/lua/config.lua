-- Every setting the configuration can reach, in one place.
--
-- Values were scattered across the layout scripts as local constants, which is
-- hardcoding written in a scripting language. A setting nobody can find is not
-- a setting. Nothing here needs the compositor rebuilt.
--
-- Don't edit this file: an install replaces it, and it is also the defaults.
-- Write only what you want changed in ~/.config/solium/user.lua
-- ($XDG_CONFIG_HOME/solium/user.lua when that is set) and it is merged over
-- these:
--
--     return {
--         gap = 4,
--         pane = "reactive",
--         tiling = { split = 0.618 },
--     }
--
-- Nested tables merge key by key, so `tiling = { split = ... }` keeps the
-- animations below it. Lists are replaced whole, because a list of widths with
-- one entry changed is a different list, not a longer one.
--
-- Not here yet, because nothing reads them from this file
-- ([#159](https://github.com/Lilium-Linux/solium/issues/159)):
--
--   * The key held to drag a window: SOLIUM_DRAG_MODIFIER. Natural
--     scrolling moved out of this list in #157: it is `input`'s
--     per-device-type `natural_scroll` below, which is what
--     SOLIUM_FORM_FACTOR's own guess ("a laptop has a touchpad") was really
--     standing in for. Whether focus follows the pointer moved into `focus`
--     below with #219, which needed it per mode.
--   * The terminal `super+return` opens: SOLIUM_TERMINAL, or bind
--     `super+return` yourself in `bindings` below.
--   * The overview's animations, fixed in `lua/overview.lua` at 260 ms in and
--     200 ms out.
--   * How long a closing window takes to fade, fixed in the compositor at
--     190 ms.

local defaults = {
    -- Desktop touch policy, implemented by touch.lua. Each action is optional;
    -- a mobile configuration may turn them all off or omit require("touch").
    touch = { focus = true, move = true, resize = true, click = true },

    -- Space between windows and around the work area, in logical pixels.
    gap = 12,

    -- The wallpaper.
    --
    -- The one that ships is the default; a path replaces it, and `false`
    -- turns it off. Turn it off if you run `swaybg`, `hyprpaper` or a shell
    -- that draws its own -- a layer surface on the background layer is drawn
    -- over this one, so leaving both on means paying for a picture nobody
    -- sees.
    --
    --     wallpaper = "~/Pictures/whatever.png",
    --     wallpaper = false,
    --
    -- A *list* gives each workspace its own background, and it travels with
    -- that workspace: `workspaces.lua` puts it in the same selection as the
    -- desk's windows, so one animation carries both and the wallpaper stops
    -- being left behind when you switch.
    --
    --     wallpaper = { "~/Pictures/one.png", "~/Pictures/two.png" },
    --
    -- Fewer pictures than workspaces cycles. It costs a scene per desk on
    -- every monitor, built when it is declared, and one screen-sized
    -- rasterisation per desk you have actually visited, per monitor, which is
    -- why a single image stays a single static surface: every desk sharing one
    -- picture makes a wallpaper that slides pixel-identical to one that does
    -- not, so it would be memory spent on nothing to look at.
    --
    -- `~` is expanded. The image is cropped to fill the screen rather than
    -- fitted, so nothing is letterboxed.
    --
    -- What actually draws it is `lua/wallpaper.lua` and `qml/wallpaper.qml`,
    -- and that is the more interesting part: there is no wallpaper code in the
    -- compositor at all. It is a short script around one `sol.surface` call,
    -- which draws any QML scene at any layer on any monitor -- so a bar, a
    -- dock or a heads-up display is the same call with a different layer. Copy
    -- `qml/wallpaper.qml` to ~/.config/solium/qml/ and the background can be a
    -- gradient, a shader or a clock. `super+shift+r` reloads it. A copy that
    -- still shows `source` needs the picture named by its path here: the
    -- shipped one is found beside the shipped QML, not beside yours.
    wallpaper = "solium",

    -- A shell: the bar, the dock, the launcher.
    --
    -- A shell is a project of its own, with its own repository, and it runs
    -- here as your configuration: its QML is hosted in the compositor's own
    -- engine, beside the window frames. Name its root file:
    --
    --     shell = { scene = "~/.config/solium/shell/shell.qml" },
    --
    -- `false`, the default, hosts none. `~` is expanded. It is drawn once on
    -- every monitor, over the windows, across the whole monitor, and each
    -- instance reads its own monitor as `Solium.monitor`. `on = "primary"`,
    -- or a connector name, draws one instead. It takes the pointer only
    -- where its items take input, and the windows under it get the rest
    -- (`a_press_where_the_shell_draws_nothing_reaches_the_window_under_it`).
    -- See `the_shell_scene_is_read_from_the_configuration` and
    -- `the_shell_is_on_every_monitor_unless_the_configuration_names_one`.
    -- SOLIUM_SHELL_SCENE overrides this, because that is set per run.
    -- `super+shift+r` picks up a change. See docs/shell-boundary.md for
    -- installing one and for what the compositor provides to a shell it
    -- hosts.
    --
    -- Its bar keeps the windows out of its strip by saying so in the scene,
    -- `Solium.surface.reserve.bottom: 48`, and quick settings growing out
    -- of it move no window. See `a_panel_growing_out_of_the_bar_moves_no_window`.
    --
    -- `outside_click`: what a press outside an open popup of the shell does,
    -- after closing it: "swallow" (macOS and iOS do this), or "pass" to also
    -- click what is under it. A table names popups by their grab's name:
    --
    --     outside_click = { default = "swallow", ["tray-menu"] = "pass" },
    --
    -- See `the_shell_takes_its_outside_click_from_the_configuration` and
    -- `a_press_outside_a_grab_dismisses_it_and_is_swallowed_by_default`.
    --
    -- What it asks for, `Solium.send("windows.focus", { id: ... })` in its
    -- QML, reaches `sol.on("surface", ...)`, and `lua/actions.lua` has the
    -- compositor do the window actions. See
    -- `windows_focus_from_a_scene_focuses_the_window`.
    --
    -- `keyboard.bindings`: while the shell holds the keyboard (a search
    -- field, a password), which compositor bindings still work:
    -- "except_claimed" (the default; every binding but the keys the field
    -- claims), "all", or "none" (every key but the Ctrl+Alt escapes goes to
    -- the shell). See `the_shell_takes_its_keyboard_bindings_from_the_configuration`
    -- and `a_claimed_key_reaches_the_scene_and_not_its_binding`.
    --
    -- A layer-shell bar (Waybar and the like) needs nothing here: it is an
    -- ordinary client, and you start it as one.
    shell = {
        scene = false,
        on = "every-monitor",
        outside_click = "swallow",
        keyboard = { bindings = "except_claimed" },
    },

    -- The compositor's own preview shell: a bar, built as configuration on
    -- the same public API `shell` above describes (`lua/preview/`,
    -- `qml/preview/`; see docs/ricing.md). On by default, so a fresh install
    -- shows live workspaces, windows, the keyboard layout and a clock rather
    -- than a blank desktop.
    --
    --     preview = false,
    --
    -- Turns it off. Naming your own `shell.scene` above, or `user.lua`'s own
    -- shell, always wins over it either way -- this only ever fills in a
    -- default nothing else set.
    --
    -- A table instead of `true` reaches the top dock
    -- (`lua/preview/dock.lua`, 04-ui.md §4.6): `preview = { dock = {
    -- pinned = { "org.mozilla.firefox", "kitty" }, visibility = "always",
    -- icon_size = 48 } }`. `pinned` is a list of desktop ids; left unset,
    -- the dock picks the first installed terminal, file manager and browser
    -- itself. `visibility` is `"always"` or `"autohide"` (the default).
    --
    -- Quick search (`lua/preview/search.lua`, 04-ui.md §4.7) opens on
    -- `super+d`: `preview = { search = { key = "super+alt+space" } }` binds
    -- a different one instead. It searches installed applications, open
    -- windows and a few compositor commands; see `docs/ricing.md` for what
    -- is cut from this first version.
    --
    -- Desktop icons (`lua/preview/desktop.lua`, 04-ui.md §4.9) show
    -- `Solium.dirs.desktop` (`$XDG_DESKTOP_DIR`, or `user-dirs.dirs`; empty
    -- draws nothing): `preview = { desktop = { icons = true, from =
    -- "top-right", cell = { width = 96, height = 100 }, labels = 2, open =
    -- "double", show_hidden = false, reveal_on_click = false } }`. `icons =
    -- false` turns them off; `from` is `"top-right"` (the default) or
    -- `"top-left"`; `open` is `"double"` (the default) or `"single"`. See
    -- `docs/ricing.md` for what is cut from this first version (dragging
    -- and saved positions, right-click menus, showing the desktop, Open
    -- With, thumbnails).
    preview = true,

    -- The keyboard: its xkb layouts (`layout`, `variant`, `options`, `model`,
    -- `rules`, `active`), the locks (`caps`, `num`), the repeat rate, and the
    -- keyboard pill.
    --
    -- No xkb name is set here, and that means "whatever the session already
    -- said". Every name below is an xkb name, and leaving one out makes
    -- xkbcommon fall back to the matching `XKB_DEFAULT_*` environment
    -- variable -- which is where a display manager or a `~/.profile` usually
    -- puts it, and which worked before any of this existed. Naming one here
    -- takes precedence.
    --
    --     keyboard = {
    --         layout  = "us,ua",
    --         variant = ",",
    --         options = "grp:alt_shift_toggle,compose:ralt",
    --         model   = "pc105",
    --     },
    --
    --   layout    a comma-separated list. The first is the one you start in;
    --            the log line `keyboard` says what the session ended up with.
    --   variant   one per layout, in the same order, and blank for "plain".
    --            "us,ua" with ",dvorak" is US ordinary and Ukrainian Dvorak.
    --   options   xkb options, comma separated. `grp:` ones switch layout and
    --            are handled inside the keymap, so they work without the
    --            compositor being involved. `compose:ralt` gives a compose
    --            key, which is the nearest thing to an input method until
    --            #26 lands.
    --   model     rarely worth setting; "pc105" is assumed by the rules.
    --   rules     the xkb rules, rarer still. Blank, like the others, falls
    --            back to the environment.
    --   active    which of the layouts is live, counting from 1. A number past
    --            the last layout is warned about and changes nothing.
    --   caps      Caps Lock on or off, and `num` the same for Num Lock: `num =
    --            true` here turns Num Lock on when the session starts and
    --            again at every reload. Set by pressing the keymap's own key
    --            inside the compositor, so the layout stays where it was.
    --
    -- The repeat rate is the compositor's own, not xkb's, so no environment
    -- variable reaches it and this is the only place it can be set:
    --
    --   repeat_rate    repeats per second after the delay. 25 is the default.
    --   repeat_delay   milliseconds held before repeating starts. 200.
    --
    -- `super+shift+k` cycles the layouts, and it is one call of the same
    -- function, so a binding of your own can switch them the same way:
    --
    --     sol.bind("super+shift+k", function()
    --         local kb = sol.keyboard()
    --         sol.keyboard{ active = kb.active % #kb.layouts + 1 }
    --     end)
    --
    -- `sol.keyboard()` with no argument returns the layout names, which one is
    -- active and its names (`layout_name`, "Russian", and `layout_short`,
    -- "RU"), whether Caps Lock and Num Lock are on, and the repeat settings.
    -- `sol.on("keyboard", function(state, changed) end)` hears the layout or a
    -- lock change, however it changed, and a hosted scene or a pane style
    -- reads the same in QML as the `Keyboard` singleton.
    keyboard = {
        -- The keyboard pill: a small capsule near where you are typing that
        -- says Caps Lock is on (`⇪`) or which layout you just switched to
        -- (`EN`, `RU`), after the one macOS shows. It is configuration and
        -- not a compositor feature -- `lua/keyboard_indicator.lua` is its
        -- policy and reads this table; the QML draws it from the data the
        -- compositor publishes, in Theme's accent -- so it is the example to
        -- copy for anything of your own that reacts to the keyboard or the
        -- caret. Delete `require("keyboard_indicator")` from `init.lua` and
        -- it is gone. See docs/ricing.md, "The keyboard pill".
        indicator = {
            -- Where it is drawn:
            --   "pane"     inside the window, just below the text field's
            --              caret, or above it when there is no room. Each
            --              shipped pane style draws it with one line,
            --              `KeyboardPillLayer {}`, and moves, scales and fades
            --              it with the window. A style of your own without
            --              that line draws none. A window drawn with no frame
            --              -- fullscreen, one drawing its own decorations, or
            --              any window under `pane = "none"` -- gets the
            --              "surface" pill at its caret instead.
            --   "surface"  on a surface of its own over everything, at the
            --              caret on screen, moved after it as the
            --              application says its caret has moved.
            --   false      nowhere; `indicator = false` says the same.
            -- The caret is known only from applications that say where it
            -- is, through the `text-input-v3` protocol; for the rest, see
            -- `fallback`.
            show = "pane",
            -- Which changes show it: switching layout, Caps Lock, Num Lock.
            -- A lock's pill stays while the lock is on and goes the moment it
            -- is off; a layout's goes after `duration`, and hands back to the
            -- lock's if one is on.
            on = { layout = true, caps = true, num = false },
            -- Show Caps Lock's pill again when a text field is focused while
            -- Caps Lock is on, as macOS does. Only while `on.caps` is true.
            caps_on_focus = true,
            -- With no caret to go to -- an application that says nothing about
            -- one, or no text field focused -- "surface" draws it on the
            -- focused window's monitor, and false draws nothing.
            fallback = "surface",
            -- Where on the monitor that is: "bottom", "center" or "top",
            -- centred across it.
            position = "bottom",
            -- How long a layout's pill stays, in milliseconds.
            duration = 1200,
        },
    },

    -- The keys, and what they do.
    --
    -- The shipped bindings are `sol.bind` calls in `init.lua`, and `init.lua`
    -- is the session's entry point -- so until #117 the only way to add one
    -- key was to write your own copy of that file and adopt two hundred lines
    -- of layout wiring, mode setup and terminal detection along with it, or to
    -- edit a file the next update overwrites. Neither of those is a setting.
    -- This table is: it is merged over these defaults exactly as `tiling` and
    -- `scrolling` are, so a `user.lua` holding nothing but a binding is a
    -- complete configuration.
    --
    --     bindings = {
    --         ["super+b"]       = "firefox",
    --         ["super+shift+s"] = { "sh", "-c", "grim -g \"$(slurp)\"" },
    --         ["super+n"]       = function() sol.spawn("kitty", "-e", "nvim") end,
    --         ["super+g"]       = false,
    --     },
    --
    -- Four forms, each the shortest way to write what it means:
    --
    --   a string    a command line, split on spaces. The common case is a
    --               program and no arguments, and that should read like one.
    --   a list      the same, already split, for an argument with a space in
    --               it -- the shell quoting above is one argument, not three.
    --   a function  anything else. The whole `sol` API is in scope, so this is
    --               a binding in exactly the sense `init.lua`'s are.
    --   false       unbind. A shipped binding you do not want is otherwise
    --               unremovable, and binding it to a function that does
    --               nothing is not the same thing: the compositor would still
    --               swallow the key.
    --
    -- A list can also say how its key behaves, in named fields beside the
    -- command (#224):
    --
    --     ["XF86AudioRaiseVolume"] = { "pamixer", "-i", "5", repeating = true, locked = true },
    --
    --   repeating   while the key is held, run it again at `keyboard`'s
    --               repeat delay and rate, as the shipped volume and
    --               brightness keys do.
    --   locked      fire at the lock screen as well, as those and the media
    --               keys do. There it may run programs, but nothing it asks of
    --               a window or a workspace, or to end the session, happens.
    --               At Solium's own lock screen, a key that types a character
    --               goes into the password instead, whatever is bound to it.
    --
    -- A binding here replaces the shipped one whole, what it asked for
    -- included: a volume key rebound as a string does not repeat.
    --
    -- The combination is written the way you would say it and normalised by
    -- the compositor, so `super+shift+q` and `Shift+Super+Q` are one binding.
    --
    -- **A binding here replaces a shipped one on the same combination, rather
    -- than being an error.** Two reasons, the second deciding. The first is
    -- consistency: `sol.bind` has always let the later call win, so a clash
    -- refusing *here* would make this table alone behave unlike the primitive
    -- underneath it.
    -- The second is that refusing would permit adding a binding and forbid
    -- changing one, and changing one is what people come here to do: `super+q`
    -- closing a window is a choice, not a law, and "you may not rebind it"
    -- leaves editing `init.lua` as the only way -- which is the fault this
    -- section exists to remove.
    --
    -- Replacing is not the same as replacing quietly. `solium --check` prints
    -- every binding this table produced, marks the ones that took a shipped
    -- combination over, and lists the ones it removed, so a clash you did not
    -- intend costs a line of output rather than an afternoon wondering why a
    -- key stopped working.
    --
    -- Applied by `lua/bindings.lua`, from the last line of `init.lua`, because
    -- that ordering *is* the replacement -- "the later call wins" only puts
    -- these last if they are read last. In your own `init.lua`, keep
    -- `require("bindings")` last for the same reason.
    --
    -- One thing this cannot check: a combination is whatever you press, so
    -- there is no list of valid names to compare yours against. `super+whoops`
    -- binds successfully and never fires. `--check` prints what was bound,
    -- which is the only honest answer available -- read it and look for the
    -- key you meant. The documentation's Key bindings page lists every
    -- binding that ships, and what each does.
    --
    -- `super+return` opens SOLIUM_TERMINAL, or else the first terminal that
    -- is installed. There is no setting for it yet (#159), so to choose one
    -- here, bind the key: `["super+return"] = "foot"`.
    --
    -- The keys that get around a desktop by direction (#150) are replaced here
    -- like any other. They ship in `direction.lua` and `modes.lua`:
    --
    --   super+arrows, super+h/j/k/l          focus the window that way
    --   super+shift+arrows,                  move it that way, and at a
    --     super+shift+h/j/l, super+alt+k     screen's edge onto the next one
    --   super+f                              fullscreen, and back
    --   super+shift+m                        maximised, and back
    --   super+shift+space                    float over the layout, and back
    --   super+m                              minimize (focusing it brings it
    --                                        back; see `minimize` below)
    --
    -- Up on k is super+alt+k because super+shift+k cycles the keyboard layout
    -- (`init.lua`). What a direction does is the layout's: see `tiling.move`
    -- below, and docs/modes.md. Pressed with either half of `us,ru` live:
    -- `the_direction_and_window_keys_fire_while_russian_is_active`.
    --
    -- `super+ctrl+l` locks the session (`init.lua`), as `lock` below says.
    --
    --     bindings = {
    --         ["super+ctrl+h"] = function() sol.focus_direction("left") end,
    --     },
    bindings = {},

    -- The pointer.
    --
    -- Empty means "whatever the session already said", exactly as `keyboard`
    -- above does and for the same reason: `XCURSOR_THEME` and `XCURSOR_SIZE`
    -- are what GTK, Qt and every other toolkit on this machine follow. A
    -- display manager or a `~/.profile` usually sets them, and a compositor
    -- that quietly overrode them would be the one thing on screen drawing a
    -- different pointer from everything else.
    --
    --     cursor = { theme = "Adwaita", size = 24 },
    --     cursor = { scene = "~/.config/solium/cursor/Cursor.qml", size = 32 },
    --
    --   scene   a QML scene to draw the pointer with, configured the way
    --           `shell.scene` is: a path (`~` is expanded), or a name looked
    --           for in ~/.config/solium/qml/ and then the shipped QML. It is
    --           drawn for every shape, ahead of any theme, and reads
    --           `Solium.cursor`: the shape asked for (`default`, `text`,
    --           `pointer`, `ew-resize`...), `pressed`, `velocity`, `scale`
    --           and `size`. It sets `Solium.cursor.hotspot` on its root, and
    --           is as big as its root says, so a glow can reach past `size`.
    --           It may animate. A window that draws its own cursor, or hides
    --           it as a game does, still does over its own surface.
    --           `SOLIUM_QML_CURSOR=<file>` is the scene for one run, over this.
    --           docs/ricing.md has an example.
    --   theme   the name of an XCursor theme -- a directory under ~/.icons,
    --           ~/.local/share/icons or /usr/share/icons. `ls /usr/share/icons`
    --           lists the ones this machine has. Naming one here takes
    --           precedence over `XCURSOR_THEME`.
    --   size    how big the pointer is, in *logical* pixels, between 8 and
    --           256. Multiplied by each monitor's scale, so 24 is 24 pixels on
    --           a 1x screen and 48 device pixels on a 2x one -- which is why a
    --           single number is still right on a desk with monitors at
    --           different scales. Takes precedence over `XCURSOR_SIZE`.
    --           Outside that range it is ignored, with a line in the log, and
    --           the next source has its turn -- not clamped, because a pointer
    --           three pixels across is as hard to find as no pointer at all
    --           and nobody meant to ask for one.
    --
    -- So the order is: what is written here, then the environment, then 24
    -- logical pixels and no theme at all.
    --
    -- **No theme is not a missing pointer.** Solium draws its own from
    -- `qml/cursor.qml`, and that is what you get with no scene and no theme
    -- set here, nothing in the environment, or a theme named that turns out
    -- not to be installed -- the log says which. It is a white arrow with a
    -- dark outline, in fixed colours rather than the theme's, because a
    -- pointer has to read over whatever a client drew, and it is one arrow
    -- for every shape. A scene that would not load is drawn as no scene is,
    -- with a line in the log, and a reload tries it again.
    --
    -- A shape your theme does *not* have is the one case that does not reach
    -- it. Applications name the cursor they want -- an I-beam over text, a
    -- resize arrow on an edge -- and a theme is free to have drawn only some
    -- of them; the missing ones fall back to that theme's own arrow, so a
    -- themed session stays wholly themed rather than mixing two designs.
    --
    -- Applied on reload, so trying a theme or a scene out is `super+shift+r`;
    -- a scene whose files changed is built again then. The compositor call is
    -- `sol.cursor_theme(...)`; `sol.cursor()` is a different function that
    -- answers with where the pointer is.
    cursor = {},

    -- How QML -- the frames, the pointer, the wallpaper -- is rendered.
    --
    --   renderer        "auto" renders on the GPU when a trial render in a
    --                   separate process passes, and in software when it
    --                   fails or times out, or when this build's last GPU
    --                   start in the compositor did not work (see
    --                   dev/README.md); nested, it is software. "gpu"
    --                   skips the trial and "software" never uses the GPU.
    --                   Anything else warns and means "auto". `--qml <mode>`,
    --                   then SOLIUM_QML, then SOLIUM_QML_GPU override this.
    --   probe_timeout   how long the trial may take, in milliseconds, before
    --                   it is killed and QML renders in software.
    --
    -- Read when the compositor starts; a reload does not change it, because
    -- Qt cannot change scene graph once started. The last log line that
    -- begins "QML renderer:" says which one this session got, and why. Tests:
    -- `qml/renderer.rs` and `the_shipped_configuration_asks_for_auto`.
    qml = {
        renderer = "auto",
        probe_timeout = 5000,
    },

    -- The monitors.
    --
    -- Empty means "work it out": every connected screen is driven, left to
    -- right in the order the kernel enumerated the connectors, top edges
    -- aligned. That is right about half the time, and wrong in a way you can
    -- see and fix in one line.
    --
    --     monitors = {
    --         { name = "DP-1", mode = "2560x1440@260", vrr = true, primary = true },
    --         { name = "DP-2", mode = "2560x1440@75", right_of = "DP-1", align = "end" },
    --         { name = "DP-3", above = "DP-1", transform = "90" },
    --         { name = "HDMI-A-1", enabled = false },
    --     },
    --
    -- `name` is the connector name; `solium --probe` prints the ones this
    -- machine has, and a name nothing answers to gets a line in the log rather
    -- than being ignored. Everything else is optional:
    --
    --   right_of, left_of, above, below   beside another monitor, by name.
    --                                     Prefer this to x and y: it does not
    --                                     go stale when a resolution changes,
    --                                     and a chain resolves whatever order
    --                                     you write the list in.
    --
    --   align = "start" | "centre"        which way the *other* axis lines up
    --         | "end"                     when placed beside something taller
    --                                     or wider. "centre" is the default.
    --                                     A 1080p beside a 1440p leaves 360
    --                                     rows belonging to no screen, and
    --                                     this decides which end they are at
    --                                     -- which is where the pointer will
    --                                     catch on the way past.
    --
    --   x, y                              the top-left corner outright, in the
    --                                     one global space every monitor is a
    --                                     window onto. `y` is how much lower
    --                                     one screen sits than another, which
    --                                     is what a monitor on a taller desk
    --                                     actually needs.
    --
    --   mode = "2560x1440@260"            resolution and refresh rate. The
    --                                     refresh is optional -- "2560x1440"
    --                                     alone means the fastest mode at that
    --                                     size. A table `{ w = , h = ,
    --                                     refresh = }` does the same thing, for
    --                                     generating a configuration rather
    --                                     than writing one.
    --
    --                                     Three words also work:
    --                                       "best"       the highest refresh at
    --                                                    the preferred
    --                                                    resolution. The
    --                                                    default.
    --                                       "preferred"  exactly what the
    --                                                    monitor's EDID says,
    --                                                    refresh included --
    --                                                    for one that is
    --                                                    unstable at its
    --                                                    fastest.
    --                                       "widest"     the largest
    --                                                    resolution, fastest at
    --                                                    that size.
    --
    --                                     "best" is not "preferred": the
    --                                     EDID's preferred *flag* names a
    --                                     resolution and usually pairs it with
    --                                     a pedestrian 60 Hz. A 260 Hz panel
    --                                     reports 2560x1440@60 as preferred,
    --                                     and taking that literally drives a
    --                                     fast display slowly and makes every
    --                                     animation look worse than it is.
    --
    --                                     A mode the monitor does not have
    --                                     warns and falls back; `--probe` says
    --                                     how many each one offers.
    --
    --   vrr = true                        variable refresh rate, where the
    --                                     monitor and the driver both offer it
    --                                     -- FreeSync, G-Sync compatible,
    --                                     Adaptive-Sync. The display's refresh
    --                                     follows what is actually being drawn
    --                                     instead of the other way round, which
    --                                     is what removes the tear and the
    --                                     stutter on anything that cannot hold
    --                                     a steady frame rate. Left alone by
    --                                     default, because it interacts badly
    --                                     with some panels at low frame rates
    --                                     (visible flicker) and that is not a
    --                                     thing to turn on for somebody.
    --
    --   transform = "90"                  rotation, anticlockwise, as degrees:
    --                                     "normal", "90", "180", "270", or the
    --                                     same with a "flipped-" prefix. A
    --                                     rotated monitor's work area is
    --                                     portrait, so every layout follows it
    --                                     without knowing about it.
    --
    --   enabled = false                   do not drive it. It also frees its
    --                                     CRTC for another screen, which
    --                                     matters on a card with more
    --                                     connectors than CRTCs.
    --
    --   primary = true                    the monitor a layer-shell surface
    --                                     that names no output goes on, and
    --                                     the one a hosted shell
    --                                     (`shell.on = "primary"`) or a
    --                                     `sol.surface` (`on = "primary"`) is
    --                                     drawn on when told to. Without this
    --                                     it is the first monitor -- stable,
    --                                     but not a choice anybody made.
    --
    --   scale = 2                         how many device pixels to a logical
    --                                     one. Everything doubles in size: a
    --                                     window, a titlebar, the pointer, and
    --                                     the compositor's own QML is
    --                                     rasterised at that many pixels
    --                                     rather than stretched.
    --
    --                                     Left out, it is worked out from the
    --                                     panel's own size -- 2x above 192 dpi,
    --                                     which is the number GNOME and KDE
    --                                     both use, and 1x below. That puts a
    --                                     13" 4K laptop at 2x and a 27" 4K at
    --                                     1x, and the second of those is
    --                                     genuinely a matter of taste, which
    --                                     is why this is settable. `--probe`
    --                                     and the log both print the dpi it
    --                                     measured.
    --
    --                                     Fractional values work; between 0.5
    --                                     and 8. A fraction outside that is
    --                                     warned about and the scale worked
    --                                     out instead, as far likelier a typo
    --                                     than a request; a whole number
    --                                     outside it, such as 0 or 10, is
    --                                     worked out instead with no warning.
    --
    -- `super+shift+r` applies a change to the placement keys, `scale`,
    -- `primary` and `enabled` without ending the session. `mode`, `vrr` and
    -- `transform` are read when a monitor is first lit, so on one already lit
    -- they wait until it is next plugged in, or a reload with
    -- `enabled = false` and another with it back.
    --
    -- A monitor plugged in while the session runs is driven with its entry
    -- here, and one unplugged leaves its windows on the monitors that remain.
    --
    -- A monitor can also be turned *off* without being taken out: it keeps
    -- its place, its work area and its windows, and only the display goes
    -- dark. Three things do it -- `idle` below, `sol.monitor_power` from a
    -- binding, and any client speaking `wlr-output-power-management`, which is
    -- `wlopm` and `swayidle` driving it. Any key, click, scroll, touch or
    -- motion turns every screen back on, and is delivered as usual: the first
    -- key typed at a lock screen that went dark is part of the password.
    --
    --     sol.bind("super+F12", function() sol.monitor_power("all", "off") end)
    --
    -- `sol.monitors()` says which are off, as `power = "on"` or `"off"`.
    monitors = {},

    -- When nobody is at the machine.
    --
    -- Solium tells idle daemons how long it has been left alone
    -- (`ext-idle-notify`) and lets a video player, a call or a game say "not
    -- now" (`idle-inhibit`). This is the part it does itself, so that no
    -- daemon is needed for the screens to go dark at night.
    idle = {
        -- Seconds without a key, a click, a touch or the pointer moving
        -- before the session is locked, the way `lock` below says: Solium's
        -- own lock screen, or your `lock.command`. 0 never does.
        --
        -- Five minutes, before the screens go dark, so a machine left alone
        -- is locked with no daemon to install and the screen that goes dark
        -- is the lock screen. The same inhibitors as below hold it off: a
        -- film is not locked half way through.
        lock_after = 300,

        -- Seconds without a key, a click, a touch or the pointer moving
        -- before every screen is turned off. 0 never does.
        --
        -- Ten minutes: GNOME blanks at five, but dims the screen first and
        -- Solium does not, so here the first sign is the screen going dark.
        -- Anything that plays holds an idle inhibitor, and while one is on a
        -- screen this waits -- a film does not go dark -- and starts again
        -- from when it lets go, so the next film in a playlist does not
        -- start in the dark. Nothing holds it off behind the lock screen,
        -- which goes dark like anything else.
        --
        -- Running `swayidle` instead? Set this and `lock_after` to 0 and give
        -- it the screens:
        --
        --     swayidle -w timeout 300 'swaylock -f' \
        --                 timeout 600 'wlopm --off \*' resume 'wlopm --on \*' \
        --                 before-sleep 'swaylock -f'
        screens_off_after = 600,

        -- How often, in milliseconds, a window on a screen that is off is
        -- still told it may draw. Throttled rather than stopped: a program
        -- that waits for that inside its swap would otherwise stop dead for
        -- as long as the screen is dark. 0 stops it, which is what sway does.
        off_frame_interval = 1000,

        -- Answer the other way programs ask for the screens to stay on:
        -- `org.freedesktop.ScreenSaver` on the session bus, which is what
        -- Chrome and Firefox call during a film instead of the Wayland idle
        -- inhibitor. Such an inhibitor holds the blank above off -- and
        -- `swayidle`'s timeouts -- like one on a window in view, goes when
        -- the program that took it closes or crashes, and holds nothing
        -- behind the lock screen. false leaves the name to whoever else wants
        -- it. Nested, it is owned only on the bus SOLIUM_SESSION_BUS names.
        dbus_inhibit = true,
    },

    -- libinput device settings: tap-to-click, scrolling, acceleration and the
    -- rest (#157). Applied to every device already connected and to each one
    -- added later, and reapplied on reload.
    --
    -- Nothing here is read on the nested backend (`solium` inside another
    -- compositor): there is no libinput device there to apply it to, only a
    -- host window's own pointer and keyboard.
    input = {
        -- Defaults by device type. A touchpad is `Pointer` capability plus
        -- `Gesture` -- the one libinput reports only for a driver that
        -- recognises multi-finger gestures, which in practice means
        -- touchpads and nothing else -- and a plain mouse is `Pointer`
        -- without it.
        touchpad = {
            -- Tapping the pad clicks. libinput leaves this off by default,
            -- which is why a bare compositor's touchpad does nothing to a
            -- tap -- #157's whole opening complaint.
            tap = true,
            -- Scroll direction follows the content under your fingers,
            -- rather than the surface. What every other touchpad on the
            -- market does.
            --
            -- This used to come from SOLIUM_FORM_FACTOR, guessing "a laptop
            -- has a touchpad" for every device at once. A device's own type
            -- answers the real question directly -- a desktop with a USB
            -- touchpad now gets this too, which the guess never could.
            natural_scroll = true,
        },
        -- A mouse keeps libinput's own defaults: no tap (it has no pad to
        -- tap), no natural scroll (a wheel is not a surface).
        mouse = {},
        keyboard = {},
        touchscreen = {
            -- Bind absolute positions to this monitor's connector name.
            -- Unset, use libinput's udev WL_OUTPUT binding; if neither names
            -- a connected monitor, use the first screen. Removing an override
            -- on reload restores that device binding. Relative devices such
            -- as mice and touchpads ignore output.
            -- output = "eDP-1",

            -- How far back, in milliseconds, each finger's velocity (the
            -- `vx` and `vy` of `sol.on("touch")`) is fitted: shorter follows
            -- a change of direction sooner, longer smooths a noisy screen.
            -- A whole number from 1 to 1000; 80 when unset.
            -- velocity_window_ms = 80,
        },
        tablet_tool = {
            -- Same connector binding and fallback as touchscreen.output.
            -- output = "DP-2",
        },
        tablet_pad = {},
        switch = {},

        -- Per-device overrides, matched by name (a case-insensitive
        -- substring of what `libinput list-devices` or `dmesg` calls it) or
        -- by `vendor`/`product` (the same tool's "Vendor"/"Product", or
        -- `lsusb`'s). At least one of the three is required -- an override
        -- naming none of them matches nothing, on purpose, rather than
        -- reaching every device on the machine.
        --
        -- Entries apply in order, over the type default above and over each
        -- other: a later one changes only the fields it names, so two
        -- overrides for the same device can each own a different setting.
        --
        --     devices = {
        --         -- A touchpad that should not act like one.
        --         { name = "SynPS/2 Synaptics", tap = false, natural_scroll = false },
        --         -- A mouse, by its usb id, slowed down.
        --         { vendor = 0x046d, product = 0xc52b, accel_speed = -0.3 },
        --         -- One touchscreen panel, overriding its type's output.
        --         { name = "ELAN Touchscreen", output = "eDP-1" },
        --     },
        devices = {},
    },

    -- Locking. `loginctl lock-session`, a power menu that locks that way, and
    -- a lid switch logind handles itself all send logind's `Lock`; that,
    -- sleep, `idle.lock_after`, `super+ctrl+l` and `sol.lock_now()` all lock
    -- the session the one way this section says: with Solium's own lock
    -- screen, unless `command` names a locker of your own.
    lock = {
        -- A locker of your own, run instead of Solium's lock screen: nothing
        -- by default. Split on whitespace, with no quoting, like an
        -- autostart Exec= line; it runs as a client of this compositor, the
        -- same as any other lock screen.
        --
        --     command = "swaylock -f",
        command = nil,

        -- Hold sleep until the lock is on every screen, Solium's own or the
        -- locker's, or logind's own InhibitDelayMaxUSec (five seconds) runs
        -- out first -- so the machine never wakes showing the desktop it was
        -- left on.
        -- `swayidle`'s own `before-sleep` keeps working either way; set this
        -- to false only to stop the two overlapping.
        before_sleep = true,

        -- Whether `loginctl unlock-session`, from another console or over
        -- SSH, ends Solium's own lock screen: the way out when the password
        -- cannot be checked. false leaves only the password, for a machine
        -- where whoever can reach logind should not unlock this session.
        logind_unlock = true,

        -- Solium's own lock screen (#281): how it takes and checks a
        -- password. The password goes to `solium-auth`, a small helper that
        -- runs PAM's `solium` service; PAM's own rules (pam_faillock) hold
        -- as well as these.
        --
        -- After `attempts` wrong passwords in a row the field is shut for
        -- `seconds`, then twice as long after each further run of them, up
        -- to `max_seconds`.
        lockout = { attempts = 3, seconds = 30, max_seconds = 300 },

        -- Which keyboard layout a password is typed in. "first": the first
        -- layout that types Latin letters, switched to as the session locks
        -- and switched back from as it unlocks, so with `us,ru` a password
        -- is not typed in Cyrillic because Russian was live. "current":
        -- whatever layout is live.
        secret_layout = "first",

        -- How long the helper has to answer before it is stopped and the
        -- password counts as not checked, in milliseconds. A network login
        -- (sssd, LDAP) can take seconds; pam_unix takes a fraction of one.
        helper_timeout_ms = 30000,

        -- When the helper could not answer this time (it crashed, or took
        -- longer than `helper_timeout_ms`), the password typed is tried
        -- again after `seconds`, then twice as long each time, up to
        -- `max_seconds`, at most `attempts` times, until it answers or
        -- Escape clears the field; then it is wiped and can be typed again.
        -- When it never can (it is missing, or PAM has no `solium`
        -- service), the password is wiped at once. When PAM had the password
        -- and asked for more than it (a one-time code), that counts as a
        -- wrong password, toward `lockout`.
        retry = { seconds = 2, max_seconds = 60, attempts = 5 },

        -- The passcode pad a lock screen can show (`SecretKeypad`), for a
        -- touchscreen: "digits", a phone's pad; "letters", the same with
        -- the phone letters under the digits; or "keyboard", the digits and
        -- the letters a-z.
        keypad = "digits",

        -- Whether Solium's own lock screen is the one used. Unset, it is,
        -- unless `command` above names a locker. While it is, each monitor's
        -- lock screen is built as the session starts, and kept, so the
        -- first locked frame is already it; it costs nothing while you work.
        native = nil,

        -- The lock screen itself: a QML file, by path ("~/my-lock.qml"), or
        -- by name, looked for as `qml/lock/<name>.qml` in your
        -- ~/.config/solium/qml/ and then in Solium's. "Lock" is the one
        -- Solium ships. A scene that is missing, will not load, or fails
        -- while it runs is replaced by the shipped one, and that by a
        -- compiled-in fallback, never by the desktop; the log says why.
        -- See docs/ricing.md, "Your own lock screen".
        scene = "Lock",

        -- Which monitor shows the password field: "pointer", the one the
        -- pointer is on; "primary"; or a monitor's name, like "DP-2" (the
        -- primary one while it is not plugged in). The others show their
        -- lock screen without it. Typing goes into the password wherever
        -- it is shown.
        prompt_on = "pointer",

        -- How long the lock screen takes to come down over the desktop as
        -- the session locks (`Lock.state` reads "locking"), in
        -- milliseconds; 0 shows it at once. The keyboard and the pointer
        -- are the lock's from the first instant, but the desktop is still
        -- on screen under it until this is up, so this is at most 600. The
        -- lock taken before sleep never comes down: it is up at once.
        enter_ms = 350,

        -- How long the lock screen stays over the desktop after it unlocks,
        -- playing its exit (`Lock.state` reads "unlocking"), in
        -- milliseconds; 0 removes it at once. The desktop has the keyboard
        -- and the pointer from the unlock on, so this is at most 900.
        exit_ms = 450,

        -- How near a passcode pad (`SecretKeypad`), in pixels, the pointer's
        -- movement is kept from the lock screen, as it is over the pad: the
        -- way the pointer takes to a key says which key. 0 hides it only over
        -- the pad itself.
        keypad_margin = 32,
    },

    -- X11 windows that are refused a tile, decoration or bar entry outright
    -- (#221): some autostart entries, such as KDE's screen-sharing fallback
    -- `xwaylandvideobridge`, exist only so a portal has something to talk to
    -- and were never meant to be seen. See docs/ricing.md.
    x11 = {
        -- WM_CLASS names to hide, matched case-insensitively against either
        -- field X11 clients set (`xprop WM_CLASS` on a nested window shows
        -- both: class first, instance second). A name here replaces this
        -- whole list rather than adding to it, so keep `xwaylandvideobridge`
        -- in your own copy unless you want it shown again.
        --
        --     hidden = { "xwaylandvideobridge", "my-tray-helper" },
        hidden = { "xwaylandvideobridge" },
    },

    -- Who takes the keyboard on a click or a hover, and when nobody does
    -- (#219). `lua/modes.lua` hands this over on every mode change, so a
    -- mode's own entry below answers immediately, without waiting for a
    -- reload.
    focus = {
        -- A press on empty desktop, the wallpaper, or a shell surface that
        -- does not take the keyboard clears keyboard focus, so a window you
        -- clicked away from stops taking what you type. `false` leaves the
        -- last focused window holding the keyboard until something else
        -- takes it.
        clear_on_empty_click = true,

        -- Per mode (the names `modes.register` uses): `"click"` focuses a
        -- window only on a press; `"follow"` also focuses it when the
        -- pointer moves over it. Applied the moment the mode changes, and
        -- never carried over from the mode you switched from. Who decides,
        -- highest first (#128):
        --
        --   1. an entry of yours here, for the mode;
        --   2. your own `follow` below, for every mode at once;
        --   3. the entries shipped here;
        --   4. the machine's own answer
        --      ([#159](https://github.com/Lilium-Linux/solium/issues/159),
        --      `SOLIUM_FORM_FACTOR`): follow on a desktop or a laptop, not
        --      on a tablet or a phone.
        --
        -- Shipped, tiling and scrolling follow the pointer, which is what
        -- every tiling compositor people arrive from does: their windows
        -- never overlap, so pointing at one is choosing it. A shipped
        -- "follow" is the machine's own answer, though, so a tablet or a
        -- phone, with no pointer to hover, stays click-only in them; an
        -- entry of yours follows on any machine. Floating -- the desktop,
        -- with no layout in charge and windows free to overlap -- is
        -- click-only: a window you are not using sits under the pointer on
        -- the way to the one you want, and hovering it should not steal the
        -- keyboard from what you were typing into.
        --
        --     focus = { modes = { floating = "follow", scrolling = "click" } },
        --
        -- An entry that is neither word counts as none.
        -- `your_mode_entry_then_your_focus_follow_then_the_shipped_entry_then_the_machine`,
        -- `the_shipped_focus_follows_the_pointer_in_tiling_and_scrolling_and_not_in_floating`,
        -- `on_a_tablet_the_shipped_layouts_leave_hover_to_the_machine` and
        -- `the_shipped_modes_each_hand_over_their_own_focus_model`.
        modes = {
            floating = "click",
            tiling = "follow",
            scrolling = "follow",
        },

        -- Where the keyboard goes when the window that has it closes (#128):
        -- per mode, a list of these, tried in order until one finds a window
        -- on screen. The keyboard moves when the close lands -- as the window
        -- finishes fading, not while you can still see it -- and when a
        -- window goes on its own.
        --
        --   "pointer"    the window under the pointer, where the windows are
        --                going: what hovering would pick the moment the
        --                pointer moved, so it comes first where hover focus
        --                is on.
        --   "neighbour"  the window that takes its place on its monitor --
        --                the tile that grows into the space, the column that
        --                slides into it -- or, where nothing does, the one
        --                nearest to where it stood. Of several, the one you
        --                used last.
        --   "recent"     the window you used before it, still open on its
        --                monitor.
        --   "topmost"    the topmost window on screen, on any monitor: what
        --                the compositor does with no configuration, after
        --                the window under the pointer.
        --
        -- A word that is not one of these is named in the log and skipped.
        -- An empty list, `{}`, leaves the keyboard on nothing until you
        -- click or point at a window. A mode with no entry here gets the
        -- compositor's own rule, `{ "pointer", "topmost" }`.
        --
        --     focus = { after_close = { tiling = { "recent", "topmost" } } },
        --
        -- `in_tiling_the_window_that_takes_the_space_takes_the_keyboard`,
        -- `in_scrolling_the_column_that_takes_the_place_takes_the_keyboard`,
        -- `in_floating_the_window_focused_before_it_on_that_monitor_takes_the_keyboard`,
        -- `with_after_close_pointer_the_window_under_the_pointer_takes_the_keyboard`
        -- and `with_after_close_recent_tiling_hands_the_keyboard_to_the_window_used_before`.
        after_close = {
            floating = { "recent", "topmost" },
            tiling = { "pointer", "neighbour", "topmost" },
            scrolling = { "pointer", "neighbour", "topmost" },
        },

        -- `click` and `follow` above, for every mode at once rather than one
        -- at a time -- not here by default, so that leaving them out means
        -- the shipped entries in `modes` and then this machine's own answer,
        -- rather than a fixed default that cannot know it. Yours outrank
        -- every shipped entry, and an entry of yours in `modes` outranks
        -- your `follow`; `click` has no entries per mode, so it is yours or
        -- the machine's:
        --
        --     focus = { follow = false },   -- never follow the pointer, in any mode
        --     focus = { follow = false, modes = { tiling = "follow" } },   -- only in tiling
        --     focus = { click = false },    -- a mode must `sol.focus` its own clicks
        --
        -- `your_focus_follow_false_keeps_every_mode_from_following_unless_you_name_one`.

        -- Where the pointer goes when the keyboard moves (#220).
        --
        -- Moving focus with the keys -- `super+arrows`, a workspace key, a
        -- move -- would leave the pointer over the window you left. With this
        -- it goes to the window that took the keyboard, and its picture
        -- glides there along `motion`. The pointer itself is there at once,
        -- so what it is over -- a hover, a click, the cursor's shape -- is
        -- the new window's from the first instant. Moving the mouse while it
        -- glides carries on as `interrupt` says, and a click that takes the
        -- pointer along waits for the button to come up, so the click lands
        -- where it was aimed. `lua/follow.lua` is the whole of the policy,
        -- and `sol.warp` the compositor's part.
        cursor = {
            -- Which focus changes take the pointer along, by what moved the
            -- keyboard: the `source` the `focus` event is told
            -- (docs/modes.md). The keys do and the mouse does not: a click
            -- or a bar's window chip is a hand already on the pointer, and a
            -- window that opens is not somewhere you went.
            follows = {
                binding = true, -- any key binding: a workspace key, the overview's keys
                direction = true, -- super+arrows and super+h/j/k/l
                move = true, -- super+shift+arrows, scrolling's column moves: to where the window was moved
                click = false, -- a press on a window, or on one in the overview
                scroll = false, -- the wheel with super held
                drop = false, -- a dragged window let go
                touch = false, -- a touch on a window
                hover = false, -- focus following the mouse
                surface = false, -- a bar's window chip, the dock, a shell's search
                open = false, -- a window opening
                activate = false, -- an application bringing its window forward
                settle = false, -- the window you were in went, and another took over (focus.after_close)
                script = false, -- `sol.focus` from anything else
            },
            -- Where on the window: "center", or a share of its width and
            -- height, as `{ x = 0.5, y = 0.25 }`.
            to = "center",
            -- How the picture travels there: milliseconds and an easing, as
            -- every `motion` here, or `easing = "spring"`, which takes as
            -- long as it takes. `false` puts it there at once.
            motion = { duration = 260, easing = "outCubic" },
            -- Where the mouse carries on from if it moves before the picture
            -- lands: "pointer", from where the pointer already is -- the
            -- picture jumps there, and with focus following the mouse the
            -- window the key chose keeps the keyboard -- or "picture", from
            -- where you see the picture, which may be over the window you
            -- left.
            interrupt = "pointer",
            -- A key that focuses again the window that already has the
            -- keyboard -- scrolling does after every key it binds, `super+r`
            -- that only widens the column included -- takes the pointer to
            -- it too when true. The keys that move the focused window take
            -- it along by `follows.move` either way.
            refocus = false,
            -- A workspace switch that leaves no window to take the keyboard,
            -- from a source above that takes the pointer along: "center" puts
            -- the pointer in the middle of the monitor that switched, "stay"
            -- leaves it where it is.
            empty_workspace = "center",
        },
    },

    -- Telling the rest of the session that Solium is its desktop.
    --
    -- Portals, programs D-Bus starts on demand, ~/.config/autostart and user
    -- services written for "a graphical session" are all started by systemd
    -- or by D-Bus, and they find the display only if they are told where it
    -- is. Read when the session starts, so a reload does not change it.
    -- `systemd` and `autostart` count only when Solium is started as the
    -- session: the session file's `solium-session`, which runs
    -- `solium --tty --session`. `solium --tty` by hand and a nested run leave
    -- the session around them alone, unless SOLIUM_SESSION_BUS names a bus to
    -- tell instead. `stop_timeout` counts however Solium was started. The
    -- tests are in `session.rs`: `a_reload_leaves_the_session_as_it_began`,
    -- `a_manual_tty_start_tells_nobody`,
    -- `a_nested_run_tells_nobody_unless_it_is_given_a_bus`,
    -- `the_target_is_stopped_and_the_variables_unset_on_exit`,
    -- `session_systemd_false_tells_nobody_anything` and
    -- `session_autostart_false_starts_the_session_without_autostart`.
    session = {
        -- Tell systemd's user manager and D-Bus activation WAYLAND_DISPLAY,
        -- DISPLAY, XDG_CURRENT_DESKTOP and XDG_SESSION_TYPE, and start
        -- solium-session.target, which starts graphical-session.target and
        -- every unit you have attached to it. On exit the target is stopped
        -- and the variables unset. `false` does none of it.
        systemd = true,
        -- Whether solium-autostart.target starts beside it: XDG autostart,
        -- the programs in ~/.config/autostart and /etc/xdg/autostart, which
        -- is how a polkit agent, a keyring or nm-applet usually start.
        -- `false` leaves autostart out, and solium-session.target, with
        -- whatever is attached to it, still starts.
        autostart = true,
        -- How long, in milliseconds, SIGTERM, SIGINT or SIGHUP waits for
        -- Solium to stop cleanly, however it was started. The first of them
        -- stops it the way a quit binding does. If it is still running this
        -- long afterwards, something inside it has stopped answering, and
        -- it ends at once. The same signal again, half a second or more
        -- after the first, ends it at once too. Tests in `signals.rs`:
        -- `a_process_that_cannot_stop_ends_when_its_stop_timeout_has_passed`,
        -- `the_same_signal_twice_ends_a_process_that_cannot_stop` and
        -- `the_same_signal_twice_at_once_is_one_request`.
        stop_timeout = 5000,
    },

    -- How every window is framed.
    --
    -- A style is a **folder** under `qml/panes` holding a `Pane.qml`: what the
    -- frame reserves from the client, and a list of layers, each its own QML
    -- scene at its own depth -- behind the client, in the frame, or above it.
    -- A name is one of the folders below, or one of your own in
    -- ~/.config/solium/qml/panes, which shadows a shipped one of the same
    -- name. A path is anywhere.
    --
    --   "glass"      the default: the window and its bar one rounded piece of
    --                dark liquid glass, the bar along the bottom with the
    --                buttons at its left, the title after them and the
    --                application's icon at its right; a thin dark shade just
    --                inside the window's rounded edge while it does not have
    --                the keyboard
    --   "top"        a titlebar above the window, opaque and square: the
    --                default until glass, and `pane = "top"` in user.lua
    --                goes back to it
    --   "left"       a titlebar down the left side
    --   "bottom"     a titlebar underneath
    --   "border"     no bar, just a frame
    --   "reactive"   a border lit where the cursor is, with a bar
    --   "proximity"  a border that answers the pointer arriving and leaving
    --   "reveal"     a bar that slides out of the window's edge on approach
    --   "pulse"      a bar with an animation running in it
    --   "rounded"    the window's own corners cut round, with a bar that
    --                fills the two notches at the top
    --   "flush"      a window with a square top and a rounded bar sitting
    --                flush on it
    --   "shadow"     a drop shadow behind the window, from stacked
    --                rectangles rather than a blur
    --   "none"       no frame at all: no bar, no border, and no QML scene
    --                built per window. For a desktop with no window furniture,
    --                or a tiling layout whose own bar makes a titlebar
    --                redundant.
    --
    -- "glass", "rounded" and "flush" cut the client's corners as it is
    -- drawn, at no extra cost: no pass and no copy of the window. The panes
    -- README, `qml/panes/README.md`, has what each style is made of and
    -- costs. A style folder may also give its windows effect rules, in an
    -- `effects.lua` beside its Pane.qml; yours in `effects.rules` win over
    -- them. "glass" is the one that ships with a rule: the `glass` effect
    -- behind every window it frames, except a fullscreen one, which keeps
    -- direct scanout. Its blur is run once per monitor and shared, and each
    -- window refracts its own share; the other styles cost no effect at all.
    -- SOLIUM_PANE overrides this, for one run.
    --
    -- A single QML file still works and is still called a decoration: drop one
    -- in ~/.config/solium/qml/decorations and name it here. It is one layer in
    -- the frame, which is what every style was before folders.
    --
    -- This setting was called `decoration` when a style was one file. The old
    -- name is still read -- see the bottom of this file.
    pane = "glass",

    -- What a window does between being asked for and its application
    -- arriving. A window's life starts when you ask for it, not when the
    -- program gets around to connecting -- these decide what that looks like.
    loading = {
        -- Which QML draws it. A name is one of the scenes in `qml/loading`,
        -- or one of your own in ~/.config/solium/qml/loading, which shadows a
        -- shipped one of the same name. A path is anywhere. SOLIUM_LOADING
        -- overrides this, because that is set per run.
        scene = "window",
        -- How long to keep a window open for an application that never
        -- arrives, in milliseconds. After that the layout is told the window
        -- has gone, and the window fades out of its place with the same fade
        -- a window you close plays, the scene it was showing fading with it.
        -- There is no application to ask, so there is nothing to wait for and
        -- nothing to refuse. A window whose scene would not load has nothing
        -- on screen to fade, and goes at once.
        --
        -- An application that was already running does not arrive either: it
        -- answers the launch by bringing forward the window it has, and the
        -- window opened for the launch goes the same way the moment it does,
        -- without waiting this long. The view goes to the window it brought
        -- forward, on its own workspace (`activate` in docs/modes.md, and
        -- `relaunching_an_application_whose_window_is_on_another_workspace_keeps_it_there_and_brings_it_into_view`).
        patience = 8000,
        -- Whether it takes its place in the layout straight away. With this
        -- off, the other windows only move aside once the application is
        -- really there -- less eager, and some people will prefer it.
        reserves_a_slot = true,
        -- Whether the frame is *drawn* while it waits. The room it takes is
        -- reserved either way, so the window does not change shape when the
        -- application arrives; this only decides whether the bar is on screen
        -- meanwhile. On, and you get a close button for an application that is
        -- not coming. Off, and the scene has the whole window.
        decorated = false,
        -- How long the scene takes to fade off the application that replaced
        -- it, in milliseconds. It is drawn *over* the window, so what is
        -- underneath is already the application. 0 cuts straight to it.
        fade = 180,
    },

    -- Reload automatically when a file this configuration loaded changes
    -- (#223): the configuration directory itself, every Lua module
    -- `require` found in it, your pane styles, your loading scenes and your
    -- shell, if you have one. Saved the same way `super+shift+r` reads it --
    -- a broken file leaves the running configuration alone and reports it
    -- the same way a manual reload does.
    reload = {
        -- false turns this off entirely: nothing is watched at all, not
        -- merely "watched but never acted on" -- so a configuration directory
        -- on a slow network mount, say, costs nothing once this is off.
        automatic = true,

        -- How long a burst of writes waits to go quiet before the one reload
        -- it earns, in milliseconds. An editor that saves by writing a
        -- temporary file and renaming it over the original fires this
        -- several times a few milliseconds apart; 300 lands all of them in
        -- one quiet period while still feeling instant for a single save.
        quiet_ms = 300,
    },

    -- What a window looks like while it is changing size: while you drag
    -- its edge or a seam, while a layout glides it to a new tile, and while
    -- it glides into or out of fullscreen or maximised.
    --
    -- A client cannot be resized; it can only be *asked*, and it answers when
    -- it gets round to it -- a few milliseconds for a terminal, rather more
    -- for a browser. The compositor does not wait for that answer: the
    -- rectangle you are dragging, or the glide has reached, is what it draws,
    -- from the first frame, and the client's picture -- the old one, or its
    -- answer while the glide is still on its way -- goes into it until the
    -- two agree. This decides how, which is entirely taste.
    resize = {
        -- "anchor", the default: the picture at its own size, never scaled,
        -- against the edges that are not moving -- the right edge of a window
        -- you shrink from the left, the top-left when every edge moves. What
        -- is larger than the window is cut off on the moving side, and what
        -- it does not cover is `background` below. Text keeps its size on
        -- every frame.
        --
        -- "crop": the same, but always from the window's top-left corner,
        -- whichever edge moves -- so the left of a left-aligned page, a
        -- prompt or a gutter, moves with a left edge you drag, and what is
        -- cut off or uncovered is on the right or the bottom.
        --
        -- "stretch": the picture scaled to fill the window, each way on its
        -- own. Nothing is ever uncovered, but text is drawn wider or taller
        -- than the application drew it until it catches up -- nearly twice
        -- as wide on a glide that halves a terminal. What most compositors
        -- do, and what Solium did before #233.
        --
        -- "scene" draws the window's QML scene over it -- the same one a
        -- window wears before its application has painted. Today that only
        -- reaches a window resized before its application ever arrived;
        -- anywhere else it behaves as "anchor".
        --
        -- Whatever this says, a client that *refuses* the size it is offered
        -- -- Firefox will not go under its minimum width -- is never
        -- stretched once it has said so, and the window takes the size the
        -- client chose when you let go of the edge. Anything else is named in
        -- the log and the default kept.
        fill = "anchor",

        -- What shows in the part of a window its picture does not cover
        -- under "anchor", "crop" or "scene": "#rrggbb", or "#rrggbbaa" with
        -- an alpha. The default is the theme's darkest grey, so the gap reads
        -- as part of the window. "none" draws nothing there, so what is
        -- behind the window -- or a style's own `behind` layer -- shows.
        -- Anything else is named in the log and the default kept.
        background = "#1c1c1c",

        -- How long, in milliseconds, a window's picture waits at the
        -- rectangle a change gave it for its application to draw at that
        -- size, once the change is over: after you let go of an edge, after
        -- a fullscreen or maximise glide lands, and after a layout's glide
        -- lands -- when the picture stays against the edges the glide did not
        -- move. Then the window is shown at the size the application has. 0
        -- (do not wait) to 5000; anything else is named in the log and the
        -- default kept.
        -- `a_longer_patience_keeps_a_landed_glides_picture_where_it_was_for_longer`.
        patience = 250,

        -- A fill of its own for a mode (the names `modes.register` uses),
        -- for every size change made while that mode is in charge: its
        -- drags, its glides, and a fullscreen glide begun in it. A mode with
        -- no entry takes `fill` above. Handed over whenever the mode
        -- changes, as `focus.modes` is:
        --
        --     resize = { modes = { scrolling = "stretch" } },
        --
        -- An entry that is not a fill is named in the log and "anchor"
        -- kept, as `fill`'s is.
        -- `each_mode_fits_a_resize_by_its_own_fill_or_the_one_for_every_mode`.
        modes = {},
    },

    -- A fullscreen window: what it covers, and how a window goes fullscreen
    -- and comes back (super+f, or the application asking).
    fullscreen = {
        -- What a fullscreen window covers, when it is the one in front on the
        -- workspace its monitor is showing.
        --
        -- "top", the default: the top layer, so a bar -- a client's, like
        -- Waybar, or one declared with `sol.surface` -- goes under a
        -- fullscreen video or game, and the video takes the clicks where the
        -- bar was. The overlay layer stays over it: notifications, an OSD, a
        -- launcher. A window going fullscreen goes over the bars as it starts
        -- to grow, and one leaving goes back under them once it has finished
        -- shrinking.
        --
        -- "none": nothing, and the bars stay over fullscreen windows.
        --
        -- Anything else is named in the log, and the default kept.
        covers = "top",

        -- How a window grows to cover its monitor, and shrinks back to where
        -- it was: from wherever it is drawn, as a window a layout moves
        -- glides, so pressing the key again half way turns it round where it
        -- is. The application is told its new size the moment the key is
        -- pressed, and its last picture goes into the new rectangle as
        -- `resize.fill` above says until it has drawn one at that size:
        -- through the glide and after it, for a quarter of a second past the
        -- landing at most, and then the window is shown at the size the
        -- application has. Once it has answered the window is drawn as it
        -- is, with nothing in between, so a game or a video can be shown
        -- directly.
        --
        -- `duration` is in milliseconds and `easing` is a curve, by name or
        -- as four numbers, as for every `motion` here (docs/animation.md).
        -- `false` makes every change instant -- nothing is moved or held,
        -- and the window is drawn as it is on the next frame --
        -- and `true` is the motion below again:
        --
        --     fullscreen = { animate = false },
        --
        -- Change this default and `SHIPPED` in `lua/fullscreen.lua` with it:
        -- that is the motion `animate = true` turns back on.
        animate = { duration = 260, easing = "outCubic" },

        -- Applications that go fullscreen and back at once whatever `animate`
        -- says, by their app id: the name `app_id` gives each window in
        -- `sol.windows()`. A game, or a video player started fullscreen, may
        -- want to be there at once:
        --
        --     fullscreen = { instant = { app_id = { "mpv", "gamescope" } } },
        --
        -- Empty as shipped, so every application animates. A list is
        -- replaced whole, so the list you write is the whole list.
        instant = { app_id = {} },
    },

    -- How a window is maximised to fill the work area of its monitor, and
    -- restored (super+shift+m, or the button on its frame). The same two
    -- settings as `fullscreen`'s and read the same way, but on their own: a
    -- maximise can move differently from a fullscreen, or not at all, and
    -- changing one changes nothing about the other.
    maximize = {
        -- How a window grows to fill the work area and shrinks back, as
        -- `fullscreen.animate` says for going fullscreen: a duration and an
        -- easing, `false` for instant, or `true` for the motion below.
        --
        --     maximize = { animate = false },
        --
        -- Change this default and `SHIPPED` in `lua/fullscreen.lua` with it.
        animate = { duration = 220, easing = "outCubic" },

        -- Applications that are maximised and restored at once whatever
        -- `animate` says, by app id, as `fullscreen.instant` lists them.
        instant = { app_id = {} },
    },

    floating = {
        -- Whether a floating window's edge drag stops at its application's
        -- own minimum and maximum size (#115). The layouts' say in the matter
        -- is `tiling.client_minimum` and `tiling.client_maximum`; this one is
        -- the compositor's, because no layout is asked about a floating drag.
        --
        -- "respect", the default: the frame you drag never shows a size the
        -- application is about to refuse. "ignore": the drag goes wherever
        -- the pointer does, as before #115, and an application that refuses
        -- the size is treated as the end of `resize` above says. An
        -- application in `tiling.client_size_ignore` is not believed here
        -- either. Anything else is read as "respect".
        client_limits = "respect",

        -- Snapping a window like Windows does (#222): `lua/floating.lua`.
        --
        -- Bound only while floating is the mode in charge -- toggled the way
        -- `overview.lua` binds Escape only while it is up -- so tiling and
        -- scrolling keep the plain arrows for focus by direction (#150) and
        -- these never clash with them. `false` unbinds one, as a key in
        -- `bindings` above does; a different combo rebinds it.
        --
        --   left, right   snap the focused window to that half of its
        --                 monitor's work area. Pressed again from that half,
        --                 the next monitor that way, if there is one.
        --   maximize      the work area, whole; from a half, that half's top
        --                 quarter instead.
        --   restore       the rectangle the window had before its first snap,
        --                 or, from the work area, un-maximises.
        snap = {
            left = "super+left",
            right = "super+right",
            maximize = "super+up",
            restore = "super+down",
            -- The motion a snap moves with. Not `tiling.snap`: a window
            -- gliding to half the screen is this file's own question, kept
            -- apart so the two can be tuned differently, as `fullscreen` and
            -- `maximize` above are kept apart from each other.
            motion = { duration = 220, easing = "outCubic" },
        },

        -- Where a new window goes, and how big, while floating is in charge.
        -- `lua/floating.lua`; a dialog is never placed by this, since a modal
        -- is lifted out of any arrangement already (`dialogs.lua`).
        placement = {
            -- Where a new window goes. "center", the default: the middle
            -- of the focused monitor's work area. "cascade": one `cascade`
            -- step from the last window opened, wherever it is now; the
            -- middle when there is none. "pointer": centred on the pointer,
            -- kept inside the work area. Anything else is read as "center".
            --
            -- Whichever it is, a new window never lands on a spot another
            -- window already holds -- its top-left corner nearer than half a
            -- `cascade` step on both axes -- but steps on by `cascade` until
            -- it finds a free one: a row of new windows steps down and right
            -- rather than piling up (#240).
            policy = "center",
            -- How far each step goes, right and down, in logical pixels: a
            -- number of 1 or more on each axis; anything else is read as 32.
            cascade = { x = 32, y = 32 },
            -- Where a new diagonal starts once the next step would take a
            -- window past the edge of the work area, rather than clamping it
            -- back onto the last spot that fit. "corner", the default: the
            -- work area's top-left plus one `cascade` step -- down only as
            -- far as a window nearly the work area's height leaves room
            -- for -- and one `cascade.x` further right each time that
            -- diagonal fills too. "center": back at the middle, one
            -- `cascade.x` further right for each time it has wrapped, so it
            -- never lands on the first window. Anything else is read as
            -- "corner". Once those diagonals reach the right edge, or have no
            -- room at all (a window nearly the work area's width), a new
            -- window takes the first free spot of a `cascade` grid over the
            -- work area, a row at a time from its top-left. Only when every
            -- spot of that grid is taken -- a window as large as the work
            -- area has only the one -- does a window land on another.
            wrap = "corner",
            -- A new window's share of its monitor's work area on each axis,
            -- when its application asked for no size of its own -- one that
            -- has not shown its first frame yet, which is every window
            -- `sol.spawn` opens (see `open` in docs/modes.md). Capped by the
            -- work area either way, and by the application's own minimum and
            -- maximum once it has one (#115, `sizes.lua`). A number above 0
            -- and at most 1; anything else is read as 0.6.
            size = 0.6,
        },
    },

    tiling = {
        -- Where a split falls, as a share of the window being divided.
        -- Hyprland calls this dwindle:default_split_ratio.
        split = 0.5,

        -- The smallest a tile may be, frame included, in logical pixels.
        --
        -- A new window that would make a tile smaller than this goes where
        -- `overflow` says instead. And a seam cannot be dragged, or moved from
        -- the keyboard, to make a tile smaller than this. A tile that is
        -- already smaller does not jump when you grab it, and cannot be made
        -- smaller still; it can be grown only while the tiles across the seam
        -- have room to give. So a seam with a tile under this on both sides
        -- does not move at all -- and that is what `"allow"` below usually
        -- makes, since it halves a tile too small to split.
        --
        -- A tile and not an application's own minimum size: a window you
        -- launch from here is given its tile the moment you ask for it,
        -- before its application has started, so there is no application yet
        -- to ask. An application's own minimum is `client_minimum` below.
        --
        -- 0 on a side is no minimum on that side.
        minimum = { w = 160, h = 96 },

        -- Whether an application's own minimum size gets a say (#115). Some
        -- applications will not go under a size -- Firefox has a minimum
        -- width -- and say so. You have the last word, not the application:
        -- this is where you give it.
        --
        -- "respect", the default: a window is laid out at least as large as
        -- its application's minimum wherever the arrangement has the room. It
        -- is given a larger share, taken from the windows beside it down to
        -- their own minimums and `minimum` above, and the share goes back when
        -- the application's minimum does. A new window that would crowd one
        -- goes where `overflow` says. A seam stops where a window beside it
        -- reaches its minimum. And in the scrolling layout, a column is never
        -- narrower than the widest minimum in it.
        --
        -- Where the room is not there, the window is laid out smaller anyway
        -- and its application's picture is cut to the tile. One line in the
        -- log says so, with the numbers, and the window is `cramped = true` in
        -- `sol.windows()` until it has room: docs/modes.md has a bar that shows
        -- it.
        --
        -- "ignore": laid out as though no application had a minimum, which is
        -- how tiling behaved before #115. Anything else is read as "respect".
        client_minimum = "respect",

        -- What a window whose tile is larger than its application's maximum
        -- size does with the rest of the tile.
        --
        -- "center", the default: it is its maximum size, in the middle of the
        -- tile. Dragging its edge still moves the seam, from the tile's edge.
        -- "ignore": it sits in the tile's top-left corner at the size its
        -- application chose, as before #115. Anything else is read as
        -- "center".
        client_maximum = "center",

        -- Applications whose sizes are not believed at all, by app_id: a
        -- Wayland application's own, or an X11 one's WM_CLASS class.
        -- `sol.windows()` lists each window's. A window of one of these is
        -- laid out as though its application had said nothing, minimum and
        -- maximum alike, and a floating drag of it is not held to them either
        -- -- for an application that claims a size it does not mean. For
        -- instance { "steam", "org.gnome.Nautilus" }.
        client_size_ignore = {},

        -- Where a new window goes when splitting the tile under the pointer
        -- would make a tile smaller than `minimum` -- side by side and one
        -- above the other, since a tile with no room one way may have it the
        -- other. Tried in order; the first with room wins.
        --
        --   "largest"    split the largest tile on this workspace that has
        --                room for it
        --   "workspace"  open it on the next empty workspace of the monitor
        --                it opened on. The workspaces are the fixed set
        --                `workspaces` describes, so when none is empty this
        --                step does nothing and the next one runs. A window
        --                that belongs to no workspace in particular is on all
        --                of them, and with `workspaces.follow_new_windows`
        --                off that is every window -- so then none is empty.
        --   "allow"      split the tile under the pointer anyway, below
        --                `minimum`
        --
        -- A list that runs out without placing the window -- one with no
        -- "allow" -- ends in "allow" all the same, with a line in the log: a
        -- window has to go somewhere. Put "workspace" first and a window goes
        -- straight to the next empty workspace once the tile under the pointer
        -- is full; leave "workspace" out and no window ever changes workspace
        -- for want of room.
        --
        -- Only a window being opened overflows, and only while tiling is the
        -- layout in charge. A window already open that goes back into the
        -- arrangement -- tiling switched on, a reload, a monitor change, a
        -- close the application refused -- never leaves its workspace for
        -- this: it takes a tile with room if there is one and a tile under
        -- `minimum` if there is not. A refused window and a dropped one go
        -- back to the tile where they were, whatever its size.
        overflow = { "largest", "workspace", "allow" },

        -- Whether the view goes with a window that opened on another
        -- workspace, the way `super+<n>` takes it there. The keyboard goes to
        -- the window -- for one launched from a binding, once its application
        -- has put up its window; until then it is not on the new window, and
        -- can still be on the one you were working in, a screen away by then.
        --
        -- false opens it there and leaves you, keyboard and all, where you
        -- are. Each window with no room then goes to another empty workspace,
        -- since the one the window before went to is no longer empty --
        -- whether or not it has room -- until none is left empty.
        follow_overflow = true,

        motion = { duration = 240, easing = "outCubic" },
        -- The shorter feel for a window snapping back after a drag.
        snap = { duration = 180, easing = "outCubic" },
        -- When the other windows close up around one you close.
        --
        -- "immediate", the default: the moment you close it. The window fades
        -- out where it stood, in front of its neighbours as they grow into its
        -- space. If the application refuses to go -- an unsaved-changes prompt
        -- -- the window comes back split off whichever window now covers where
        -- it was, which is its old neighbour when nothing else has moved, at
        -- `split` rather than the ratio it had.
        --
        -- The compositor waits a second for an application to go before it
        -- takes the silence for a refusal. One slower than that to quit --
        -- some Electron and Java applications, a browser saving its session --
        -- is put back and then goes, so its neighbours grow, shrink back and
        -- grow again. "when_gone" moves them once.
        --
        -- "when_landed": once the fade has played, as the application is
        -- asked to quit -- the window goes out where it stood, and then the
        -- others close up. A refusal puts it back as "immediate" does.
        --
        -- "when_gone": once the application has actually quit. Its tile stays
        -- reserved for the length of the fade and for however long the
        -- application then takes, and a refusal has nothing to put back. This
        -- is how every close behaved before #128.
        --
        -- Whichever, only while tiling is the layout in charge.
        -- `with_reflow_on_close_when_landed_the_grid_waits_for_the_close_to_land`
        -- and `a_window_that_asks_inside_itself_gets_its_tile_back`.
        --
        -- Anything else is read as "immediate".
        reflow_on_close = "immediate",

        -- What a keyboard move (super+shift+arrows, #150) does with the
        -- window it moves towards: the tile wholly past it that way and level
        -- with it, never one off on a diagonal.
        --
        -- "swap", the default: the two trade tiles, and every split keeps its
        -- axis and its ratio. Where the tile it left is the one that way from
        -- the tile it took, as in a grid, the opposite move puts both back.
        -- niri moves windows this way; sway and Hyprland reshape the tree
        -- instead, which is what a dragged seam does not survive.
        --
        -- "split": the window leaves its tile and splits the neighbour's
        -- across that tile's longer side, as a window opening in it would, as
        -- Hyprland's `movewindow` does. Pressed again, it goes on into the
        -- next one.
        --
        -- At the edge of a monitor either way goes onto the next monitor, in
        -- beside the tile it arrives at. Anything else is named in the log
        -- and read as "swap". `with_tiling_move_split_a_move_goes_into_the_neighbours_split`,
        -- `in_tiling_focus_and_move_reach_the_neighbour_in_each_direction` and
        -- `in_tiling_a_tile_off_on_a_diagonal_is_not_that_way`.
        move = "swap",
    },

    scrolling = {
        -- The widths a column cycles through with super+r, as shares of the
        -- view.
        --
        -- Read by `sol.layout.scroller` since #117. Before that the strip used
        -- the constant list in `crates/layout/src/scroller.rs` and these two
        -- lines were a description of it -- so a configuration asking for
        -- quarters got thirds, and nothing anywhere said which of the two had
        -- been believed.
        --
        -- An entry that is not a share of a view -- zero, negative, above 1,
        -- not a number at all -- is dropped with a line in the log, and a list
        -- with nothing usable left falls back to this one. Refused rather than
        -- clamped, for the reason `cursor.size` is: a column at 0.001 of the
        -- screen is as unusable as no column, and nobody meant to ask for one.
        widths = { 1 / 3, 1 / 2, 2 / 3 },
        -- Which of them a new column opens at, and where the cycle starts.
        -- 1-based, the way Lua counts. Past the end of the list is the last
        -- width, with a line in the log.
        default_width = 1,
        motion = { duration = 260, easing = "outCubic" },
        -- The shorter feel for bringing a column into view.
        snap = { duration = 200, easing = "outCubic" },
        -- When the strip closes the gap a window you close leaves: the moment
        -- you close it ("immediate", the default), once its fade has played
        -- ("when_landed"), or once the application has actually quit
        -- ("when_gone", how it was before #128).
        -- A window whose application refuses to go comes back in a column of
        -- its own after the window that was before it; one that was first
        -- comes back first. Anything else is read as "immediate". The same
        -- setting as `tiling.reflow_on_close`, kept apart so the two layouts
        -- can be set differently -- and with the same cost for an application
        -- slower than a second to quit, which closes the gap, opens it and
        -- closes it again.
        reflow_on_close = "immediate",
    },

    workspaces = {
        -- Whether each monitor has its own active workspace.
        --
        -- On, `super+2` switches the screen the pointer is on and leaves the
        -- other showing whatever it was: a reference on the second monitor
        -- stays put while you move around on the first. This is what sway,
        -- Hyprland and niri do, and what most people expect.
        --
        -- Off, one switch moves every screen at once, so a workspace is a
        -- whole desk rather than a screenful. That is GNOME's model, and it is
        -- the right one if you think of your two monitors as one surface you
        -- happen to have cut in half.
        --
        -- Only matters with more than one monitor.
        per_monitor = true,

        -- "horizontal": workspaces sit in a row and slide sideways.
        -- "vertical":   a column, sliding up and down.
        -- "grid":       both, `columns` wide and `rows` tall.
        --
        -- The arrangement decides which way a switch travels, and that is the
        -- whole difference between the three: a workspace to the right of this
        -- one enters from the right, because that is where it is.
        --
        -- It also decides how many there are: `columns` in a row, `rows` in a
        -- column, `columns` times `rows` in a grid -- so four, as shipped.
        arrangement = "horizontal",
        columns = 4,
        rows = 2,
        -- How far apart workspaces sit, as a fraction of the screen. Above 1.0
        -- there is a gap of empty space between them mid-slide, which reads as
        -- distance rather than as a cut.
        spread = 1.06,
        motion = { duration = 300, easing = "outCubic" },
        -- Whether a new window joins the workspace you are looking at.
        follow_new_windows = true,
    },

    -- The preview dock is configured under shell.dock. The old top-level
    -- dock.items and dock.morph keys were never read and are not supported.
    -- Live icon anchors and launch morphs remain separate future work.

    open = {
        -- The animation a window arrives with.
        --
        -- These read 200 / outCubic / 0.92 until #117, and `open.lua` had never
        -- looked at them: it held 220 / outBack / 0.88 as local constants, so
        -- what was written here described an animation nobody had ever seen.
        -- Wiring the two together meant choosing which pair was the default,
        -- and the script's won -- those are the numbers the animation was
        -- actually tuned against and the ones every session so far has been
        -- watching. Taking the advertised pair instead would have changed how
        -- every window on every machine opens, as a side effect of correcting
        -- a comment.
        motion = { duration = 220, easing = "outBack" },
        -- How small it starts, as a share of the size it ends up. `outBack`
        -- overshoots, so it grows a little past that and settles back.
        scale = 0.88,
    },

    -- Effects: folders of Lua and GLSL in ~/.config/solium/effects/ (yours)
    -- or shipped with Solium, put in a slot of a part of a window by a rule.
    -- See crates/solium/effects/README.md for the folders and docs/ricing.md
    -- for rules. With no rules nothing changes, and costs nothing. The
    -- default pane style, "glass" (`pane` above), brings two rules of its
    -- own: the `glass` effect behind every window it frames, and the `shade`
    -- in front of one without the keyboard, fading with the focus. A rule of
    -- yours for `{ style = "glass" }`, the `pane` part and its `behind` or
    -- `front` slot wins over the style's; `effect = false` there takes it
    -- away.
    effects = {
        -- { match = { app_id = "mpv" }, part = "client", slot = "replace",
        --   effect = { "blur", source = "self" } },
        -- { match = { app_id = "kitty" }, part = "client", slot = "behind",
        --   effect = "blur", memo = true, cut = "below" },
        -- { match = { focused = false }, part = "pane", slot = "front",
        --   effect = "shade", fade = { duration = 240, easing = "outCubic" } },
        --
        -- The first blurs a window's own pixels. The second, with no
        -- `source`, blurs what lies behind a translucent window: the layers
        -- below it (`cut = "background"`: the wallpaper alone, even where
        -- `xray.layers` holds "bottom"), blurred once per monitor and shared
        -- between windows (`memo = false`: each window blurs it alone). The
        -- third darkens the edge of every window without the keyboard, and
        -- its `fade` eases it in as the window loses the keyboard and out as
        -- it gets it back, over 240 ms on `outCubic` (0 to 60000 ms; linear,
        -- outCubic, outBack, inOutQuad, inOutCubic or spring): the
        -- compositor blends the slot's result at another opacity on each
        -- frame and runs nothing for it.
        --
        -- A list, so yours replaces this one whole. A set with a broken rule
        -- is refused whole, named on the overlay, and the rules that ran stay.
        -- A pane style's own rules, in its effects.lua, come first: yours win.
        rules = {},
        -- What an effect's own Lua may spend as it loads: `load_ms` of time
        -- for each call (10 to 5000), shared by the checks a geometry
        -- effect's `mesh` is put through, and `memory_mib` of memory (1 to
        -- 512). A folder past either is named on the overlay and not loaded.
        sandbox = { load_ms = 100, memory_mib = 16 },
        -- How many numbers an effect's params may be when a geometry or a
        -- pixels effect packs them (1 to 64); one with more is named on the
        -- overlay, never cut.
        limits = { params = 8 },
        -- What a `sol.present` deform draws when its mesh is refused or its
        -- folder is missing (`failed`: "flat", the window undeformed, or
        -- "hide"), and when a reload finds its folder changed mid-flight
        -- (`on_reload`: "flat", or "keep", the version it began with); a
        -- reload that leaves the folder as it was changes nothing, even one
        -- that changes `sandbox`. A deform may say either for itself.
        present = { failed = "flat", on_reload = "flat" },
        -- What T2 effects read as the backdrop: a sharp picture of these
        -- layers, per monitor, rebuilt only when it changes. It always holds
        -- "background"; "bottom" beside it adds desktop widgets, at the cost
        -- of a rebuild each time one repaints.
        -- `scale = 0.5` builds it at half resolution for good (0.25 to 1). A
        -- picture nothing reads is kept `keep_ms` (0 to 60000) before it is
        -- given back. A window across two monitors reads each monitor's
        -- picture (`straddle = "each"`, a seam at the edge) or its own
        -- monitor's ("own"). A tilted or genied window shows the backdrop
        -- where each of its pixels is drawn (`warped = "drawn"`), or,
        -- cheaper, the backdrop where it rests, bent with it ("rest"). A
        -- window that glides or is dragged always reads it where it is drawn.
        xray = { layers = { "background" }, scale = 1, keep_ms = 2000, straddle = "each", warped = "drawn" },
        -- The part of an effect that reads only the backdrop (a blur) runs
        -- once per monitor and is shared: `split = "resize"` shares it up to
        -- its last pass that changes size (a tint after it stays per
        -- window), "longest" shares the tint too, "off" shares nothing.
        -- A shared result nobody reads is kept `keep_passes` passes (0 to
        -- 3600); `budget_mib` caps what they hold (0 is no cap; 0 to 4096),
        -- past which a window runs its whole effect itself, and a held one
        -- past a lowered cap is given back.
        memo = { split = "resize", keep_passes = 0, budget_mib = 0 },
        -- While a window glides, is dragged or changes size, an effect's own
        -- steps for it run again at most `motion_hz` times a second: the
        -- glass's refraction as it moves, the shade as it changes size (10
        -- to 1000; 0 on every pass). Between, its last result is drawn where
        -- the window is, stretched to its size, so what shows through the
        -- glass trails the window by at most that long's worth of its motion,
        -- and everything is exact again within it once the window stops. A
        -- backdrop that changes, or new params, run it at once.
        motion_hz = 60,
    },

    -- Starting up. The first desktop frame waits until what it shows is
    -- ready (the wallpaper's blur, the shell's materials), so nothing pops
    -- in after it: at most `max_ms` (0 to 10000; 0 never waits), not at all
    -- with `hold = false`, and after every reload too with `on_reload = true`.
    -- With no effects there is nothing to wait for, and nothing waits.
    startup = {
        first_frame = { hold = true, max_ms = 2000, on_reload = false },
    },

    -- A minimized window (#30, `minimize.lua`): `super+m`, `sol.minimize`, or
    -- the application's own button. It goes into a slot at the bottom of its
    -- monitor with the genie, and comes back out of it when it is shown
    -- again. `motion` is how long that takes and on which curve, both ways;
    -- `spread` is how much of the window trails behind it, 0 pulling it in
    -- rigidly and larger values drawing the tail out; `slot` is the size of
    -- the strip it goes into, centred on the bottom edge of the monitor's
    -- work area. The Developer Tweaks panel's genie (`--debug-mode`) plays
    -- the same. A layout leaves a minimized window out and takes it back
    -- where it was; focusing it shows it again.
    minimize = {
        motion = { duration = 520, easing = "inOutCubic" },
        spread = 1.4,
        slot = { w = 120, h = 24 },
    },
}

-- A list is a table with a [1]; anything else with keys is a section to
-- descend into. Crude, and right for every shape in this file.
local function is_list(value)
    return type(value) == "table" and value[1] ~= nil
end

-- Sections whose key set is not the one written above them.
--
-- `merge` reports every key a user's file sets that these defaults do not
-- define -- which is what catches `tilling = { ... }`. Three sections would be
-- reported wrongly by that rule, and each needs saying out loud rather than
-- being quietly skipped:
--
--   keyboard, cursor   their xkb and cursor keys are left out on purpose,
--                      because leaving them out *means* "whatever the session
--                      already said" -- see their comments. Those key sets
--                      belong to `sol.keyboard` and `sol.cursor_theme` in the
--                      compositor, so they are written out here: the defaults
--                      cannot carry them without changing what leaving them
--                      out means. `keyboard.indicator` is the one key the
--                      defaults do define, and is checked against them.
--   bindings           open by construction. A key combination is whatever you
--                      press, so there is no list to check one against.
--
-- **This table is checked against the compositor, and was wrong when it
-- shipped.** #117 wrote here that a reader growing a key and this list not
-- being updated to match would report that key as a typo, and called that
-- failure loud. It was not loud at all: `active` was missing from the day this
-- was written, `sol.keyboard` has always read it, `init.lua` passes it straight
-- through, and so `keyboard = { layout = "us,ru", active = 2 }` -- the only
-- shape of configuration that has an `active` worth naming -- was told its
-- working setting is read by nothing, and `solium --check` exited 1 over it.
-- Which is worse than the silence #117 replaced: `--check` is sold as "did my
-- configuration work", and one wrong answer there teaches people to stop
-- reading it.
--
-- A hand-restated list drifts, so a comment asking the next person to remember
-- is not the fix. `every_key_a_section_accepts_is_one_the_compositor_reads` in
-- `script.rs` hands each of these functions a table that records what is looked
-- up in it and compares the two sets, in both directions: a key only the
-- compositor reads is a working setting called a typo, and a key only this list
-- has is #117's own fault from the other side.
local open_sections = {
    keyboard = {
        rules = true,
        model = true,
        layout = true,
        variant = true,
        options = true,
        repeat_rate = true,
        repeat_delay = true,
        -- Which of several layouts is live. `sol.keyboard{ active = 2 }` is
        -- also how the cycle binding in `init.lua` switches it.
        active = true,
        -- Caps Lock and Num Lock, on or off. `sol.keyboard{ caps = false }`
        -- from a binding turns Caps Lock off.
        caps = true,
        num = true,
    },
    cursor = { theme = true, size = true, scene = true },
    -- `click` and `follow` are deliberately absent from `focus` above, so
    -- that leaving either out of a `user.lua` means "nil", not "zero" --
    -- the same reason `keyboard` and `cursor` need naming here rather than
    -- being read off their own defaults.
    focus = { click = true, follow = true },
    -- Mode names are open-ended, the same as a key combination: a mode
    -- outside this file can register any name with `modes.register` and
    -- give `focus.modes`, `focus.after_close` and `resize.modes` entries of
    -- its own.
    ["focus.modes"] = true,
    ["focus.after_close"] = true,
    ["resize.modes"] = true,
    -- `true` rather than a set of names: everything is accepted.
    bindings = true,
}

-- The one key a user may write that these defaults deliberately do not define.
-- See the read-across at the bottom of this file.
--
-- Accepted, and deliberately never *suggested*: `nearest` is not shown this
-- table. It used to be, and so `decoraton` was answered with "did you mean
-- decoration", which sends somebody to correct a typo into a spelling that is
-- deprecated -- past the key that actually configures this, which is `pane`. An
-- alias exists so a file written before the rename keeps working, not so a new
-- file can be steered into using it.
local legacy = { decoration = true }

-- Keys the user's file set that nothing here defines, as dotted paths.
local unrecognised = {}

-- How many single-character edits turn one word into the other.
--
-- Plain Levenshtein, two rows. Cheap enough not to think about: it runs only
-- on a key that is already known to be wrong, against a handful of candidates
-- of a dozen characters each, once per configuration load.
local function distance(from, to)
    local previous = {}
    for column = 0, #to do
        previous[column] = column
    end
    for row = 1, #from do
        local current = { [0] = row }
        for column = 1, #to do
            local substitution = previous[column - 1]
            if from:sub(row, row) ~= to:sub(column, column) then
                substitution = substitution + 1
            end
            current[column] = math.min(previous[column] + 1, current[column - 1] + 1, substitution)
        end
        previous = current
    end
    return previous[#to]
end

-- The nearest key that does exist, when one is near enough to be worth naming.
--
-- Two edits, which is `tilling` to `tiling` and `scal` to `scale` but not
-- `dock` to `pane`. A suggestion that is wrong is worse than none: it sends
-- somebody to rewrite a line that was not the problem.
local function nearest(key, base, accepted)
    local best, best_distance = nil, 3
    local function consider(candidate)
        if type(candidate) ~= "string" then
            return
        end
        local apart = distance(key:lower(), candidate:lower())
        if apart < best_distance then
            best, best_distance = candidate, apart
        end
    end
    for candidate in pairs(base) do
        consider(candidate)
    end
    if type(accepted) == "table" then
        for candidate in pairs(accepted) do
            consider(candidate)
        end
    end
    return best
end

-- Merge `over` onto `base`, and say what could not have been meant.
--
-- `path` is where we are, for the report; `accepted` is the extra key set in
-- force here, from `open_sections` -- `true` for a section that accepts
-- anything, a table of names for one whose names live elsewhere, `nil` for the
-- ordinary case where `base` itself is the list of what exists.
--
-- `legacy` is read straight out of the upvalue rather than handed in as an
-- `accepted` set, because the two are accepted for opposite reasons: an
-- `open_sections` name is the current spelling and is worth suggesting, and a
-- legacy name is the old one and is not.
--
-- Checking on the way *in*, before the assignment, because after it the key
-- exists in `base` and the evidence is gone.
local function merge(base, over, path, accepted)
    for key, value in pairs(over) do
        local where = path and (path .. "." .. tostring(key)) or tostring(key)
        -- `legacy` is consulted here and not passed to `nearest`: an old
        -- spelling still works, and is still not what to suggest. Only at the
        -- top level, which is where the alias is.
        local known = accepted == true
            or base[key] ~= nil
            or (accepted and accepted[key])
            or (path == nil and legacy[key])
        if not known then
            local meant = nearest(tostring(key), base, accepted)
            -- Qualified the same way the key is, so the suggestion is something
            -- you can paste: inside `tiling` the fix is `tiling.split`, not
            -- `split`.
            if meant and path then
                meant = path .. "." .. meant
            end
            unrecognised[#unrecognised + 1] = { key = where, meant = meant }
        end
        if type(value) == "table" and type(base[key]) == "table"
            and not is_list(value) and not is_list(base[key]) then
            merge(base[key], value, where, open_sections[where])
        else
            base[key] = value
        end
    end
    return base
end

-- `focus.modes` keeps the shipped entries apart from yours (#128). Merged in
-- place, the two could not be told apart, and a shipped entry outranked your
-- own `focus.follow`: `focus = { follow = false }` still followed the pointer
-- in tiling. So the shipped entries go behind an empty table, which your
-- file's entries are merged into: a plain read finds either --
-- `config.focus.modes.tiling` is "follow" until you say otherwise -- while
-- `rawget`, and `pairs`, find only yours. `modes.lua`'s `follow_for` ranks
-- them. `your_mode_entry_then_your_focus_follow_then_the_shipped_entry_then_the_machine`.
defaults.focus.modes = setmetatable({}, { __index = defaults.focus.modes })

-- `require` searches the user's directory first, so this finds
-- ~/.config/solium/user.lua when there is one and nothing when there is not.
--
-- Having no user file and having a broken one both make `require` fail, and
-- they must not be treated alike: a typo that silently changes nothing is the
-- worst way to lose an afternoon. Only "no such module" is quiet; anything
-- else is raised, so `solium --check` reports it and a reload keeps whatever
-- was already running.
local found, user = pcall(require, "user")
if found then
    if type(user) == "table" then
        merge(defaults, user)
    else
        error("user.lua must return a table, got " .. type(user), 0)
    end
elseif not tostring(user):match("module 'user' not found") then
    error(tostring(user), 0)
end

-- Say what was merged and will never be read.
--
-- `merge` used to validate nothing at all, so a `user.lua` saying `tilling`
-- gained a section by that name, read by nothing, for ever -- and `solium
-- --check`, the one command whose job is answering "did my configuration
-- work", said it loaded fine. It had loaded fine. It was not doing what the
-- file said, and there was no way to tell those two apart from the outside
-- (#117).
--
-- Reported rather than refused. A key nothing reads has no effect by
-- definition, so raising here would trade a setting that does nothing for a
-- session that does not start -- and on a reload, for a session that keeps the
-- *old* configuration while you correct a spelling. `sol.unknown` puts each
-- one in the log for a running session and in front of `solium --check`, which
-- exits non-zero when there is one.
--
-- What this cannot see, stated rather than implied:
--
--   * Lists are replaced whole and never descended into, so the keys inside a
--     `monitors` entry are not checked here. `mode` misspelled as `moed` is
--     merged as written and the monitor keeps its default mode.
--   * `keyboard`, `cursor` and `bindings` are the sections above; the first
--     two are checked against a list this file restates, the other two
--     against nothing: `bindings` because any key combination is one, and
--     `focus.modes`, `focus.after_close` and `resize.modes` because mode
--     names are open-ended the same way (a mode outside this file can
--     register any name). `focus.modes.folating` merges in silently, exactly
--     like a misspelled binding.
--   * A value of the wrong *type* is not this check's business. `gap = "12"`
--     is a recognised key and passes.
for _, entry in ipairs(unrecognised) do
    sol.unknown(entry.key, entry.meant)
end

-- `pane` was called `decoration` when a style was a single QML file rather
-- than a folder. Merging a user.lua that sets the old key leaves `pane` at its
-- default, so without this the setting would silently stop working -- which is
-- the worst way to lose an afternoon, and the thing the alias exists to
-- prevent. Read across only when the new name was not also given: someone who
-- wrote both meant the one they had to look up.
if found and type(user) == "table" and user.decoration ~= nil and user.pane == nil then
    defaults.pane = user.decoration
end

-- And the old key does not survive into the table the compositor reads.
-- `merge` above copied it in, so without this the configuration carries both
-- `pane` and a `decoration` that is no longer a setting -- inert today, and
-- exactly what a later pass validating `pairs(config)` would flag against a key
-- the compositor itself wrote. Unconditional: the read-across above takes its
-- value from `user`, never from here.
defaults.decoration = nil

return defaults
