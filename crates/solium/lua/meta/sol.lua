---@meta sol

-- The `sol` table, described for lua-language-server.
--
-- This file is never run. The compositor builds `sol` itself before it loads
-- the configuration (`build_api` in `crates/solium/src/script.rs`); this is a
-- description of what that table holds, so an editor can complete it and check
-- the arguments you pass. Point lua-language-server's `workspace.library` at
-- the folder this file is in; docs/ricing.md says how.
--
-- It is also the source of the Lua API page on the documentation site, and a
-- test (`sol_lua_documents_exactly_the_api_the_compositor_registers`) fails
-- when the compositor registers a name this file does not describe, or this
-- file describes a name the compositor does not have.
--
-- Most calls queue a command and return nothing. The commands a handler queues
-- are applied together when it returns, in the order it queued them, and every
-- animated one uses the timing `sol.animate` last set in that handler.

-- Shapes ---------------------------------------------------------------------

---A rectangle in the one global coordinate space every monitor is a window
---onto, in logical pixels.
---@class sol.Rect
---@field x number
---@field y number
---@field w number
---@field h number

---A size in logical pixels.
---@class sol.Size
---@field w number
---@field h number

---A point in the global coordinate space.
---@class sol.Point
---@field x number
---@field y number

---One row of `sol.windows()`: a window a layout may arrange, as it was at the
---moment the handler was called. `x`, `y`, `w` and `h` are where the window
---lives, frame included.
---@class sol.Window: sol.Rect
---@field id integer The window's id, which every other call takes.
---@field title string The window's title; the program's name until its application arrives.
---@field focused boolean Whether it has the keyboard.
---@field monitor string The name of the monitor it is on.
---@field modal boolean Whether it says it is a modal dialog.
---@field parent? integer|false The window it belongs to; `false` when the client named a parent that is not on screen, absent when it named none.
---@field leaving boolean Whether it is closing: between `closing` and `close` or `refused`.
---@field app_id string The application's own name: a Wayland `app_id`, an X11 `WM_CLASS` class. Empty until the application arrives.
---@field min? sol.Size The smallest the application says it can be, frame included; absent when it set no limit.
---@field max? sol.Size The largest the application says it can be, frame included; absent when it set no limit.
---@field cramped boolean Whether the layout last placed it with `cramped = true`.
---@field shown boolean Whether its application has shown its first frame yet.

---One row of `sol.monitors()`. `x`, `y`, `w` and `h` are the work area: the
---monitor less the room layer-shell clients have reserved.
---@class sol.Monitor: sol.Rect
---@field name string The connector name, such as `"DP-1"`.
---@field scale number Device pixels per logical pixel.
---@field focused boolean Whether the pointer is on it.
---@field primary boolean Whether it is the primary monitor.
---@field transform string Its rotation, as Smithay names it in lower case: `"normal"`, `"_90"`, `"_180"`, `"_270"`, `"flipped"`, `"flipped90"`, `"flipped180"` or `"flipped270"`. Not the spelling `sol.monitors{ ... }` takes.
---@field power "on"|"off" Whether its display is on. A monitor that is off keeps its place, its work area and its windows.
---@field whole sol.Rect The whole monitor, which is what a wallpaper or a fullscreen window covers.

---How one monitor should be driven and where it goes, for `sol.monitors{ ... }`.
---Every key but `name` is optional. `config.lua`'s `monitors` section explains
---each one at length.
---@class sol.MonitorPlacement
---@field name string The connector name. `solium --probe` lists the ones the machine has.
---@field x? integer The top-left corner, in the global space.
---@field y? integer The top-left corner, in the global space.
---@field right_of? string Beside the monitor with this name.
---@field left_of? string Beside the monitor with this name.
---@field above? string Beside the monitor with this name.
---@field below? string Beside the monitor with this name.
---@field align? "start"|"centre"|"end" Which way the other axis lines up when placed beside a monitor of another size. `"centre"` is the default.
---@field mode? string|{ w: integer, h: integer, refresh?: integer } `"2560x1440@165"`, `"2560x1440"`, or one of `"best"` (the default), `"preferred"` and `"widest"`.
---@field vrr? boolean Variable refresh rate, where the monitor and the driver offer it.
---@field transform? string|integer Anticlockwise: `"normal"`, `"90"`, `"180"`, `"270"`, `"flipped"`, `"flipped-90"`, `"flipped-180"` or `"flipped-270"`. A number is read as degrees.
---@field enabled? boolean `false` does not drive the monitor at all.
---@field primary? boolean The monitor things belonging to one screen go on.
---@field scale? number|"auto" Device pixels per logical pixel, from 0.5 to 8. Left out, it is worked out from the panel's size.

---The keyboard as `sol.keyboard()` answers.
---@class sol.KeyboardState
---@field layouts string[] The layout names, in order.
---@field active integer Which of them is live, counting from 1.
---@field repeat_rate integer Repeats per second.
---@field repeat_delay integer Milliseconds a key is held before it repeats.

---What `sol.keyboard{ ... }` may change. Every key is optional and one left
---out is left as it is. The first five are xkb names; setting any of them
---compiles a new keymap, and one left blank falls back to the matching
---`XKB_DEFAULT_*` environment variable.
---@class sol.KeyboardOptions
---@field rules? string
---@field model? string
---@field layout? string A comma-separated list, such as `"us,ua"`.
---@field variant? string One per layout, in the same order.
---@field options? string xkb options, comma separated, such as `"grp:alt_shift_toggle"`.
---@field repeat_rate? integer Repeats per second.
---@field repeat_delay? integer Milliseconds before repeating starts.
---@field active? integer Which layout is live, counting from 1.

---A QML scene for `sol.surface` to draw.
---@class sol.SurfaceOptions
---@field scene string A file name looked up in `~/.config/solium/qml` and then in the shipped QML, or a path (absolute, or starting with `~/`).
---@field layer? sol.Layer Which layer it is drawn in. `"background"` is the default.
---@field on? "primary"|"every-monitor"|string|sol.Rect `"every-monitor"` (the default) draws one instance per monitor, filling it; `"primary"` one on the primary monitor; a monitor's name one there; a rect one at that rect.
---@field properties? table Values for the scene's properties, handed over as JSON: strings, numbers, booleans and tables of those. A function, userdata or non-finite number is left out.
---@field interactive? boolean Whether the pointer reaches it. An interactive scene sets its `action` property, and `sol.on("surface", ...)` hears it.

---@alias sol.Layer
---| "background" # Under everything, including client background surfaces.
---| "bottom" # Above the background, below windows.
---| "top" # Above windows, below client top surfaces, and below a fullscreen window that covers the top layer.
---| "overlay" # Above client top surfaces, below client overlay ones and the pointer.

---A curve by name, or four numbers: the control points of a cubic bezier, as
---CSS writes `cubic-bezier(x1, y1, x2, y2)`. An unknown name is logged and the
---default kept.
---@alias sol.Easing
---| "linear"
---| "outCubic"
---| "outBack"
---| "inOutQuad"
---| "inOutCubic"
---| "spring"
---| number[]

---How long an animation takes and how it moves.
---@class sol.Motion
---@field duration? integer Milliseconds.
---@field easing? sol.Easing

---Where and how `sol.present` draws a window. A key left out says nothing: with
---no rect the window is drawn at its own, with no opacity it is opaque, and
---with no rotation it is flat.
---@class sol.PresentOptions
---@field x? number The rect it is drawn at: all four of `x`, `y`, `w` and `h`, or none.
---@field y? number
---@field w? number
---@field h? number
---@field opacity? number From 0 to 1.
---@field rotate_x? number Degrees about the horizontal axis.
---@field rotate_y? number Degrees about the vertical axis.
---@field rotate_z? number Degrees in the plane of the screen.
---@field perspective? number The viewer's distance in pixels. Without it a rotation is orthographic.
---@field pivot_x? number What the rotation turns about, as a share of the width. 0.5 is the default; outside 0..1 is kept.
---@field pivot_y? number What the rotation turns about, as a share of the height. 0.5 is the default; outside 0..1 is kept.
---@field z? number Draw order and nothing else: higher is in front, equal keeps the stacking order, 0 is the default. Input still follows the rect.
---@field deform? sol.Deform A deformation of the window's mesh.

---A deformation, named by effect. Its other keys are that effect's parameters.
---@class sol.Deform
---@field effect "genie" The effect, from `crates/effects`. An unknown one is logged and the window drawn undeformed.
---@field to { window: integer }|{ surface: string }|sol.Rect What the window is pulled into or drawn out of. A window or a surface is followed as it moves; a rect stays where it is.
---@field axis? "down"|"up"|"left"|"right" For the genie: which edge leads.
---@field spread? number For the genie: how much of the window is in motion at once. 0 is rigid.

---Where `sol.place` puts a window.
---@class sol.PlaceOptions: sol.Rect
---@field tile? boolean|sol.Rect `true` (the default) makes the rect a tile the window is held inside; `false` places it without tiling it; a rect is the tile, for a window placed smaller than its tile.
---@field cramped? boolean The layout saying the tile is smaller than the window's own minimum. `sol.windows()` reports it back.

---What a selection holds, for `sol.group`. Every key is optional.
---@class sol.Selection
---@field windows? integer[] Windows, by id.
---@field surfaces? string[] Surfaces, by the name `sol.surface` gave them.
---@field monitors? string[] Everything drawn on these monitors.
---@field monitor? string Which instance of each surface above: the one on this monitor.

---What `sol.present_group` does to a selection. `x` and `y` are a
---displacement, not a destination, and the rest compose with each member's own
---transform.
---@class sol.Shift
---@field x? number
---@field y? number
---@field opacity? number
---@field rotate_x? number
---@field rotate_y? number
---@field rotate_z? number
---@field perspective? number

---What a window does before its application arrives. Every key is optional
---and one left out keeps the compositor's default.
---@class sol.LoadingOptions
---@field scene? string The QML scene: a name in `qml/loading`, the user's first, or a path.
---@field patience? integer Milliseconds to wait for an application that never arrives.
---@field reserves_a_slot? boolean Whether the window takes its place in the layout at once.
---@field decorated? boolean Whether its frame is drawn while it waits.
---@field fade? integer Milliseconds the scene takes to fade off the application.

---The idle blank. A key left out, or one that is not a number of 0 or more,
---keeps the default.
---@class sol.IdleOptions
---@field screens_off_after? number Seconds with nobody at the machine before every screen is turned off. 0 never does.
---@field off_frame_interval? number How often, in milliseconds, a window on a screen that is off is still told it may draw. 0 stops it.

---The pointer's theme. A key left out, or no table, means the configuration
---did not say, and `XCURSOR_THEME` and `XCURSOR_SIZE` have their turn.
---@class sol.CursorOptions
---@field theme? string The name of an XCursor theme.
---@field size? integer Logical pixels, from 8 to 256.

---How QML is rendered. Read once, before Qt starts.
---@class sol.QmlOptions
---@field renderer? "auto"|"gpu"|"software" `"auto"` tries the GPU in a child process first.
---@field probe_timeout? integer Milliseconds the GPU trial may take.

---Telling systemd and D-Bus that Solium is the session. A key left out, or
---one of the wrong kind, keeps the default.
---@class sol.SessionOptions
---@field systemd? boolean Export the environment to systemd and D-Bus activation and start `solium-session.target`; undone on exit. `true` by default.
---@field autostart? boolean Start `solium-autostart.target`, XDG autostart, beside it. `true` by default.
---@field stop_timeout? integer Milliseconds SIGTERM, SIGINT or SIGHUP waits for a clean stop before Solium ends at once. 5000 by default.

---One entry of `sol.decorations()`.
---@class sol.DecorationEntry
---@field name string What `sol.pane` takes.
---@field kind "bundle"|"file" A folder under `qml/panes`, or a single QML file under `qml/decorations`.

---What the layout calls take: the work area to arrange in, and how.
---@class sol.LayoutOptions: sol.Rect
---@field gap? number Space between windows and around the edge. 12 by default.
---@field ratio? number The share of the width the main window takes in `master_stack`. 0.6 by default.
---@field column? number The share of the width a column takes in `scrolling`. 0.5 by default.
---@field padding? number Space around each thumbnail in `grid`. 24 by default.
---@field split? number Where a dwindle split falls, as a share of the tile divided. 0.5 by default.
---@field minimum? sol.Size The smallest a dwindle tile may be. None by default.
---@field floors? table<integer, sol.Size> Each window's own floor, by id, frame included.
---@field offset? number How far the view has scrolled, for `strip`, `strip_scroll_to`, `scrolling` and `scroll_to`.

---A rect a tree or a strip hands back, tagged with its window.
---@class sol.Slot: sol.Rect
---@field id integer
---@field cramped? boolean `true` on a tile smaller than its window's own floor; trees only.

---One column of a strip, for `sol.layout.strip`.
---@class sol.Column
---@field width? number The column's width as a share of the view. 0.5 by default.
---@field windows? integer How many windows share the column. 1 by default.

---What `sol.on` can listen for.
---@alias sol.Event
---| "open" # A window's life began: `(id)`.
---| "focus" # The keyboard moved to a window: `(id)`.
---| "closing" # A close was asked for and the window is fading: `(id)`.
---| "refused" # Its application declined, and the window is back: `(id)`.
---| "close" # The window is gone: `(id)`.
---| "drop" # A dragged window was let go: `(id, x, y)`.
---| "resize" # An edge is being dragged: `(id, edge_x, edge_y, horizontal_side, vertical_side)`.
---| "scroll" # The wheel turned with Super held: `(dx, dy)`.
---| "click" # A press while a script holds input: `(x, y)`.
---| "surface" # An interactive surface set its `action`: `(name, action)`.
---| "layout" # Arrange the windows you already hold again: `()`.
---| "monitors" # The monitors changed, or were announced at startup or after a reload: `()`.
---| "restore" # These scripts replaced a running session's, after a reload and never at startup: `()`.

-- sol ------------------------------------------------------------------------

---The compositor's scripting interface. Built by the compositor before the
---configuration runs; there is nothing to require.
---@class sol
sol = {}

---State that survives `super+shift+r`.
---
---Answers the table the last configuration kept under `name`, or `defaults` on
---the first load. Mutate it in place and the next reload gets what you left in
---it. It may hold numbers, strings, booleans, tables of those, and the trees
---and strips `sol.layout` makes, eight tables deep; anything else is named in
---the log and left out. Asked for twice under one name, it is the same table
---both times.
---@param name string
---@param defaults table
---@return table
function sol.keep(name, defaults) end

---Every window a layout may arrange, topmost first.
---
---A snapshot taken for this handler, not a live view. A window that closes
---while you hold its id is ordinary: `place` and `present` on an id that no
---longer exists do nothing.
---@return sol.Window[]
function sol.windows() end

---Draw a QML scene: a wallpaper, a bar, a dock, a heads-up display.
---
---Declaring a name again replaces what it declared, so running the
---configuration again is harmless. `false` or `nil` takes the surface away. A
---scene that cannot be found is logged and nothing is drawn.
---@param name string
---@param options sol.SurfaceOptions|false|nil
---@return nil
function sol.surface(name, options) end

---Read the keyboard, or change it.
---
---With no argument, answers the layouts, which one is live and how keys
---repeat. With a table, changes what it names and leaves the rest alone, so a
---binding that switches layout does not reset the repeat rate.
---@overload fun(): sol.KeyboardState
---@param options? sol.KeyboardOptions
---@return sol.KeyboardState|nil
function sol.keyboard(options) end

---Read the monitors, or arrange them.
---
---With no argument, answers every monitor. With a list, drives and places the
---monitors it names; a row with no name is logged and skipped.
---@overload fun(): sol.Monitor[]
---@param rows? sol.MonitorPlacement[]
---@return sol.Monitor[]|nil
function sol.monitors(rows) end

---Turn a monitor's display off or on, or every monitor's with `"all"`.
---
---The monitor keeps its place, its work area and its windows. Any key, click,
---scroll, touch or pointer motion turns every screen back on. A mode that is
---neither `"on"` nor `"off"` is an error; a name no monitor has is a line in
---the log.
---@param which string A monitor's name, or `"all"`.
---@param mode "on"|"off"
---@return nil
function sol.monitor_power(which, mode) end

---Set the idle blank: `config.idle`, which the shipped `init.lua` hands over.
---@param options? sol.IdleOptions
---@return nil
function sol.idle(options) end

---The work area of the monitor a window is on, or of the active monitor when
---no window is named or the id is unknown.
---@param id? integer
---@return sol.Rect
function sol.monitor(id) end

---Where the pointer is.
---@return sol.Point
function sol.cursor() end

---The window drawn at a point, asked the way the compositor asks it: against
---where windows are drawn, on the monitors that draw them.
---
---`skip` leaves one window out, which is what makes it useful while dragging:
---the dragged window is always the one under the pointer.
---@param x number
---@param y number
---@param skip? integer
---@return integer|nil id
function sol.window_at(x, y, skip) end

---Draw a window somewhere other than where it lives, animated.
---
---A transform: the window keeps its place and its client is told nothing, so
---`sol.present_clear` puts it back exactly. Input follows the rect it is drawn
---at.
---@param id integer
---@param options? sol.PresentOptions
---@return nil
function sol.present(id, options) end

---Draw a window at a rect and animate it to where it lives: every "appears from
---somewhere" animation. The rect is required.
---@param id integer
---@param options { x: number, y: number, w: number, h: number, opacity?: number }
---@return nil
function sol.present_from(id, options) end

---Whether the compositor was started with `--debug-mode` or with
---`SOLIUM_DEBUG_MODE` set. The Developer Tweaks panel asks this.
---@return boolean
function sol.debug_mode() end

---Read the configuration again, without ending the session.
---
---A configuration that fails to load leaves the running one in place. See
---`sol.keep` and the `restore` event for what a reload carries.
---@return nil
function sol.reload() end

---Choose how every window is framed, at once.
---
---A name is a folder under `~/.config/solium/qml/panes` or the shipped
---`qml/panes`, or a single file under `~/.config/solium/qml/decorations`; a
---path is anyone's; `"none"` draws no frame. `SOLIUM_PANE`, when set, wins over
---the name given here.
---@param name? string
---@return nil
function sol.pane(name) end

---The old name for `sol.pane`, from when a style was a single QML file. The
---same function.
sol.decoration = sol.pane

---Every frame style `sol.pane` could be given, found in the folders the
---compositor looks in: sorted, bundles first, each name once.
---@return sol.DecorationEntry[]
function sol.decorations() end

---Set what a window does between being asked for and its application arriving.
---@param options sol.LoadingOptions
---@return nil
function sol.loading(options) end

---Set whose own size limits a floating window's edge drag is held to.
---
---`floating = "ignore"` lets the drag go wherever the pointer does; anything
---else believes every application. `ignore` lists applications, by `app_id`,
---whose limits are never believed.
---@param options? { floating?: "respect"|"ignore", ignore?: string[] }
---@return nil
function sol.client_sizes(options) end

---Set what fills a window while a resize drag is ahead of its client.
---
---`"stretch"` (the default) scales the last picture; `"hold"` leaves it at its
---own size; `"scene"` draws the window's QML scene, which today reaches only a
---window resized before its application painted. Another name is logged and
---the default kept.
---@param options? { fill?: "stretch"|"hold"|"scene" }
---@return nil
function sol.resize(options) end

---Set what a fullscreen window covers.
---
---`"top"` (the default) covers the top layer, so bars go under it; `"none"`
---leaves bars over it. Another name is logged and the default kept.
---@param options? { covers?: "top"|"none" }
---@return nil
function sol.fullscreen(options) end

---Set the pointer's XCursor theme and size. Applied at once, so a reload is
---how a theme is tried. Not `sol.cursor`, which says where the pointer is.
---@param options? sol.CursorOptions
---@return nil
function sol.cursor_theme(options) end

---Choose how QML renders, and how long the GPU trial may take.
---
---Read once, when the compositor starts, before any scene does; a reload does
---not change it. `--qml`, `SOLIUM_QML` and `SOLIUM_QML_GPU` override it.
---@param options? sol.QmlOptions
---@return nil
function sol.qml(options) end

---Say how this session is announced, and how long a signal to end waits.
---
---Read once, when the compositor starts; a reload does not change it. Who is
---told: the session bus, when Solium is started as the session with
---`solium --tty --session`; nobody, for a `--tty` start by hand or a nested
---run, unless `SOLIUM_SESSION_BUS` names a bus to tell instead. The shipped
---`init.lua` calls it with `config.session`.
---@param options? sol.SessionOptions
---@return nil
function sol.session(options) end

---Stop presenting a window: animate it back to where it lives.
---@param id integer
---@return nil
function sol.present_clear(id) end

---Name a selection of windows, surfaces and monitors, so one transform can
---carry them together.
---
---Declaring the name again replaces the membership and keeps the transform, so
---a mode that rebuilds its groups on every pass does not restart its own
---animation. `false` or `nil` takes the name away.
---@param name string
---@param selection sol.Selection|false|nil
---@return nil
function sol.group(name, selection) end

---Move, fade or turn a selection as one.
---
---Composes with each member's own `sol.present`, so a window tilted inside a
---moving desk stays tilted within it. `motion` sets this call's own timing;
---left out, it is whatever `sol.animate` set.
---@param name string
---@param shift? sol.Shift
---@param motion? sol.Motion
---@return nil
function sol.present_group(name, shift, motion) end

---Animate a selection back to where its members live.
---@param name string
---@param motion? sol.Motion
---@return nil
function sol.present_group_clear(name, motion) end

---Set the timing for every animated command queued after this one in the same
---handler. Left unset, it is 220 ms and `"outCubic"`.
---@param options sol.Motion
---@return nil
function sol.animate(options) end

---Give a window the keyboard.
---@param id integer
---@return nil
function sol.focus(id) end

---Put a window where it lives: the layout's call.
---
---The client is told the size and asked to redraw. The rect is a tile unless
---`tile = false`: a client that will not shrink to it is cut to it rather than
---drawn over its neighbours. A rect is required.
---@param id integer
---@param options sol.PlaceOptions
---@return nil
function sol.place(id, options) end

---Let a window go from its tile, when a layout stops arranging it.
---`modes.use` sends this for every window when the layout in charge changes.
---@param id integer
---@return nil
function sol.unplace(id) end

---Ask a window to close. A request, not a kill: its application may refuse.
---@param id integer
---@return nil
function sol.close(id) end

---Start a program, with its arguments as separate strings:
---`sol.spawn("foot", "-e", "htop")`.
---@param program string
---@param ... string
---@return nil
function sol.spawn(program, ...) end

---Whether a program can be found on `PATH`. Nothing is run.
---@param program string
---@return boolean
function sol.which(program) end

---End the session. `Ctrl+Alt+Backspace` does the same and cannot be rebound.
---@return nil
function sol.quit() end

---Take input away from clients, or give it back.
---
---While a script holds it, keys it has not bound are swallowed, pointer
---presses go to `click` listeners, and focus stops following the pointer.
---Release it when you leave, or nothing reaches a window again.
---@param grabbed boolean
---@return nil
function sol.grab_input(grabbed) end

---Say which mode is in charge. The compositor keeps the text and logs a change
---at debug level; nothing on screen shows it.
---@param text string
---@return nil
function sol.status(text) end

---Bind a key combination to a function.
---
---The combination is normalised, so `super+shift+q` and `Shift+Super+Q` are
---one binding. A later call for the same combination replaces the earlier one.
---`note` is a short phrase saying where the binding came from, which
---`solium --check` prints beside it.
---@param combo string
---@param handler fun()
---@param note? string
---@return nil
function sol.bind(combo, handler, note) end

---Whether a combination is bound, in whatever spelling it is given.
---@param combo string
---@return boolean
function sol.bound(combo) end

---Take a binding away. With a `note`, `solium --check` lists the combination as
---removed by the configuration.
---@param combo string
---@param note? string
---@return nil
function sol.unbind(combo, note) end

---Report a setting nothing reads. `config.lua` calls this for each key of a
---`user.lua` its defaults do not define; `solium --check` lists them and exits
---with 1. `meant` is a near-miss suggestion.
---@param key string
---@param meant? string
---@return nil
function sol.unknown(key, meant) end

---Listen for an event. Listeners are added, never replaced, and one that fails
---is logged while the others still run.
---@overload fun(event: "open"|"focus"|"closing"|"refused"|"close", handler: fun(id: integer))
---@overload fun(event: "drop", handler: fun(id: integer, x: number, y: number))
---@overload fun(event: "resize", handler: fun(id: integer, edge_x: number, edge_y: number, horizontal_side: "left"|"right"|nil, vertical_side: "top"|"bottom"|nil))
---@overload fun(event: "scroll", handler: fun(dx: number, dy: number))
---@overload fun(event: "click", handler: fun(x: number, y: number))
---@overload fun(event: "surface", handler: fun(name: string, action: string))
---@overload fun(event: "layout"|"monitors"|"restore", handler: fun())
---@param event sol.Event
---@param handler function
---@return nil
function sol.on(event, handler) end

---Write a line to the compositor's log, at info level.
---@param message string
---@return nil
function sol.log(message) end

---Internal: the bindings, by normalised combination. Use `sol.bind`,
---`sol.bound` and `sol.unbind`.
---@private
---@type table<string, fun()>
sol._bindings = {}

---Internal: the listeners, by event. Use `sol.on`.
---@private
---@type table<string, function[]>
sol._handlers = {}

---Internal: what `sol.keep` holds, by name.
---@private
---@type table<string, table>
sol._keeps = {}

---Internal: the notes `sol.bind` and `sol.unbind` were given, by combination.
---@private
---@type table<string, string>
sol._binding_sources = {}

---Internal: what `sol.unknown` reported.
---@private
---@type { key: string, meant?: string }[]
sol._unknown_settings = {}

-- sol.layout -----------------------------------------------------------------

---The standard arrangements, from `crates/layout`. The pure ones take a count
---or a list and a `sol.LayoutOptions` and answer rects; `tree` and `scroller`
---make an arrangement the script holds.
---@class sol.layout
sol.layout = {}

---One large window and the rest stacked beside it.
---@param count integer
---@param options sol.LayoutOptions
---@return sol.Rect[]
function sol.layout.master_stack(count, options) end

---A dwindle tree, held by the script that made it. Stateful because where a
---window lands depends on which window was split and where the pointer was.
---@return sol.Tree
function sol.layout.tree() end

---A scrolling strip of columns, held by the script that made it.
---
---Reads `widths` and `default_width` from the table given, which is meant to
---be `config.scrolling`; with none it uses the built-in widths.
---@param options? { widths?: number[], default_width?: integer }
---@return sol.Scroller
function sol.layout.scroller(options) end

---A row of columns, each holding a stack, with `options.offset` as how far the
---view has scrolled.
---@param columns sol.Column[]
---@param options sol.LayoutOptions
---@return sol.Rect[]
function sol.layout.strip(columns, options) end

---The view offset that brings column `index` (counting from 1) fully into
---view, moving as little as possible.
---@param index integer
---@param columns sol.Column[]
---@param options sol.LayoutOptions
---@return number offset
function sol.layout.strip_scroll_to(index, columns, options) end

---A row of `count` windows at `options.column` of the width each, with
---`options.offset` as how far the view has scrolled.
---@param count integer
---@param options sol.LayoutOptions
---@return sol.Rect[]
function sol.layout.scrolling(count, options) end

---The view offset that brings window `index` (counting from 1) of `count`
---fully into view in `sol.layout.scrolling`.
---@param index integer
---@param count integer
---@param options sol.LayoutOptions
---@return number offset
function sol.layout.scroll_to(index, count, options) end

---A grid of thumbnails, as close to square as the count allows. Each size keeps
---its aspect ratio inside its cell; overview uses this.
---@param sizes sol.Rect[]
---@param options sol.LayoutOptions
---@return sol.Rect[]
function sol.layout.grid(sizes, options) end

-- A dwindle tree -------------------------------------------------------------

---A dwindle tree from `sol.layout.tree()`. Every call that takes `options`
---reads `options.floors` first, and lays the tree out in `options` as the work
---area.
---@class sol.Tree
local Tree = {}

---Add a window by splitting `target`, or the tile under `x`, `y` when there is
---no target. Splits however small that leaves the halves.
---@param id integer
---@param target? integer
---@param x? number
---@param y? number
---@param options sol.LayoutOptions
---@return nil
function Tree:insert(id, target, x, y, options) end

---`insert`, refusing a split that would leave a tile under `options.minimum` or
---a window under its floor: the same side first, then the other axis. Answers
---`false` and leaves the tree alone when neither fits.
---@param id integer
---@param target? integer
---@param x? number
---@param y? number
---@param options sol.LayoutOptions
---@return boolean inserted
function Tree:insert_fitting(id, target, x, y, options) end

---Split the largest tile that has room for another window at
---`options.minimum`, or answer `false` and leave the tree alone.
---@param id integer
---@param options sol.LayoutOptions
---@return boolean inserted
function Tree:insert_largest(id, options) end

---Take a window out of the tree.
---@param id integer
---@return nil
function Tree:remove(id) end

---Move a seam from the keyboard: `by` is a signed share that grows the window
---when positive. The seam on the right or below is preferred, and the other
---one used only when there is none. Without `options` there is no minimum.
---@param id integer
---@param axis "width"|"height"
---@param by number
---@param options? sol.LayoutOptions
---@return nil
function Tree:resize(id, axis, by, options) end

---Move a seam from a drag: put the named side of the window at `edge_x` (for
---`"left"` and `"right"`) or `edge_y` (for `"top"` and `"bottom"`). Hand it
---what the `resize` event gave you. Clamped to 0.05..0.95 and to
---`options.minimum`.
---@param id integer
---@param edge "left"|"right"|"top"|"bottom"
---@param edge_x number
---@param edge_y number
---@param options sol.LayoutOptions
---@return nil
function Tree:drag_seam(id, edge, edge_x, edge_y, options) end

---Whether a window is in the tree.
---@param id integer
---@return boolean
function Tree:contains(id) end

---Every window in the tree, in tree order.
---@return integer[]
function Tree:windows() end

---Where every window in the tree goes, ready for `sol.place`.
---@param options sol.LayoutOptions
---@return sol.Slot[]
function Tree:layout(options) end

-- A scrolling strip ----------------------------------------------------------

---A strip of columns from `sol.layout.scroller()`. Every call that takes
---`options` reads `options.floors` first: a column is never laid out narrower
---than the widest floor in it.
---@class sol.Scroller
local Scroller = {}

---Open a window in a new column, to the right of the active one.
---@param id integer
---@param options sol.LayoutOptions
---@return nil
function Scroller:insert(id, options) end

---Add a window to the active column instead of beside it.
---@param id integer
---@param options sol.LayoutOptions
---@return nil
function Scroller:insert_into_column(id, options) end

---Take a window out of the strip.
---@param id integer
---@return nil
function Scroller:remove(id) end

---Focus a window, bringing its column into view.
---@param id integer
---@param options sol.LayoutOptions
---@return nil
function Scroller:focus_window(id, options) end

---Move focus `by` columns left (negative) or right, taking the view with it.
---@param by integer
---@param options sol.LayoutOptions
---@return nil
function Scroller:focus_sideways(by, options) end

---Move focus `by` windows up (negative) or down within the active column.
---@param by integer
---@return nil
function Scroller:focus_vertically(by) end

---Swap the active column with its neighbour `by` places away, carrying focus.
---@param by integer
---@param options sol.LayoutOptions
---@return nil
function Scroller:move_column(by, options) end

---Pull the next column's focused window into the active column.
---@return nil
function Scroller:consume() end

---Push the focused window out into a column of its own.
---@param options sol.LayoutOptions
---@return nil
function Scroller:expel(options) end

---Move a window into the column another window is in.
---@param id integer
---@param target integer
---@param options sol.LayoutOptions
---@return nil
function Scroller:move_to_column_of(id, target, options) end

---Widen (positive) or narrow the column a window is in, by a share of the
---view. Clamped so a column neither vanishes nor grows past the view.
---@param id integer
---@param by number
---@param options sol.LayoutOptions
---@return nil
function Scroller:widen(id, by, options) end

---Cycle the active column through the preset widths.
---@param options sol.LayoutOptions
---@return nil
function Scroller:cycle_width(options) end

---Take `widths` and `default_width` from `options` as `sol.layout.scroller`
---does, keeping the columns, the focus and the view. For a strip `sol.keep`
---carried across a reload.
---@param options { widths?: number[], default_width?: integer }
---@return nil
function Scroller:configure(options) end

---Scroll the view by a distance, without moving focus.
---@param delta number
---@return nil
function Scroller:scroll_by(delta) end

---The focused window, if there is one.
---@return integer|nil
function Scroller:focused() end

---Whether a window is in the strip.
---@param id integer
---@return boolean
function Scroller:contains(id) end

---Every window in the strip, left to right and top to bottom within a column.
---@return integer[]
function Scroller:windows() end

---Where every window in the strip goes, ready for `sol.place`.
---@param options sol.LayoutOptions
---@return sol.Slot[]
function Scroller:layout(options) end
