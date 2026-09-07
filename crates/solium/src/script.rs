//! The Lua scripting surface.
//!
//! Modes — overview, the app switcher, peek, the icon→window genie — are
//! scripts over the presentation transform, not features with their own
//! renderers. This module is the whole boundary between them and the
//! compositor, and it is deliberately narrow: a script enumerates windows,
//! sets targets, and binds triggers. **Nothing here mentions Wayland.** A mode
//! that had to know a window was a `wl_surface` would be a mode that could not
//! be written by anyone but us.
//!
//! ## How a call works
//!
//! Scripts never hold compositor state. Each dispatch is:
//!
//! 1. Rust builds a [`Snapshot`] — windows, work area, cursor — and hands it to
//!    Lua as app data.
//! 2. The handler runs. Reads come from the snapshot; writes are pushed onto a
//!    queue as [`Command`]s.
//! 3. Rust drains the queue and applies it.
//!
//! Which is the "commands are not state" rule from `AGENTS.md` made structural:
//! a script cannot mutate the compositor directly, so there is no way for its
//! idea of a window's geometry to drift from the compositor's. It also means no
//! borrow of `Solium` is alive while Lua runs, which is what stops a script
//! calling back into the compositor mid-dispatch and deadlocking on the seat.

use std::{path::Path, time::Duration};

use anyhow::{Context, Result, anyhow};
use mlua::{Lua, Table, Value};
use smithay::utils::{Logical, Point};

use crate::present::Curve;

/// A rectangle as a script sees it: plain numbers, no coordinate-space types.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Rect {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) w: f64,
    pub(crate) h: f64,
}

impl Rect {
    fn to_table(self, lua: &Lua) -> mlua::Result<Table> {
        let table = lua.create_table()?;
        table.set("x", self.x)?;
        table.set("y", self.y)?;
        table.set("w", self.w)?;
        table.set("h", self.h)?;
        Ok(table)
    }

    fn contains(self, x: f64, y: f64) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
}

/// A window as a script sees it.
#[derive(Clone, Debug)]
pub(crate) struct WindowInfo {
    /// Stable for the window's lifetime. Not a pointer, not an index — a script
    /// holding one across a frame must not be able to address a different
    /// window with it.
    pub(crate) id: u64,
    /// The window including its frame, which is what "the window" means to
    /// anything positioning it.
    pub(crate) rect: Rect,
    /// Where it is being drawn right now, which in a mode is somewhere else.
    pub(crate) drawn: Rect,
    pub(crate) title: String,
    pub(crate) focused: bool,
    /// Which monitor it is on, by name.
    ///
    /// Derived from where the window is rather than remembered, so a window
    /// dragged to the next screen belongs to it without anything having to be
    /// told. This is what lets a layout run per monitor: group the windows by
    /// this, lay out each group in that monitor's own area.
    pub(crate) monitor: String,
}

/// A monitor as a script sees it.
#[derive(Clone, Debug, Default)]
pub(crate) struct MonitorInfo {
    /// The connector name — `DP-1`, `eDP-1`, `winit-2`. What `WindowInfo`'s
    /// `monitor` matches, and what a configured arrangement names.
    pub(crate) name: String,
    /// What windows may use: the monitor less whatever anchored surfaces have
    /// reserved. In the global space, so a rect from here can be handed
    /// straight to `sol.place`.
    pub(crate) area: Rect,
    /// The whole monitor, exclusive zones included. What a wallpaper or a
    /// fullscreen window covers.
    pub(crate) whole: Rect,
    /// How many device pixels to a logical one.
    pub(crate) scale: f64,
    /// Whether this is the one the pointer is on. See `Solium::active_output`.
    pub(crate) focused: bool,
    /// Whether this is the one a dock goes on. See `Solium::primary_output`.
    pub(crate) primary: bool,
    /// Its rotation, as the name a configuration would write.
    pub(crate) transform: String,
}

/// What the compositor looked like when a handler was called.
#[derive(Clone, Debug, Default)]
pub(crate) struct Snapshot {
    /// Topmost first, so hit-testing walks it in order.
    pub(crate) windows: Vec<WindowInfo>,
    /// Every monitor, in the order the compositor holds them.
    pub(crate) monitors: Vec<MonitorInfo>,
    /// The active monitor's work area — what `sol.monitor()` answers.
    ///
    /// Kept as its own field rather than found in `monitors` every time,
    /// because almost every script wants exactly this and nothing else.
    pub(crate) work_area: Rect,
    pub(crate) cursor: (f64, f64),
}

/// How a batch of transforms should animate.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AnimationSpec {
    pub(crate) duration: Duration,
    pub(crate) easing: Curve,
}

impl Default for AnimationSpec {
    fn default() -> Self {
        Self {
            duration: Duration::from_millis(220),
            easing: Curve::OutCubic,
        }
    }
}

/// Something a script asked the compositor to do.
#[derive(Clone, Debug)]
pub(crate) enum Command {
    Present {
        id: u64,
        rect: Option<Rect>,
        opacity: Option<f32>,
        /// A 3D transform about the drawn rect's centre, when the script asked
        /// for one. `None` keeps the window flat and on the cheap path.
        matrix: Option<Mat4>,
        /// A deformation the drawn rect cannot express, such as a genie.
        deform: Option<crate::present::Deform>,
        animation: AnimationSpec,
    },
    Clear {
        id: u64,
        animation: AnimationSpec,
    },
    /// Draw the window at `rect` and animate it to where it lives.
    PresentFrom {
        id: u64,
        rect: Rect,
        opacity: Option<f32>,
        animation: AnimationSpec,
    },
    Focus {
        id: u64,
    },
    /// Frame every window with a named decoration.
    Decoration {
        name: Option<String>,
    },
    /// End the session.
    Quit,
    /// Read the configuration again.
    Reload,
    /// Where the monitors are, relative to each other.
    Monitors(crate::monitor::Arrangement),
    /// Show or hide the Developer Tweaks panel.
    TweaksToggle,
    /// Move and resize a window for real — the layout's authority, not a
    /// transform. The compositor animates it there from where it was.
    Place {
        id: u64,
        rect: Rect,
        animation: AnimationSpec,
    },
    /// Ask a window to close. A request, not a kill: the client decides.
    Close {
        id: u64,
    },
    /// Start a program, connected to this compositor.
    Spawn {
        program: String,
        args: Vec<String>,
    },
    /// How a window behaves between being asked for and its application
    /// arriving. See `Loading`.
    Loading(Loading),
}

/// What the compositor does with a window whose application has not connected.
///
/// Settings rather than constants because every part of this is a matter of
/// taste: what it looks like, how long to wait, and whether it takes a place in
/// the layout at all. Someone who wants a window to appear only when it is
/// really there should be able to say so without a rebuild.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Loading {
    /// Which QML draws it — a name, or a path. See `pane::loading_source`.
    pub(crate) scene: Option<String>,
    /// How long to hold a window open for an application that never arrives.
    pub(crate) patience: std::time::Duration,
    /// Whether it takes a place in the layout before its application connects.
    /// Off, and the other windows only move aside once it is really there.
    pub(crate) reserves_a_slot: bool,
    /// Whether the frame is *drawn* while it waits.
    ///
    /// Off by default. The frame is always built, so the room it takes is
    /// reserved from the first frame and the window does not change shape when
    /// the application arrives — this only decides whether the bar is on screen
    /// meanwhile. Drawn, it gives a close button for an application that is not
    /// coming; hidden, the scene has the whole window and says the name once
    /// rather than twice.
    pub(crate) decorated: bool,
    /// How long the scene takes to fade off the application that replaced it.
    ///
    /// Drawn over the window rather than instead of it, so the application is
    /// already there underneath as it goes. Zero cuts straight to it.
    pub(crate) fade: std::time::Duration,
}

impl Default for Loading {
    fn default() -> Self {
        Self {
            scene: None,
            patience: std::time::Duration::from_secs(8),
            reserves_a_slot: true,
            decorated: false,
            fade: std::time::Duration::from_millis(180),
        }
    }
}

/// The result of one dispatch.
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    /// Whether a script handled the trigger. An unhandled key goes to the
    /// focused client.
    pub(crate) handled: bool,
    pub(crate) commands: Vec<Command>,
    /// Whether a mode now owns input, if a script said either way.
    pub(crate) grab: Option<bool>,
    /// What the compositor should show as the active mode, if it changed.
    pub(crate) status: Option<String>,
}

/// The queue a script writes into. Lives in Lua's app data for the length of a
/// dispatch, so no compositor state is borrowed while Lua runs.
#[derive(Debug, Default)]
struct Pending {
    commands: Vec<Command>,
    animation: AnimationSpec,
    grab: Option<bool>,
    status: Option<String>,
}

/// The Lua runtime and the scripts loaded into it.
#[derive(Debug)]
pub(crate) struct Scripts {
    lua: Lua,
}

impl Scripts {
    /// Bytes Lua is holding. Its allocator does not hand memory back to the
    /// system, so this rising while everything else is flat says the growth is
    /// script-side rather than a compositor leak.
    pub(crate) fn used_memory(&self) -> usize {
        self.lua.used_memory()
    }

    /// Where the configuration is, in the order it is looked for.
    ///
    /// The user's own file wins, and the bundled one is the fallback rather
    /// than a default that has to be copied before anything works.
    /// Where a user's own scripts live, whether or not they have any.
    pub(crate) fn user_config_dir() -> Option<std::path::PathBuf> {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config"))
            })
            .map(|base| base.join("solium"))
    }

    /// Every binding the configuration registered, as it spelled them.
    ///
    /// Sorted, because this is read by a person comparing one run to the next.
    pub(crate) fn binding_names(&self) -> Vec<String> {
        let Ok(sol) = self.lua.globals().get::<Table>("sol") else {
            return Vec::new();
        };
        let Ok(bindings) = sol.get::<Table>("_bindings") else {
            return Vec::new();
        };
        let mut names: Vec<String> = bindings
            .pairs::<String, Value>()
            .filter_map(Result::ok)
            .map(|(combo, _)| combo)
            .collect();
        names.sort();
        names
    }

    pub(crate) fn config_path() -> std::path::PathBuf {
        if let Some(path) = std::env::var_os("SOLIUM_LUA_INIT") {
            return std::path::PathBuf::from(path);
        }

        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config"))
            });

        if let Some(home) = config_home {
            let path = home.join("solium").join("init.lua");
            if path.is_file() {
                return path;
            }
        }

        std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/lua/init.lua"))
    }

    /// Load the configuration script and everything it pulls in.
    pub(crate) fn load(config: &Path) -> Result<Self> {
        let lua = Lua::new();
        lua.set_app_data(Pending::default());
        lua.set_app_data(Snapshot::default());

        let sol = build_api(&lua).map_err(failed("building the script API"))?;
        lua.globals()
            .set("sol", &sol)
            .map_err(failed("installing `sol`"))?;

        // So a script can `require` its neighbours.
        if let Some(directory) = config.parent().and_then(|path| path.to_str()) {
            let package: Table = lua
                .globals()
                .get("package")
                .map_err(failed("reading `package`"))?;
            let path: String = package.get("path").unwrap_or_default();
            // The user's directory first, then the config's own, then the
            // shipped set, then Lua's. Ordered this way so dropping a single
            // `config.lua` into ~/.config/solium overrides just that file —
            // copying the whole set to change one number is not
            // configurability.
            //
            // The shipped directory is added *unconditionally*, and that is
            // the fix for the thing this whole model rests on. The path used
            // to be built from the chosen config's own directory, which is
            // the shipped one only while you are using the shipped
            // `init.lua`. Write your own and `require("modes")` stopped
            // resolving — so the documented way to start ("replace the entry
            // point, require everything that ships, add your own") could not
            // work at all. Found by running the example out of the guide.
            let shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua");
            let mut search = format!("{directory}/?.lua;{shipped}/?.lua;{path}");
            if let Some(user) = Self::user_config_dir() {
                search = format!("{}/?.lua;{search}", user.display());
            }
            package
                .set("path", search)
                .map_err(failed("extending package.path"))?;
        }

        let source = std::fs::read_to_string(config)
            .with_context(|| format!("reading {}", config.display()))?;
        lua.load(&source)
            .set_name(config.to_string_lossy().as_ref())
            .exec()
            .map_err(failed("running the configuration"))?;

        let bindings: Table = sol.get("_bindings").map_err(failed("reading bindings"))?;
        // Counted outside the macro: inside it, `Value` resolves to tracing's
        // own trait of that name rather than to Lua's type.
        let bound = bindings.pairs::<Value, Value>().count();
        tracing::info!(config = %config.display(), bindings = bound, "scripts loaded");

        Ok(Self { lua })
    }

    /// Whether any script has bound this key combination.
    ///
    /// Deliberately side-effect free, and separate from dispatching it: the
    /// keyboard filter runs while the seat holds its own lock, so the decision
    /// to intercept is made there and the handler runs afterwards. Calling a
    /// script from inside the filter would let it call back into the seat and
    /// deadlock.
    pub(crate) fn has_binding(&self, combo: &str) -> bool {
        let Ok(sol) = self.lua.globals().get::<Table>("sol") else {
            return false;
        };
        let Ok(bindings) = sol.get::<Table>("_bindings") else {
            return false;
        };
        bindings
            .get::<Value>(normalise_combo(combo))
            .is_ok_and(|value| matches!(value, Value::Function(_)))
    }

    /// Run the handler bound to a key combination.
    /// The Developer Tweaks entries, as the scripts declared them.
    ///
    /// A JSON array, straight from Lua: the panel is a list of whatever the
    /// configuration says it is, so adding a tweak is editing `tweaks.lua`
    /// rather than the compositor.
    pub(crate) fn tweaks(&self) -> Option<String> {
        let sol = self.lua.globals().get::<Table>("sol").ok()?;
        let entries: Table = sol.get("_tweaks").ok()?;
        // Written out by hand rather than through a serialiser: three known
        // string fields do not justify a dependency, and this is the same
        // shape `publish_windows` already hands the shell.
        let quoted = |value: String| value.replace('\\', "").replace('"', "'");
        let mut out = String::from("[");
        for (index, entry) in entries.sequence_values::<Table>().enumerate() {
            let Ok(entry) = entry else { continue };
            let id = quoted(entry.get("id").unwrap_or_default());
            let label = quoted(entry.get("label").unwrap_or_default());
            let group = quoted(entry.get("group").unwrap_or_default());
            if index > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "{{\"id\":\"{id}\",\"label\":\"{label}\",\"group\":\"{group}\"}}"
            ));
        }
        out.push(']');
        Some(out)
    }

    /// Run the handler for a tweak the panel asked for.
    pub(crate) fn tweak(&mut self, id: &str, snapshot: Snapshot) -> Outcome {
        let id = id.to_owned();
        self.dispatch(snapshot, move |sol| {
            let handler: Value = sol.get("_tweak")?;
            match handler {
                Value::Function(function) => {
                    function.call::<()>(id)?;
                    Ok(true)
                }
                _ => Ok(false),
            }
        })
    }

    pub(crate) fn key(&mut self, combo: &str, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, |sol| {
            let bindings: Table = sol.get("_bindings")?;
            let handler: Value = bindings.get(normalise_combo(combo))?;
            match handler {
                Value::Function(function) => {
                    function.call::<()>(())?;
                    Ok(true)
                }
                _ => Ok(false),
            }
        })
    }

    /// Tell scripts a window has appeared.
    ///
    /// Every listener runs. Returns whether any of them did: nothing happening
    /// is a valid answer, and the compositor then uses its own plain animation
    /// rather than leaving a window to pop into existence.
    pub(crate) fn opened(&mut self, id: u64, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |sol| call_listeners(sol, "open", id))
    }

    /// Run the handler for a pointer press, while a mode owns input.
    pub(crate) fn click(&mut self, x: f64, y: f64, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |sol| call_listeners(sol, "click", (x, y)))
    }

    /// Whatever the configuration asked for while it was being read.
    ///
    /// A script's top level runs once, at load, and anything it requests there
    /// — the dock's contents, a starting mode — lands in the same pending
    /// buffer an event handler would use. Without draining it that work is
    /// simply dropped, silently, which is what happened to the first dock: no
    /// error, no items, nothing drawn.
    pub(crate) fn startup(&mut self) -> Outcome {
        // Takes the buffer the configuration filled, rather than dispatching:
        // `dispatch` installs a fresh `Pending` before it calls anything, so
        // routing this through it would throw away the very requests it is
        // here to collect.
        let pending = self.lua.remove_app_data::<Pending>().unwrap_or_default();
        self.lua.set_app_data(Pending::default());
        Outcome {
            handled: false,
            commands: pending.commands,
            grab: pending.grab,
            status: pending.status,
        }
    }

    /// Focus moved to a window.
    pub(crate) fn focused(&mut self, id: u64, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |sol| call_listeners(sol, "focus", id))
    }

    /// An edge was dragged.
    ///
    /// Offered to layouts before the compositor resizes anything, so a tiled
    /// window can move its seam instead of growing over its neighbour.
    pub(crate) fn resized(
        &mut self,
        id: u64,
        at: (f64, f64),
        edges: (bool, bool),
        snapshot: Snapshot,
    ) -> Outcome {
        self.dispatch(snapshot, move |sol| {
            call_listeners(sol, "resize", (id, at.0, at.1, edges.0, edges.1))
        })
    }

    /// A window went away.
    ///
    /// Needed by any layout that keeps state of its own: a tree cannot drop a
    /// node it is never told about, and diffing the window list on every pass
    /// — which is what the layouts did instead — cannot tell "closed" from
    /// "moved to another workspace".
    pub(crate) fn closed(&mut self, id: u64, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |sol| call_listeners(sol, "close", id))
    }

    /// Something the compositor owns changed the space windows get.
    ///
    /// A decoration reserving a different amount is the case that needs it:
    /// the slots are the same but what fits in them is not, and a layout that
    /// is never told goes on believing its own last arithmetic.
    pub(crate) fn relayout(&mut self, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |sol| call_listeners(sol, "layout", ()))
    }

    /// A window was dragged and let go.
    ///
    /// The event a layout needs and could not have: without it a drag in a
    /// tiled layout leaves the window wherever the cursor stopped, because
    /// nothing ever tells the layout to think again.
    pub(crate) fn dropped(&mut self, id: u64, x: f64, y: f64, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |sol| call_listeners(sol, "drop", (id, x, y)))
    }

    /// The pointer wheel turned, with the compositor's modifier held.
    ///
    /// Only offered to scripts when Super is down, so an unmodified wheel keeps
    /// belonging to whatever is under the cursor. A scrolling layout that ate
    /// every wheel event would make every terminal in it unusable.
    pub(crate) fn scrolled(&mut self, dx: f64, dy: f64, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |sol| call_listeners(sol, "scroll", (dx, dy)))
    }

    /// The shared shape of every dispatch: snapshot in, commands out.
    fn dispatch(
        &mut self,
        snapshot: Snapshot,
        call: impl FnOnce(&Table) -> mlua::Result<bool>,
    ) -> Outcome {
        self.lua.set_app_data(snapshot);
        self.lua.set_app_data(Pending::default());

        let handled = match self.lua.globals().get::<Table>("sol") {
            Ok(sol) => match call(&sol) {
                Ok(handled) => handled,
                Err(err) => {
                    // A script erroring must not take the compositor with it,
                    // and must not leave the trigger swallowed either: an
                    // unhandled key belongs to the focused client.
                    tracing::error!(%err, "a script failed");
                    false
                }
            },
            Err(err) => {
                tracing::error!(%err, "`sol` is missing");
                false
            }
        };

        let pending = self.lua.remove_app_data::<Pending>().unwrap_or_default();

        Outcome {
            handled,
            commands: pending.commands,
            grab: pending.grab,
            status: pending.status,
        }
    }
}

use crate::mat4::Mat4;
use solium_layout::{Rect as Slot, Settings};

/// The work area a layout call was given.
fn area(options: &Table) -> mlua::Result<Slot> {
    Ok(Slot::new(
        options.get("x")?,
        options.get("y")?,
        options.get("w")?,
        options.get("h")?,
    ))
}

fn tuning(options: &Table) -> mlua::Result<Settings> {
    let defaults = Settings::default();
    Ok(Settings {
        gap: options.get::<Option<f64>>("gap")?.unwrap_or(defaults.gap),
        ratio: options
            .get::<Option<f64>>("ratio")?
            .unwrap_or(defaults.ratio),
        column: options
            .get::<Option<f64>>("column")?
            .unwrap_or(defaults.column),
        padding: options
            .get::<Option<f64>>("padding")?
            .unwrap_or(defaults.padding),
        split: options
            .get::<Option<f64>>("split")?
            .unwrap_or(defaults.split),
    })
}

/// The `sol.layout` table.
fn layouts(lua: &Lua) -> mlua::Result<Table> {
    fn to_lua(lua: &Lua, slots: &[Slot]) -> mlua::Result<Table> {
        let list = lua.create_table()?;
        for (index, slot) in slots.iter().enumerate() {
            let entry = lua.create_table()?;
            entry.set("x", slot.x)?;
            entry.set("y", slot.y)?;
            entry.set("w", slot.w)?;
            entry.set("h", slot.h)?;
            list.set(index + 1, entry)?;
        }
        Ok(list)
    }

    let layout = lua.create_table()?;

    layout.set(
        "master_stack",
        lua.create_function(|lua, (count, options): (usize, Table)| {
            let slots = solium_layout::master_stack(count, area(&options)?, tuning(&options)?);
            to_lua(lua, &slots)
        })?,
    )?;

    // A dwindle tree, held by the script that made it.
    //
    // Stateful on purpose, because the arrangement is: where a window lands
    // depends on which window was split and where the pointer was, and no
    // function of the window list can recover that after the fact.
    layout.set(
        "tree",
        lua.create_function(|_, ()| Ok(TilingTree::default()))?,
    )?;

    // A scrolling workspace, held by the script that made it. Stateful for the
    // same reason the tree is: which column is active and where the view sits
    // relative to it are not recoverable from a list of windows.
    layout.set(
        "scroller",
        lua.create_function(|_, ()| Ok(Scrolling::default()))?,
    )?;

    layout.set(
        "strip",
        lua.create_function(|lua, (columns, options): (Table, Table)| {
            let columns = columns_from(&columns)?;
            let offset = options.get::<Option<f64>>("offset")?.unwrap_or_default();
            let slots = solium_layout::strip(&columns, area(&options)?, tuning(&options)?, offset);
            to_lua(lua, &slots)
        })?,
    )?;

    layout.set(
        "strip_scroll_to",
        lua.create_function(|_, (index, columns, options): (usize, Table, Table)| {
            let columns = columns_from(&columns)?;
            let offset = options.get::<Option<f64>>("offset")?.unwrap_or_default();
            Ok(solium_layout::strip_scroll_to(
                index.saturating_sub(1),
                &columns,
                area(&options)?,
                tuning(&options)?,
                offset,
            ))
        })?,
    )?;

    layout.set(
        "scrolling",
        lua.create_function(|lua, (count, options): (usize, Table)| {
            let offset = options.get::<Option<f64>>("offset")?.unwrap_or_default();
            let slots = solium_layout::scrolling(count, area(&options)?, tuning(&options)?, offset);
            to_lua(lua, &slots)
        })?,
    )?;

    layout.set(
        "scroll_to",
        lua.create_function(|_, (index, count, options): (usize, usize, Table)| {
            let offset = options.get::<Option<f64>>("offset")?.unwrap_or_default();
            Ok(solium_layout::scroll_to(
                index.saturating_sub(1),
                count,
                area(&options)?,
                tuning(&options)?,
                offset,
            ))
        })?,
    )?;

    layout.set(
        "grid",
        lua.create_function(|lua, (sizes, options): (Table, Table)| {
            let mut windows = Vec::new();
            for size in sizes.sequence_values::<Table>() {
                let size = size?;
                windows.push(Slot::new(
                    size.get("x")?,
                    size.get("y")?,
                    size.get("w")?,
                    size.get("h")?,
                ));
            }
            let slots = solium_layout::grid(&windows, area(&options)?, tuning(&options)?);
            to_lua(lua, &slots)
        })?,
    )?;

    Ok(layout)
}

/// Run every listener registered for an event.
///
/// One failing listener is logged and the rest still run: a broken script must
/// not silently disable the others, which is what returning early would do.
fn call_listeners(
    sol: &Table,
    event: &str,
    args: impl mlua::IntoLuaMulti + Clone,
) -> mlua::Result<bool> {
    let handlers: Table = sol.get("_handlers")?;
    let Value::Table(listeners) = handlers.get::<Value>(event)? else {
        return Ok(false);
    };

    let mut called = false;
    for listener in listeners.sequence_values::<mlua::Function>() {
        match listener.and_then(|handler| handler.call::<()>(args.clone())) {
            Ok(()) => called = true,
            Err(err) => tracing::error!(%err, event, "a listener failed"),
        }
    }
    Ok(called)
}

/// Turn a Lua error into one of ours.
///
/// `mlua::Error` is not `Send + Sync`, so it cannot be an `anyhow` source
/// directly; the message is what matters here anyway, and a Lua error already
/// carries its own traceback in it.
fn failed(context: &'static str) -> impl FnOnce(mlua::Error) -> anyhow::Error {
    move |err| anyhow!("{context}: {err}")
}

/// Build the `sol` table.
fn build_api(lua: &Lua) -> mlua::Result<Table> {
    let sol = lua.create_table()?;
    sol.set("_bindings", lua.create_table()?)?;
    sol.set("_handlers", lua.create_table()?)?;

    sol.set(
        "windows",
        lua.create_function(|lua, ()| {
            let snapshot = snapshot(lua)?;
            let windows = lua.create_table()?;
            for (index, window) in snapshot.windows.iter().enumerate() {
                let entry = lua.create_table()?;
                entry.set("id", window.id)?;
                entry.set("x", window.rect.x)?;
                entry.set("y", window.rect.y)?;
                entry.set("w", window.rect.w)?;
                entry.set("h", window.rect.h)?;
                entry.set("title", window.title.clone())?;
                entry.set("focused", window.focused)?;
                entry.set("monitor", window.monitor.clone())?;
                windows.set(index + 1, entry)?;
            }
            Ok(windows)
        })?,
    )?;

    // Reads with no argument, configures with a table. One name for one
    // subject: `sol.monitors()` asks where the monitors are, `sol.monitors{…}`
    // says. The same shape as `sol.monitor()`, which answers about one.
    sol.set(
        "monitors",
        lua.create_function(|lua, options: Option<Vec<mlua::Table>>| {
            let Some(rows) = options else {
                let monitors = lua.create_table()?;
                for (index, monitor) in snapshot(lua)?.monitors.iter().enumerate() {
                    let entry = monitor.area.to_table(lua)?;
                    entry.set("name", monitor.name.clone())?;
                    entry.set("scale", monitor.scale)?;
                    entry.set("focused", monitor.focused)?;
                    entry.set("primary", monitor.primary)?;
                    entry.set("transform", monitor.transform.clone())?;
                    // The whole monitor as well as the usable part: a
                    // wallpaper and a fullscreen window want the one a bar has
                    // not taken a bite out of.
                    entry.set("whole", monitor.whole.to_table(lua)?)?;
                    monitors.set(index + 1, entry)?;
                }
                return Ok(Value::Table(monitors));
            };

            let mut places = Vec::new();
            for row in rows {
                // A row with no name cannot be matched to a connector, and
                // silently dropping it is how a configuration appears to be
                // ignored. Say which row, because the list has no other
                // landmarks.
                let Ok(Value::String(name)) = row.get::<Value>("name") else {
                    tracing::warn!(
                        row = places.len() + 1,
                        "a monitor with no name -- run `solium --probe` for the ones this \
                         machine has"
                    );
                    continue;
                };
                let Ok(name) = name.to_str() else {
                    continue;
                };
                let name = name.to_string();

                // A position outright, when both halves were given. One half
                // alone is treated as unset rather than as zero: `y = 200` on
                // its own almost certainly means "and leave x alone", and
                // reading the other as 0 would slide the monitor to the left
                // edge of the desk for no stated reason.
                let (x, y) = (row.get::<Option<i32>>("x")?, row.get::<Option<i32>>("y")?);
                let at = match (x, y) {
                    (Some(x), Some(y)) => Some(Point::<i32, Logical>::from((x, y))),
                    (Some(x), None) => Some(Point::from((x, 0))),
                    (None, Some(y)) => Some(Point::from((0, y))),
                    (None, None) => None,
                };

                // Or beside another monitor, which is the form that does not
                // go stale when a resolution changes.
                let align = match row.get::<Value>("align") {
                    Ok(Value::String(name)) => name
                        .to_str()
                        .ok()
                        .and_then(|name| crate::monitor::align(&name))
                        .unwrap_or_default(),
                    _ => crate::monitor::Align::default(),
                };
                let mut beside = None;
                for (key, side) in [
                    ("right_of", crate::monitor::Side::Right),
                    ("left_of", crate::monitor::Side::Left),
                    ("above", crate::monitor::Side::Above),
                    ("below", crate::monitor::Side::Below),
                ] {
                    if let Ok(Value::String(anchor)) = row.get::<Value>(key)
                        && let Ok(anchor) = anchor.to_str()
                    {
                        beside = Some((side, anchor.to_string(), align));
                        break;
                    }
                }

                // A mode, written the way every display tool writes one:
                // `"2560x1440@165"`. A table with `w`, `h` and `refresh` does
                // the same thing, for anyone generating a configuration rather
                // than typing it.
                let mode = match row.get::<Value>("mode") {
                    Ok(Value::String(text)) => {
                        let text = text.to_str().ok().map(|text| text.to_string());
                        match text.as_deref().and_then(crate::monitor::mode) {
                            Some(mode) => mode,
                            None => {
                                tracing::warn!(
                                    monitor = name,
                                    mode = ?text,
                                    "not a mode -- write it as 2560x1440@165, or as one of \
                                     best, preferred, widest"
                                );
                                crate::monitor::Wanted::default()
                            }
                        }
                    }
                    Ok(Value::Table(mode)) => {
                        match (mode.get::<Option<i32>>("w")?, mode.get::<Option<i32>>("h")?) {
                            (Some(width), Some(height)) if width > 0 && height > 0 => {
                                crate::monitor::Wanted::Exact {
                                    width,
                                    height,
                                    refresh: mode.get::<Option<i32>>("refresh")?,
                                }
                            }
                            _ => {
                                tracing::warn!(
                                    monitor = name,
                                    "a mode table needs both w and h -- ignoring it"
                                );
                                crate::monitor::Wanted::default()
                            }
                        }
                    }
                    _ => crate::monitor::Wanted::default(),
                };

                let transform = match row.get::<Value>("transform") {
                    Ok(Value::String(value)) => {
                        let value = value.to_str().ok().map(|value| value.to_string());
                        match value.as_deref().and_then(crate::monitor::transform) {
                            Some(transform) => Some(transform),
                            None => {
                                tracing::warn!(
                                    monitor = name,
                                    transform = ?value,
                                    "not a transform -- one of normal, 90, 180, 270, \
                                     flipped, flipped-90, flipped-180, flipped-270"
                                );
                                None
                            }
                        }
                    }
                    // A number is what people write for a rotation, and
                    // refusing it over a quotation mark would be pedantry.
                    Ok(Value::Integer(degrees)) => crate::monitor::transform(&degrees.to_string()),
                    _ => None,
                };

                places.push(crate::monitor::Placement {
                    name,
                    at,
                    beside,
                    mode,
                    vrr: row.get::<Option<bool>>("vrr")?,
                    transform,
                    enabled: row.get::<Option<bool>>("enabled")?.unwrap_or(true),
                    primary: row.get::<Option<bool>>("primary")?.unwrap_or(false),
                    scale: row.get::<Option<f64>>("scale")?,
                });
            }
            with_pending(lua, |pending| {
                pending
                    .commands
                    .push(Command::Monitors(crate::monitor::Arrangement::new(
                        places.clone(),
                    )));
            })?;
            Ok(Value::Nil)
        })?,
    )?;

    // The monitor a window is on, or the active one when asked about nothing.
    //
    // Both answers are a work-area rect, so every script written when there
    // was one monitor still reads correctly: `sol.monitor()` meant "the screen"
    // and still does — it is just no longer the only one.
    sol.set(
        "monitor",
        lua.create_function(|lua, id: Option<u64>| {
            let snapshot = snapshot(lua)?;
            let named = id
                .and_then(|id| snapshot.windows.iter().find(|window| window.id == id))
                .and_then(|window| {
                    snapshot
                        .monitors
                        .iter()
                        .find(|monitor| monitor.name == window.monitor)
                })
                .map(|monitor| monitor.area);
            named.unwrap_or(snapshot.work_area).to_table(lua)
        })?,
    )?;

    sol.set(
        "cursor",
        lua.create_function(|lua, ()| {
            let (x, y) = snapshot(lua)?.cursor;
            let table = lua.create_table()?;
            table.set("x", x)?;
            table.set("y", y)?;
            Ok(table)
        })?,
    )?;

    // Hit-testing against *drawn* rects, not real ones: in a mode a window is
    // where the script put it, and asking the compositor is what keeps the
    // script from reimplementing the transform to find out.
    sol.set(
        "window_at",
        lua.create_function(|lua, (x, y, skip): (f64, f64, Option<u64>)| {
            // `skip` is what makes this usable while dragging. A dragged
            // window follows the cursor, so it is always the topmost thing
            // under it — ask without skipping and the answer is always the
            // window in your hand, which is why dropping one onto another
            // never swapped anything.
            Ok(snapshot(lua)?
                .windows
                .iter()
                .find(|window| Some(window.id) != skip && window.drawn.contains(x, y))
                .map(|window| window.id))
        })?,
    )?;

    sol.set(
        "present",
        lua.create_function(|lua, (id, options): (u64, Option<Table>)| {
            let (rect, opacity, matrix, deform) = match options {
                Some(options) => (
                    rect_from(&options)?,
                    options.get::<Option<f32>>("opacity")?,
                    transform_from(&options)?,
                    deform_from(&options)?,
                ),
                None => (None, None, None, None),
            };
            with_pending(lua, |pending| {
                let animation = pending.animation;
                pending.commands.push(Command::Present {
                    id,
                    rect,
                    opacity,
                    matrix,
                    deform,
                    animation,
                });
            })
        })?,
    )?;

    // The other direction from `present`: this says where a window comes *from*
    // and lets it land where it belongs. A dock icon's rectangle here is the
    // macOS-style genie, and the compositor needs no idea that is what it is.
    sol.set(
        "present_from",
        lua.create_function(|lua, (id, options): (u64, Table)| {
            let Some(rect) = rect_from(&options)? else {
                return Err(mlua::Error::runtime(
                    "sol.present_from needs a rect to come from",
                ));
            };
            let opacity = options.get::<Option<f32>>("opacity")?;
            with_pending(lua, |pending| {
                let animation = pending.animation;
                pending.commands.push(Command::PresentFrom {
                    id,
                    rect,
                    opacity,
                    animation,
                });
            })
        })?,
    )?;

    // Which QML file frames every window. A name is one of the decorations
    // that ship, or one of the user's own in ~/.config/solium/qml/decorations;
    // a path is anyone's. Takes effect immediately -- every frame is rebuilt.
    // Read the configuration again, in place. Bound to a key, this is the
    // difference between trying an idea and committing to it.
    // What the Developer Tweaks panel offers, and what to do when one is
    // pressed. Both live in Lua so the panel is a list of whatever the
    // configuration says rather than a menu built into the compositor.
    sol.set(
        "tweaks",
        lua.create_function(|lua, entries: Table| {
            let sol: Table = lua.globals().get("sol")?;
            sol.set("_tweaks", entries)
        })?,
    )?;

    // Show or hide the panel. Does nothing without `--debug-mode`, so the
    // binding can live in the ordinary configuration.
    sol.set(
        "tweaks_toggle",
        lua.create_function(|lua, ()| {
            with_pending(lua, |pending| pending.commands.push(Command::TweaksToggle))
        })?,
    )?;

    sol.set(
        "on_tweak",
        lua.create_function(|lua, handler: mlua::Function| {
            let sol: Table = lua.globals().get("sol")?;
            sol.set("_tweak", handler)
        })?,
    )?;

    sol.set(
        "reload",
        lua.create_function(|lua, ()| {
            with_pending(lua, |pending| pending.commands.push(Command::Reload))
        })?,
    )?;

    sol.set(
        "decoration",
        lua.create_function(|lua, name: Option<String>| {
            with_pending(lua, |pending| {
                pending.commands.push(Command::Decoration { name });
            })
        })?,
    )?;

    // A table, so adding a setting later does not change the call. Anything
    // left out keeps its default rather than being reset -- `config.lua` is
    // merged from the user's own file and may well carry only one key.
    sol.set(
        "loading",
        lua.create_function(|lua, options: mlua::Table| {
            // Read as `Value` and matched, never `get::<Option<T>>`: on a table
            // that *errors* rather than answering None, and the `?` would take
            // the whole handler down with it.
            let mut loading = Loading::default();
            // Numbers and booleans read straight through; a missing key is
            // None, which leaves the default alone. `scene` is read as a
            // `Value` and matched instead, because asking mlua for an
            // `Option<String>` errors on anything that is not string-like
            // rather than answering None -- and `?` here would take the whole
            // handler down with it, which is how bezier easings once silently
            // stopped working.
            if let Some(millis) = options.get::<Option<u64>>("patience")? {
                loading.patience = Duration::from_millis(millis);
            }
            if let Some(reserves) = options.get::<Option<bool>>("reserves_a_slot")? {
                loading.reserves_a_slot = reserves;
            }
            if let Some(decorated) = options.get::<Option<bool>>("decorated")? {
                loading.decorated = decorated;
            }
            if let Some(millis) = options.get::<Option<u64>>("fade")? {
                loading.fade = Duration::from_millis(millis);
            }
            if let Ok(Value::String(scene)) = options.get::<Value>("scene")
                && let Ok(scene) = scene.to_str()
            {
                loading.scene = Some(scene.to_string());
            }
            with_pending(lua, |pending| {
                pending.commands.push(Command::Loading(loading.clone()));
            })
        })?,
    )?;

    sol.set(
        "present_clear",
        lua.create_function(|lua, id: u64| {
            with_pending(lua, |pending| {
                let animation = pending.animation;
                pending.commands.push(Command::Clear { id, animation });
            })
        })?,
    )?;

    // Applies to everything queued after it, so a mode sets the feel of a batch
    // once instead of repeating it per window — and every window in that batch
    // is animated by the same clock over the same interval.
    sol.set(
        "animate",
        lua.create_function(|lua, options: Table| {
            let duration = options
                .get::<Option<u64>>("duration")?
                .map(Duration::from_millis);
            let easing = easing_from(&options)?;
            with_pending(lua, |pending| {
                if let Some(duration) = duration {
                    pending.animation.duration = duration;
                }
                if let Some(easing) = easing {
                    pending.animation.easing = easing;
                }
            })
        })?,
    )?;

    sol.set(
        "focus",
        lua.create_function(|lua, id: u64| {
            with_pending(lua, |pending| pending.commands.push(Command::Focus { id }))
        })?,
    )?;

    // `place` changes where a window *lives*; `present` changes where it is
    // *drawn*. A layout uses this one — and leaving a mode afterwards restores
    // it to wherever the layout has since put it, which is the correct answer
    // and comes out for free.
    sol.set(
        "place",
        lua.create_function(|lua, (id, options): (u64, Table)| {
            let Some(rect) = rect_from(&options)? else {
                return Err(mlua::Error::runtime("sol.place needs a rect"));
            };
            with_pending(lua, |pending| {
                let animation = pending.animation;
                pending.commands.push(Command::Place {
                    id,
                    rect,
                    animation,
                });
            })
        })?,
    )?;

    sol.set(
        "close",
        lua.create_function(|lua, id: u64| {
            with_pending(lua, |pending| pending.commands.push(Command::Close { id }))
        })?,
    )?;

    // `sol.spawn("foot", "-e", "htop")`. Variadic rather than a table because
    // the common case is one program and no arguments, and that should read
    // like one.
    sol.set(
        "spawn",
        lua.create_function(|lua, mut args: mlua::Variadic<String>| {
            if args.is_empty() {
                return Err(mlua::Error::runtime("sol.spawn needs a program to run"));
            }
            let program = args.remove(0);
            let args = args.into_iter().collect();
            with_pending(lua, |pending| {
                pending.commands.push(Command::Spawn { program, args });
            })
        })?,
    )?;

    // Whether a program exists to be run. A desktop cannot assume any
    // particular terminal, editor or browser is installed, and a script that
    // picks the first one that *is* there beats one that fails silently on a
    // machine set up differently from the author's.
    sol.set(
        "which",
        lua.create_function(|_, program: String| Ok(which(&program)))?,
    )?;

    // Ending the session, as a command a script issues rather than a key the
    // compositor keeps for itself. Ctrl+Alt+Backspace still works and always
    // will — that one has to survive a broken config — but a compositor whose
    // only way out is hardcoded is not a configurable one.
    sol.set(
        "quit",
        lua.create_function(|lua, ()| {
            with_pending(lua, |pending| pending.commands.push(Command::Quit))
        })?,
    )?;

    sol.set(
        "grab_input",
        lua.create_function(|lua, grabbed: bool| {
            with_pending(lua, |pending| pending.grab = Some(grabbed))
        })?,
    )?;

    sol.set(
        "status",
        lua.create_function(|lua, text: String| {
            with_pending(lua, |pending| pending.status = Some(text))
        })?,
    )?;

    sol.set(
        "bind",
        lua.create_function(|lua, (combo, handler): (String, mlua::Function)| {
            let sol: Table = lua.globals().get("sol")?;
            let bindings: Table = sol.get("_bindings")?;
            bindings.set(normalise_combo(&combo), handler)?;
            Ok(())
        })?,
    )?;

    // Appended, never replaced. Two scripts caring about the same event is
    // ordinary — a tiling script and an open animation both want to know a
    // window appeared — and the second one silently unhooking the first is the
    // kind of bug that gets blamed on the compositor.
    sol.set(
        "on",
        lua.create_function(|lua, (event, handler): (String, mlua::Function)| {
            let sol: Table = lua.globals().get("sol")?;
            let handlers: Table = sol.get("_handlers")?;
            let listeners: Table = match handlers.get::<Value>(event.clone())? {
                Value::Table(existing) => existing,
                _ => {
                    let created = lua.create_table()?;
                    handlers.set(event, &created)?;
                    created
                }
            };
            listeners.push(handler)?;
            Ok(())
        })?,
    )?;

    // The standard arrangements, from the same crate the preview calls. A
    // script picks which and when — and may compute its own rectangles instead,
    // which is why these are offered rather than imposed.
    sol.set("layout", layouts(lua)?)?;

    sol.set(
        "log",
        lua.create_function(|_, message: String| {
            tracing::info!(target: "solium::script", "{message}");
            Ok(())
        })?,
    )?;

    Ok(sol)
}

fn snapshot(lua: &Lua) -> mlua::Result<Snapshot> {
    Ok(lua
        .app_data_ref::<Snapshot>()
        .map(|snapshot| snapshot.clone())
        .unwrap_or_default())
}

fn with_pending(lua: &Lua, f: impl FnOnce(&mut Pending)) -> mlua::Result<()> {
    match lua.app_data_mut::<Pending>() {
        Some(mut pending) => {
            f(&mut pending);
            Ok(())
        }
        // Reachable only if a script squirrels a `sol` function away and calls
        // it outside a dispatch. Ignoring it is right: there is no batch for
        // the command to belong to.
        None => Err(mlua::Error::runtime(
            "sol functions may only be called from a handler",
        )),
    }
}

fn rect_from(options: &Table) -> mlua::Result<Option<Rect>> {
    let (x, y, w, h) = (
        options.get::<Option<f64>>("x")?,
        options.get::<Option<f64>>("y")?,
        options.get::<Option<f64>>("w")?,
        options.get::<Option<f64>>("h")?,
    );
    match (x, y, w, h) {
        (Some(x), Some(y), Some(w), Some(h)) => Ok(Some(Rect { x, y, w, h })),
        (None, None, None, None) => Ok(None),
        // A half-specified rect is a typo, and silently filling in the missing
        // half would put a window somewhere nobody asked for.
        _ => Err(mlua::Error::runtime(
            "a rect needs all of x, y, w and h, or none of them",
        )),
    }
}

/// Look up a curve by the name a script used.
///
/// The names come from the animation engine rather than a list kept here, so a
/// curve added there is immediately available to scripts — including springs,
/// which is why this is a lookup and not a match.
fn parse_easing(name: &str) -> Option<Curve> {
    Curve::from_name(name).or_else(|| {
        tracing::warn!(easing = name, "unknown easing, keeping the default");
        None
    })
}

/// Read an easing from an options table: a name, or a curve of your own.
///
/// ```lua
/// sol.animate({ duration = 240, easing = "outBack" })
/// sol.animate({ duration = 240, easing = { 0.34, 1.56, 0.64, 1 } })
/// ```
///
/// The four numbers are the control points of a cubic bezier -- what CSS
/// calls `cubic-bezier`, and what every easing generator on the internet
/// hands out. Named curves stay for the handful worth naming; this is so a
/// feel nobody anticipated does not need a compositor release.
fn easing_from(options: &Table) -> mlua::Result<Option<Curve>> {
    // Read as a value and matched, not asked for as a String and then as a
    // Table: asking a table for a String does not answer "no", it *fails*, and
    // the failure takes down the whole call it was made from. Which is how
    // every curve given as four numbers silently did nothing.
    let points = match options.get::<Value>("easing")? {
        Value::Nil => return Ok(None),
        Value::String(name) => return Ok(parse_easing(&name.to_string_lossy())),
        Value::Table(points) => points,
        other => {
            tracing::warn!(
                kind = other.type_name(),
                "an easing is a name or four numbers; keeping the default"
            );
            return Ok(None);
        }
    };
    let numbers: Vec<f64> = points.sequence_values::<f64>().collect::<Result<_, _>>()?;
    let [x1, y1, x2, y2] = numbers.as_slice() else {
        tracing::warn!(
            found = numbers.len(),
            "an easing curve is four numbers; keeping the default"
        );
        return Ok(None);
    };
    // x outside 0..1 is not a curve a clock can walk along: time would have to
    // run backwards to reach it.
    Ok(Some(Curve::Bezier {
        x1: x1.clamp(0.0, 1.0),
        y1: *y1,
        x2: x2.clamp(0.0, 1.0),
        y2: *y2,
    }))
}

/// A scrolling workspace, as scripts hold it.
#[derive(Debug, Default)]
struct Scrolling(solium_layout::scroller::Scroller);

impl mlua::UserData for Scrolling {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method_mut("insert", |_, this, (id, options): (u64, Table)| {
            this.0.insert(id, area(&options)?, tuning(&options)?);
            Ok(())
        });

        methods.add_method_mut(
            "insert_into_column",
            |_, this, (id, options): (u64, Table)| {
                this.0
                    .insert_into_active(id, area(&options)?, tuning(&options)?);
                Ok(())
            },
        );

        methods.add_method_mut("remove", |_, this, id: u64| {
            this.0.remove(id);
            Ok(())
        });

        methods.add_method_mut("focus_window", |_, this, (id, options): (u64, Table)| {
            this.0.focus_window(id, area(&options)?, tuning(&options)?);
            Ok(())
        });

        methods.add_method_mut("focus_sideways", |_, this, (by, options): (i32, Table)| {
            this.0
                .focus_sideways(by as isize, area(&options)?, tuning(&options)?);
            Ok(())
        });

        methods.add_method_mut("focus_vertically", |_, this, by: i32| {
            this.0.focus_vertically(by as isize);
            Ok(())
        });

        methods.add_method_mut("move_column", |_, this, (by, options): (i32, Table)| {
            this.0
                .move_column(by as isize, area(&options)?, tuning(&options)?);
            Ok(())
        });

        methods.add_method_mut("consume", |_, this, ()| {
            this.0.consume();
            Ok(())
        });

        methods.add_method_mut("expel", |_, this, options: Table| {
            this.0.expel(area(&options)?, tuning(&options)?);
            Ok(())
        });

        methods.add_method_mut(
            "move_to_column_of",
            |_, this, (id, target, options): (u64, u64, Table)| {
                this.0
                    .move_to_column_of(id, target, area(&options)?, tuning(&options)?);
                Ok(())
            },
        );

        methods.add_method_mut("widen", |_, this, (id, by, options): (u64, f64, Table)| {
            this.0.widen(id, by, area(&options)?, tuning(&options)?);
            Ok(())
        });

        methods.add_method_mut("cycle_width", |_, this, options: Table| {
            this.0.cycle_width(area(&options)?, tuning(&options)?);
            Ok(())
        });

        methods.add_method_mut("scroll_by", |_, this, delta: f64| {
            this.0.scroll_by(delta);
            Ok(())
        });

        methods.add_method("focused", |_, this, ()| Ok(this.0.focused()));
        methods.add_method("contains", |_, this, id: u64| Ok(this.0.contains(id)));

        methods.add_method("layout", |lua, this, options: Table| {
            let out = lua.create_table()?;
            for (index, (id, rect)) in this
                .0
                .layout(area(&options)?, tuning(&options)?)
                .into_iter()
                .enumerate()
            {
                let entry = lua.create_table()?;
                entry.set("id", id)?;
                entry.set("x", rect.x)?;
                entry.set("y", rect.y)?;
                entry.set("w", rect.w)?;
                entry.set("h", rect.h)?;
                out.set(index + 1, entry)?;
            }
            Ok(out)
        });
    }
}

/// A dwindle tree, as scripts hold it.
#[derive(Debug, Default)]
struct TilingTree(solium_layout::tree::Tiling);

impl mlua::UserData for TilingTree {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        // `target` is the window to split and `x`/`y` are where the pointer
        // was; both may be nil, and then the pointer alone decides — which is
        // how Hyprland picks what to divide.
        methods.add_method_mut(
            "insert",
            |_, this, (id, target, x, y, options): (u64, Option<u64>, Option<f64>, Option<f64>, Table)| {
                let at = x.zip(y);
                this.0
                    .insert(id, target, at, area(&options)?, tuning(&options)?);
                Ok(())
            },
        );

        methods.add_method_mut("remove", |_, this, id: u64| {
            this.0.remove(id);
            Ok(())
        });

        methods.add_method_mut("resize", |_, this, (id, by): (u64, f64)| {
            this.0.resize(id, by);
            Ok(())
        });

        // `axis` is "width" or "height": which way the seam being dragged runs.
        methods.add_method_mut(
            "drag_seam",
            |_, this, (id, axis, x, y, options): (u64, String, f64, f64, Table)| {
                let axis = if axis == "width" {
                    solium_layout::tree::Axis::Vertical
                } else {
                    solium_layout::tree::Axis::Horizontal
                };
                this.0
                    .drag_seam(id, axis, (x, y), area(&options)?, tuning(&options)?);
                Ok(())
            },
        );

        methods.add_method("contains", |_, this, id: u64| Ok(this.0.contains(id)));

        methods.add_method("windows", |lua, this, ()| {
            let out = lua.create_table()?;
            for (index, id) in this.0.windows().into_iter().enumerate() {
                out.set(index + 1, id)?;
            }
            Ok(out)
        });

        // Rects come back tagged with the window they belong to, because tree
        // order is not the order the script knows its windows in.
        methods.add_method("layout", |lua, this, options: Table| {
            let out = lua.create_table()?;
            for (index, (id, rect)) in this
                .0
                .layout(area(&options)?, tuning(&options)?)
                .into_iter()
                .enumerate()
            {
                let entry = lua.create_table()?;
                entry.set("id", id)?;
                entry.set("x", rect.x)?;
                entry.set("y", rect.y)?;
                entry.set("w", rect.w)?;
                entry.set("h", rect.h)?;
                out.set(index + 1, entry)?;
            }
            Ok(out)
        });
    }
}

/// Read a 3D transform out of a script's options.
///
/// Degrees, because a configuration file written in radians is a
/// configuration file nobody edits twice. `perspective` is the viewer
/// distance in pixels — smaller is a wider lens and a harsher
/// foreshortening; without it the rotation is orthographic and a card turned
/// edge-on simply gets thinner instead of receding.
///
/// Returns `None` when nothing was asked for, so a window with no transform
/// stays on the renderer's cheap path.
fn transform_from(options: &Table) -> mlua::Result<Option<Mat4>> {
    let degrees = |name: &str| -> mlua::Result<Option<f32>> { options.get::<Option<f32>>(name) };
    let (rx, ry, rz) = (
        degrees("rotate_x")?,
        degrees("rotate_y")?,
        degrees("rotate_z")?,
    );
    let perspective = degrees("perspective")?;
    if rx.is_none() && ry.is_none() && rz.is_none() && perspective.is_none() {
        return Ok(None);
    }

    let radians = |value: Option<f32>| value.unwrap_or(0.0).to_radians();
    let mut matrix = Mat4::rotate_x(radians(rx))
        .then(Mat4::rotate_y(radians(ry)))
        .then(Mat4::rotate_z(radians(rz)));
    if let Some(distance) = perspective {
        matrix = matrix.then(Mat4::perspective(distance));
    }
    Ok(Some(matrix))
}

/// Read a deformation out of a `sol.present` options table.
///
/// One key per kind, so a script names the effect rather than describing a
/// mesh: `genie = { x, y, width, height, progress, spread }`. The rect is the
/// slot the window is pulled into -- a dock icon's, usually -- and `progress`
/// defaults to all the way in, since that is what one animates towards.
fn deform_from(options: &Table) -> mlua::Result<Option<crate::present::Deform>> {
    let Some(genie) = options.get::<Option<Table>>("genie")? else {
        return Ok(None);
    };
    let number = |name: &str| -> mlua::Result<f64> {
        genie.get::<Option<f64>>(name).map(|v| v.unwrap_or(0.0))
    };
    Ok(Some(crate::present::Deform::Genie {
        slot: crate::present::logical(
            (number("x")?, number("y")?),
            (
                genie.get::<Option<f64>>("width")?.unwrap_or(1.0),
                genie.get::<Option<f64>>("height")?.unwrap_or(1.0),
            ),
        ),
        progress: genie.get::<Option<f32>>("progress")?.unwrap_or(1.0),
        spread: genie.get::<Option<f32>>("spread")?.unwrap_or(1.0),
    }))
}

/// Read a list of columns out of a Lua table.
fn columns_from(columns: &Table) -> mlua::Result<Vec<solium_layout::Column>> {
    let mut out = Vec::new();
    for column in columns.sequence_values::<Table>() {
        let column = column?;
        out.push(solium_layout::Column {
            width: column.get::<Option<f64>>("width")?.unwrap_or(0.5),
            windows: column.get::<Option<usize>>("windows")?.unwrap_or(1),
        });
    }
    Ok(out)
}

/// Whether a program can be found on `PATH`.
///
/// A plain lookup rather than a spawn: asking costs nothing, and finding out by
/// running it means finding out *after* something has already failed. A desktop
/// cannot assume any particular terminal or editor is installed, and a script
/// that picks the first one that *is* beats one that silently does nothing on a
/// machine set up differently from the author's.
fn which(program: &str) -> bool {
    if program.contains('/') {
        return std::path::Path::new(program).is_file();
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|directory| directory.join(program).is_file())
}

/// Put a key combination into one canonical form.
///
/// So that `shift+super+q` and `super+shift+q` are the same binding. Done in
/// one place and used both when binding and when dispatching, because two
/// spellings of the same key is a bug that only shows up for whoever spelled it
/// the other way.
pub(crate) fn normalise_combo(combo: &str) -> String {
    let mut modifiers = Vec::new();
    let mut key = String::new();

    for part in combo.split('+') {
        let part = part.trim().to_ascii_lowercase();
        match part.as_str() {
            "" => {}
            "ctrl" | "control" => modifiers.push("ctrl"),
            "alt" | "meta" => modifiers.push("alt"),
            "shift" => modifiers.push("shift"),
            "super" | "logo" | "win" | "cmd" => modifiers.push("super"),
            _ => key = part,
        }
    }

    // A fixed order, and each modifier once.
    let mut canonical = String::new();
    for name in ["ctrl", "alt", "shift", "super"] {
        if modifiers.contains(&name) {
            canonical.push_str(name);
            canonical.push('+');
        }
    }
    canonical.push_str(&key);
    canonical
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combos_normalise_to_one_spelling() {
        assert_eq!(normalise_combo("super+space"), "super+space");
        assert_eq!(normalise_combo("Super+Space"), "super+space");
        assert_eq!(normalise_combo("shift+super+q"), "shift+super+q");
        // The whole point: written the other way round, still the same binding.
        assert_eq!(
            normalise_combo("super+shift+q"),
            normalise_combo("shift+super+q")
        );
        assert_eq!(normalise_combo("win+d"), "super+d");
        assert_eq!(normalise_combo("escape"), "escape");
    }

    #[test]
    fn a_repeated_modifier_is_not_repeated() {
        assert_eq!(normalise_combo("ctrl+ctrl+c"), "ctrl+c");
    }

    #[test]
    fn a_rect_covers_its_own_top_left_but_not_its_bottom_right() {
        let rect = Rect {
            x: 10.0,
            y: 20.0,
            w: 100.0,
            h: 50.0,
        };
        assert!(rect.contains(10.0, 20.0));
        assert!(rect.contains(109.0, 69.0));
        // Exclusive, so adjacent thumbnails cannot both claim the same pixel.
        assert!(!rect.contains(110.0, 70.0));
        assert!(!rect.contains(9.0, 20.0));
    }

    /// The whole round trip, without a compositor: a script binds a key, the
    /// binding runs, and its transforms come back as commands.
    #[test]
    fn a_script_binds_a_key_and_its_commands_come_back() {
        let directory = std::env::temp_dir().join("solium-script-test");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("Super+Space", function()
                sol.animate({ duration = 300, easing = "outBack" })
                for _, window in ipairs(sol.windows()) do
                    sol.present(window.id, { x = 0, y = 0, w = 100, h = 50 })
                end
                sol.grab_input(true)
                sol.status("test")
            end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        assert!(scripts.has_binding("super+space"));
        // Normalisation is not just for the binding side.
        assert!(scripts.has_binding("SUPER+space"));
        assert!(!scripts.has_binding("super+tab"));

        let snapshot = Snapshot {
            windows: vec![WindowInfo {
                id: 7,
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 800.0,
                    h: 600.0,
                },
                drawn: Rect::default(),
                title: "a window".to_owned(),
                focused: true,
                monitor: "test-1".to_owned(),
            }],
            monitors: vec![MonitorInfo {
                name: "test-1".to_owned(),
                area: Rect {
                    x: 0.0,
                    y: 34.0,
                    w: 1600.0,
                    h: 866.0,
                },
                whole: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 1600.0,
                    h: 900.0,
                },
                scale: 1.0,
                focused: true,
                primary: true,
                transform: "normal".to_owned(),
            }],
            work_area: Rect {
                x: 0.0,
                y: 34.0,
                w: 1600.0,
                h: 866.0,
            },
            cursor: (0.0, 0.0),
        };

        let outcome = scripts.key("super+space", snapshot);
        assert!(outcome.handled);
        assert_eq!(outcome.grab, Some(true));
        assert_eq!(outcome.status.as_deref(), Some("test"));
        assert_eq!(outcome.commands.len(), 1);

        match &outcome.commands[0] {
            Command::Present {
                id,
                rect,
                animation,
                ..
            } => {
                assert_eq!(*id, 7);
                assert_eq!(rect.map(|rect| rect.w), Some(100.0));
                // `animate` applied to the batch queued after it.
                assert_eq!(animation.duration, Duration::from_millis(300));
                assert_eq!(animation.easing, Curve::OutBack);
            }
            other => panic!("expected a present command, got {other:?}"),
        }
    }

    #[test]
    fn a_failing_script_does_not_swallow_the_key() {
        let directory = std::env::temp_dir().join("solium-script-test-error");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"sol.bind("super+e", function() error("deliberate") end)"#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let outcome = scripts.key("super+e", Snapshot::default());
        // Not handled: the key goes to the focused client rather than
        // disappearing into a broken script.
        assert!(!outcome.handled);
        assert!(outcome.commands.is_empty());
    }
}
