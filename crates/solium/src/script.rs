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
}

/// What the compositor looked like when a handler was called.
#[derive(Clone, Debug, Default)]
pub(crate) struct Snapshot {
    /// Topmost first, so hit-testing walks it in order.
    pub(crate) windows: Vec<WindowInfo>,
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
    /// End the session.
    Quit,
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
            // The user's directory first, then the config's own, then Lua's.
            // Ordered this way so dropping a single `config.lua` into
            // ~/.config/solium overrides just that file — copying the whole
            // set to change one number is not configurability.
            let mut search = format!("{directory}/?.lua;{path}");
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
                windows.set(index + 1, entry)?;
            }
            Ok(windows)
        })?,
    )?;

    sol.set(
        "monitor",
        lua.create_function(|lua, ()| snapshot(lua)?.work_area.to_table(lua))?,
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
            let easing = options
                .get::<Option<String>>("easing")?
                .and_then(|name| parse_easing(&name));
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
