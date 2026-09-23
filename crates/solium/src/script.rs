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
use mlua::{IntoLua, Lua, Table, Value};
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

/// Which window a window belongs to, as far as anything here can tell.
///
/// Three answers rather than `Option<u64>`, because "the client never named a
/// parent" and "the client named one this compositor cannot point at" are
/// different facts, and a layout that cannot tell them apart cannot say why a
/// dialog ended up in the middle of the screen instead of over its window.
///
/// The second case is not a corner: `xdg_toplevel.set_parent` may name a
/// toplevel that has not mapped yet, X11's `WM_TRANSIENT_FOR` is routinely set
/// to the root window rather than to a window we manage, and a parent that
/// closes with its dialog still up leaves the dialog behind — nothing in
/// either protocol requires the child to go with it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Parentage {
    /// No `set_parent`, no `WM_TRANSIENT_FOR`. An ordinary top-level window.
    #[default]
    None,
    /// A parent was named, and it is not a window that can be pointed at: not
    /// mapped, not ours, or gone.
    Unknown,
    /// The parent, by the id a script holds windows as.
    Window(u64),
}

impl Parentage {
    /// How a script sees it.
    ///
    /// A number when the parent can be pointed at, `false` when one was named
    /// and cannot be, and `nil` when there is none. Chosen so that the common
    /// question — "have I got a window to centre on?" — is `if window.parent
    /// then`, which is correct for all three, while a script that wants to
    /// distinguish "named but lost" from "never named" can still ask
    /// `window.parent == false`. An `Option<u64>` flattened to nil-or-number
    /// would have thrown the distinction away at the boundary.
    fn to_value(self, lua: &Lua) -> mlua::Result<Value> {
        match self {
            Self::None => Ok(Value::Nil),
            Self::Unknown => Ok(Value::Boolean(false)),
            Self::Window(id) => id.into_lua(lua),
        }
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
    /// Whether a layout should treat this as a modal dialog: something that
    /// floats over the window waiting on it rather than taking a share of the
    /// screen.
    ///
    /// On Wayland this is `xdg_dialog_v1`'s `set_modal` verbatim, read back out
    /// of `XdgToplevelSurfaceRoleAttributes::modal`.
    ///
    /// On X11 there is nothing equivalent to read. EWMH spells modality
    /// `_NET_WM_STATE_MODAL`, and smithay 0.7's `X11Surface::is_popup()` looks
    /// like it reads exactly that and does not: the state it asks is the one the
    /// window manager wrote, never the one the client set
    /// (`xwayland/xwm/surface.rs`). So the window *type* stands in for it, which
    /// is the promise #104 left open — the whole argument, and the evidence, is
    /// in `xwayland::floats_over_its_parent`.
    pub(crate) modal: bool,
    /// Which window this one belongs to, if it said. See [`Parentage`].
    pub(crate) parent: Parentage,
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
    /// The keyboard, for `sol.keyboard()` and for a shell that wants to draw
    /// a layout indicator.
    pub(crate) keyboard: crate::keymap::State,
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

/// What a deformation is aimed at, in the words a script wrote.
///
/// The compositor's own [`crate::present::Anchor`] names a surface with a
/// number, because it lives inside a `Copy` frame that is blended per node per
/// frame. A script has no numbers for surfaces and should not be given any, so
/// the name survives this far and is resolved where the command is applied —
/// which is also the first place that can see whether the surface exists.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Aim {
    /// A place. Nothing to track, and honest about it.
    Rect(Rect),
    /// A window, by the id a script holds it as.
    Window(u64),
    /// A surface a script declared — a dock, a bar, a slot in one.
    Surface(String),
}

/// A deformation as a script asked for it: the shape, and what it is aimed at.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Deform {
    pub(crate) effect: solium_effects::Deform,
    pub(crate) aim: Aim,
}

/// Who is in a named selection, in the words a script wrote.
///
/// Three lists rather than one of a sum type, because that is how a script
/// writes it — `{ windows = {...}, surfaces = {...} }` — and turning it into
/// `crate::group::Member`s is the compositor's half of the same seam `Aim`
/// crosses.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Selection {
    pub(crate) windows: Vec<u64>,
    pub(crate) surfaces: Vec<String>,
    /// Connector names. Every node drawn on one of these is in the selection.
    pub(crate) monitors: Vec<String>,
    /// Narrows the surfaces above to one monitor's instance, because a surface
    /// declared `on = "every-monitor"` is several things wearing one name.
    pub(crate) on: Option<String>,
}

/// Something a script asked the compositor to do.
#[derive(Clone, Debug)]
pub(crate) enum Command {
    Present {
        id: u64,
        rect: Option<Rect>,
        opacity: Option<f32>,
        /// A 3D transform about `pivot`, which is the drawn rect's centre
        /// unless the script moved it. `None` keeps the window flat and on the
        /// cheap path.
        matrix: Option<Mat4>,
        /// A deformation the drawn rect cannot express, such as a genie.
        deform: Option<Deform>,
        /// How deep the window is drawn. See [`crate::present::Frame::z`].
        ///
        /// Resolved rather than `Option`, unlike the four above it: nothing
        /// downstream means "left alone" — `sol.present` builds a whole frame
        /// every time — and a default kept here is one a test can reach
        /// without a compositor to run it against.
        z: f32,
        /// What `matrix` turns about, as a fraction of the drawn rect. See
        /// [`crate::present::Frame::pivot`]. Resolved here for the same reason
        /// as `z`.
        pivot: (f32, f32),
        animation: AnimationSpec,
    },
    /// Name a selection, or take the name away with `None`.
    ///
    /// The animation is for the members that *change* selection: a window that
    /// leaves one desk for another has the difference between the two lands on
    /// it in one frame, and this is how long it takes to get there. See
    /// `present::rebase`.
    Group {
        name: String,
        selection: Option<Selection>,
        animation: AnimationSpec,
    },
    /// Carry a named selection, members and all.
    PresentGroup {
        name: String,
        to: crate::group::Shift,
        animation: AnimationSpec,
    },
    /// Carry one back to doing nothing, and stop carrying it.
    ClearGroup {
        name: String,
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
    /// Which layouts the keyboard has, which is live, and how keys repeat.
    Keyboard(crate::keymap::Request),
    /// A surface for the compositor to draw in QML: a wallpaper, a bar, an
    /// overlay. Boxed because it is much larger than the other variants and an
    /// enum is as big as its widest arm.
    Surface(Box<crate::scripted::Declaration>),
    /// Take one away, by name.
    SurfaceGone(String),
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
    /// What fills a window between an edge drag asking for a size and the
    /// client painting it. See `crate::resizing::Settings`.
    Resize(crate::resizing::Settings),
    /// Which XCursor theme the pointer is drawn from, and how big it is.
    ///
    /// Carries what the *configuration* said and nothing else — `None` in a
    /// field means "`config.lua` did not say", which is what lets
    /// `XCURSOR_THEME` and `XCURSOR_SIZE` be consulted next. The precedence is
    /// resolved in `cursor::theme::Settings::resolve`, not here, so that it is
    /// in one place and testable without Lua.
    Cursor(crate::cursor::theme::Configured),
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

/// What a script handed the host to hold while the Lua state is replaced.
///
/// Plain data, because that is the only thing that can cross. A reload builds
/// a whole new [`Lua`]; a table, a closure, an upvalue — every Lua value there
/// is — dies with the old one. So what crosses is this, and it is rebuilt as a
/// fresh table in the new state.
///
/// Tables are a list of pairs rather than a map for two reasons. Lua has one
/// table type that is both a list and a dictionary, and both shipped keeps are
/// dictionaries with keys of different types: `workspaces.of` is keyed by
/// window id, which is an integer, and `workspaces.showing` by connector name,
/// which is a string. And a `Vec` of pairs needs no `Hash` or `Eq` on the key,
/// which a float key could not honestly have.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Kept {
    Bool(bool),
    /// Kept apart from `Number` so a window id survives as the integer it is.
    /// Round-tripped through `f64` it would come back as `7.0`, which is a
    /// *different table key* in Lua from `7` — so every entry in
    /// `workspaces.of` would be unreachable by the id that wrote it.
    Int(i64),
    Number(f64),
    Text(String),
    Table(Vec<(Kept, Kept)>),
}

/// Everything kept, under the names the scripts gave it.
pub(crate) type Keep = std::collections::HashMap<String, Kept>;

/// How deep a kept table is followed.
///
/// A budget rather than a cycle check. A table holding itself — directly, or
/// around through three others — would otherwise be copied until the stack ran
/// out, and the compositor does not get to die because a configuration made a
/// ring. Nothing that belongs in a keep is eight tables deep; what is cut is
/// named in the log rather than dropped in silence, because half a table kept
/// quietly is worse than a table that was not kept at all.
const KEEP_DEPTH: usize = 8;

impl Kept {
    /// Read a Lua value, or say why it cannot be kept.
    ///
    /// `where_it_is` is the path from the keep's name down to this value, so a
    /// warning names the key someone can go and look at rather than saying
    /// "something in your table".
    fn read(value: &Value, depth: usize, where_it_is: &str) -> Option<Self> {
        match value {
            Value::Boolean(flag) => Some(Self::Bool(*flag)),
            Value::Integer(number) => Some(Self::Int(*number)),
            Value::Number(number) => Some(Self::Number(*number)),
            Value::String(text) => text.to_str().ok().map(|text| Self::Text(text.to_owned())),
            Value::Table(table) => {
                if depth >= KEEP_DEPTH {
                    tracing::warn!(
                        key = where_it_is,
                        depth = KEEP_DEPTH,
                        "a kept table is deeper than the host will follow; this branch of it \
                         will not survive the next reload"
                    );
                    return None;
                }
                let mut pairs = Vec::new();
                for (key, value) in table.pairs::<Value, Value>().flatten() {
                    let named = format!("{where_it_is}.{}", describe(&key));
                    let (Some(key), Some(value)) = (
                        Self::read(&key, depth + 1, &named),
                        Self::read(&value, depth + 1, &named),
                    ) else {
                        continue;
                    };
                    pairs.push((key, value));
                }
                Some(Self::Table(pairs))
            }
            // `nil` is not a failure: a key that has been cleared is a key that
            // is not there, and Lua's own iteration never yields one.
            Value::Nil => None,
            other => {
                tracing::warn!(
                    key = where_it_is,
                    kind = other.type_name(),
                    "only plain data survives a reload, and this is not plain data; it will be \
                     missing from the keep when the configuration is read again"
                );
                None
            }
        }
    }

    /// Build it again, in the Lua state that replaced the one it came from.
    fn into_value(self, lua: &Lua) -> mlua::Result<Value> {
        Ok(match self {
            Self::Bool(flag) => Value::Boolean(flag),
            Self::Int(number) => Value::Integer(number),
            Self::Number(number) => Value::Number(number),
            Self::Text(text) => Value::String(lua.create_string(&text)?),
            Self::Table(pairs) => {
                let table = lua.create_table()?;
                for (key, value) in pairs {
                    table.set(key.into_value(lua)?, value.into_value(lua)?)?;
                }
                Value::Table(table)
            }
        })
    }
}

/// A Lua value as a log line should name it.
fn describe(value: &Value) -> String {
    match value {
        Value::Integer(number) => number.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.to_string_lossy(),
        other => format!("<{}>", other.type_name()),
    }
}

/// What the configuration being replaced asked to keep, waiting in the new Lua
/// state for `sol.keep` to claim it.
///
/// App data rather than a field on [`Scripts`], because `sol.keep` is called
/// while the configuration is still being *read* — the whole point is that a
/// script's top level has its state back before it does anything with it — and
/// at that moment there is no `Scripts` yet, only a `Lua`.
#[derive(Debug, Default)]
struct Carried(Keep);

/// A key binding as `solium --check` reports it.
///
/// The note is what the script said about where the binding came from, and is
/// `None` for the shipped ones -- which is most of them, and is why `--check`
/// stays a plain list of combinations until a configuration has something to
/// add. See `sol.bind`'s third argument.
#[derive(Debug)]
pub(crate) struct Binding {
    pub(crate) combo: String,
    pub(crate) note: Option<String>,
}

/// A setting a configuration wrote that nothing reads.
///
/// `meant` is the nearest key that does exist, when `config.lua` found one
/// close enough to be worth naming.
#[derive(Debug)]
pub(crate) struct UnknownSetting {
    pub(crate) key: String,
    pub(crate) meant: Option<String>,
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

    /// Every binding the configuration registered, in its canonical spelling.
    ///
    /// Sorted, because this is read by a person comparing one run to the next.
    pub(crate) fn bindings(&self) -> Vec<Binding> {
        let Ok(sol) = self.lua.globals().get::<Table>("sol") else {
            return Vec::new();
        };
        let Ok(bindings) = sol.get::<Table>("_bindings") else {
            return Vec::new();
        };
        let sources = sol.get::<Table>("_binding_sources").ok();
        let mut names: Vec<Binding> = bindings
            .pairs::<String, Value>()
            .filter_map(Result::ok)
            .map(|(combo, _)| Binding {
                note: sources
                    .as_ref()
                    .and_then(|sources| sources.get::<Option<String>>(combo.clone()).ok())
                    .flatten(),
                combo,
            })
            .collect();
        names.sort_by(|left, right| left.combo.cmp(&right.combo));
        names
    }

    /// Bindings a script deliberately took away.
    ///
    /// A combination somebody said something about and which is now bound to
    /// nothing -- which is exactly `sol.unbind`. Reported separately from the
    /// list above because it is the one thing that list cannot show: a key
    /// that is missing on purpose looks, in a list of what survived, identical
    /// to one that was never there.
    pub(crate) fn unbound(&self) -> Vec<Binding> {
        let Ok(sol) = self.lua.globals().get::<Table>("sol") else {
            return Vec::new();
        };
        let (Ok(bindings), Ok(sources)) = (
            sol.get::<Table>("_bindings"),
            sol.get::<Table>("_binding_sources"),
        ) else {
            return Vec::new();
        };
        let mut gone: Vec<Binding> = sources
            .pairs::<String, String>()
            .filter_map(Result::ok)
            .filter(|(combo, _)| {
                !matches!(bindings.get::<Value>(combo.clone()), Ok(Value::Function(_)))
            })
            .map(|(combo, note)| Binding {
                combo,
                note: Some(note),
            })
            .collect();
        gone.sort_by(|left, right| left.combo.cmp(&right.combo));
        gone
    }

    /// Settings the configuration wrote that its defaults do not define.
    ///
    /// In the order `config.lua` found them, which is `pairs` order and so is
    /// arbitrary -- sorted here, because the only reader is a person checking a
    /// file. See `sol.unknown`.
    pub(crate) fn unknown_settings(&self) -> Vec<UnknownSetting> {
        let Ok(sol) = self.lua.globals().get::<Table>("sol") else {
            return Vec::new();
        };
        let Ok(unknown) = sol.get::<Table>("_unknown_settings") else {
            return Vec::new();
        };
        let mut found: Vec<UnknownSetting> = unknown
            .sequence_values::<Table>()
            .filter_map(Result::ok)
            .filter_map(|entry| {
                Some(UnknownSetting {
                    key: entry.get::<String>("key").ok()?,
                    meant: entry.get::<Option<String>>("meant").ok().flatten(),
                })
            })
            .collect();
        found.sort_by(|left, right| left.key.cmp(&right.key));
        found
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

        crate::assets::lua().join("init.lua")
    }

    /// Load the configuration script and everything it pulls in, cold.
    ///
    /// Nothing is carried, because at startup there is nothing to carry. A
    /// reload goes through [`Self::load_carrying`].
    pub(crate) fn load(config: &Path) -> Result<Self> {
        Self::load_carrying(config, Keep::new())
    }

    /// Everything the running scripts asked the host to keep.
    ///
    /// Read off the *old* `Scripts` before the new ones are built, which is
    /// the only moment both exist. Reading rather than moving: a load that
    /// fails leaves the running configuration alone — a typo costs a log line,
    /// not the session — and that promise only holds if collecting the keep
    /// cannot damage the scripts it was collected from.
    pub(crate) fn kept(&self) -> Keep {
        let Ok(sol) = self.lua.globals().get::<Table>("sol") else {
            return Keep::new();
        };
        let Ok(keeps) = sol.get::<Table>("_keeps") else {
            return Keep::new();
        };
        keeps
            .pairs::<String, Value>()
            .filter_map(Result::ok)
            .filter_map(|(name, value)| Kept::read(&value, 0, &name).map(|kept| (name, kept)))
            .collect()
    }

    /// Load the configuration again, handing back what the last one kept.
    ///
    /// ## What a script is entitled to know after a reload
    ///
    /// This is the contract, and it is deliberately short, because everything
    /// in it is something the host has to keep true for ever:
    ///
    ///  1. **Whatever it handed to `sol.keep`, and nothing else.** A reload
    ///     throws the whole Lua state away, so a script's own variables are
    ///     gone by construction. `sol.keep` is the one exception, it is opt-in,
    ///     and it holds plain data only — see [`Kept`].
    ///  2. **The world as it now is, re-announced.** `restore`, then
    ///     `monitors`, then `layout` — see [`crate::state::Solium::reload`].
    ///     A script that can rebuild itself from `sol.windows()` and
    ///     `sol.monitors()` needs no keep at all.
    ///
    /// Anything else a script believed is gone, and that is the point: the
    /// alternative is every script inventing its own answer, which is what
    /// `workspaces.lua` and `modes.lua` had each done — differently, and one
    /// of them wrongly. See the comment at the top of `lua/modes.lua`.
    pub(crate) fn load_carrying(config: &Path, carried: Keep) -> Result<Self> {
        let lua = Lua::new();
        lua.set_app_data(Pending::default());
        lua.set_app_data(Snapshot::default());
        lua.set_app_data(Carried(carried));

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
            //
            // *Where* the shipped directory is stopped being a constant here
            // when the compositor became installable: `crate::assets` answers
            // that once, for the Lua and the QML together. What is local to
            // this function is only the order.
            let shipped_lua = crate::assets::lua();
            let shipped = shipped_lua.display();
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
    ///
    /// `(id, edge_x, edge_y, horizontal_side, vertical_side)`.
    ///
    /// This signature line has been wrong twice, so it is written out rather
    /// than summarised. It said `(id, x, y, horizontal, vertical)` until now —
    /// the names of the *pre-#120* booleans, which that issue replaced with
    /// sides in the code and forgot to replace here — and the prose under it
    /// called the first pair "the delta" for the whole life of the event and
    /// then "where the pointer is", neither of which it is any more.
    ///
    /// `edge_x, edge_y` is **where the dragged edge should come to rest** on
    /// each axis, in the coordinates `sol.place` and `tree:layout` already
    /// speak. A position and not a delta, so handling the same drag twice gives
    /// the same layout; derived from the drag's own rectangle rather than from
    /// the seat, so the edge moves *with* the pointer instead of jumping to it.
    /// That is #124, and the difference is most of a window's width on a
    /// `super`+right-button drag, which begins in the middle of one. See
    /// [`crate::input::resize::dragged_edge`].
    ///
    /// On an axis this drag does not move there is no such edge, and the
    /// pointer's own coordinate is passed through there instead. A handler that
    /// checks its side before using the coordinate — which is what the side is
    /// for — never sees it.
    ///
    /// `horizontal_side, vertical_side` are the *side* of the window being
    /// dragged on each axis: `"left"`/`"right"`, `"top"`/`"bottom"`, or nil
    /// where that axis is not in play. They were a pair of booleans until #120;
    /// a script that only tested them for truthiness still reads the same,
    /// because a side is truthy and nil is not, but one that passes them on now
    /// passes on something a layout can choose a seam from. See
    /// [`crate::input::resize::sides`].
    pub(crate) fn resized(
        &mut self,
        id: u64,
        edge_at: (f64, f64),
        sides: (Option<&'static str>, Option<&'static str>),
        snapshot: Snapshot,
    ) -> Outcome {
        self.dispatch(snapshot, move |sol| {
            call_listeners(sol, "resize", (id, edge_at.0, edge_at.1, sides.0, sides.1))
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
    /// The set of monitors changed: one arrived, or one went away.
    ///
    /// Separate from `layout` because it is a different question. `layout`
    /// asks a mode to arrange the windows it already knows about; this says
    /// the screens themselves are not the screens they were, so a mode holding
    /// per-monitor state -- which both shipped layouts do, one tree or one
    /// view per screen -- has to re-home the windows whose monitor is gone
    /// before arranging anything.
    ///
    /// Without it a window on an unplugged monitor is in a tree that nothing
    /// iterates, because `tiling.apply` walks the monitors that *exist*. It
    /// keeps its old slot, which is now off every screen, and it reappears
    /// there when the monitor comes back -- which is exactly what a hardware
    /// test found.
    pub(crate) fn monitors_changed(&mut self, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |sol| call_listeners(sol, "monitors", ()))
    }

    /// These scripts have replaced a running session's, rather than started one.
    ///
    /// The first half of the reload contract on [`Self::load_carrying`], and
    /// the half a keep cannot cover. `sol.keep` gives a script its *data* back;
    /// this is the moment it may act on it — after every script has loaded, so
    /// a mode restored here can be sure the layout it names has registered
    /// itself, which it cannot be at its own top level.
    ///
    /// It does not fire at startup, and that asymmetry is the whole meaning of
    /// the event: a cold start has nothing to restore, and a script that
    /// listens for this is saying "this is what I do differently when I am not
    /// the first configuration this session has had".
    pub(crate) fn restored(&mut self, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |sol| call_listeners(sol, "restore", ()))
    }

    /// A scripted surface was pressed and asked for something.
    pub(crate) fn surface_action(
        &mut self,
        name: &str,
        action: &str,
        snapshot: Snapshot,
    ) -> Outcome {
        let name = name.to_owned();
        let action = action.to_owned();
        self.dispatch(snapshot, move |sol| {
            call_listeners(sol, "surface", (name.clone(), action.clone()))
        })
    }

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

/// Which way a script means a seam to run.
///
/// "width" is the seam that bounds a window's width, and that seam is cut
/// *vertically*. The two names disagree, which is exactly why this is a named
/// function: `axis == "width"` producing `Vertical` reads like a bug at the
/// call site, and has been reported as one.
///
/// Anything else is "height", because the caller is a keyboard binding
/// choosing between two axes and there is no third answer to fall back to.
/// [`edge_named`] is stricter for the opposite reason: it has four answers and
/// a wrong guess moves a seam the user did not touch.
fn axis_named(name: &str) -> solium_layout::tree::Axis {
    if name == "width" {
        solium_layout::tree::Axis::Vertical
    } else {
        solium_layout::tree::Axis::Horizontal
    }
}

/// Which side of a window a script means.
///
/// The spellings are the ones the `resize` event emits — see
/// [`crate::input::resize::sides`] — so the ordinary script is passing back a
/// value the compositor just handed it and cannot misspell. `None` for
/// anything else, so a hand-written one is told rather than silently given a
/// side it did not ask for.
fn edge_named(name: &str) -> Option<solium_layout::tree::Edge> {
    use solium_layout::tree::Edge;
    match name {
        "left" => Some(Edge::Left),
        "right" => Some(Edge::Right),
        "top" => Some(Edge::Top),
        "bottom" => Some(Edge::Bottom),
        _ => None,
    }
}

/// A Lua number, whatever Lua happened to store it as.
///
/// `widths = { 1/3, 1/2, 1 }` puts two floats and an *integer* in one list, and
/// asking mlua for an `f64` on the wrong one is a conversion error rather than
/// a None -- which, with a `?` on it, would take the whole configuration down
/// over a list that is perfectly well written. Same reasoning as `scene` and
/// `cursor.size`, which are both read this way and say so.
fn number(value: &Value) -> Option<f64> {
    match value {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a share of a view, written as a whole number: 1, not 2^53"
        )]
        Value::Integer(whole) => Some(*whole as f64),
        Value::Number(fraction) => Some(*fraction),
        _ => None,
    }
}

/// Build a scrolling strip from `config.scrolling`.
///
/// Both of these keys were documented in `lua/config.lua` and read by nothing
/// until #117: the strip used the constant list in `solium_layout`, so a
/// configuration naming quarters got thirds, and the person who wrote it could
/// not tell whether they had misunderstood the setting or been ignored.
///
/// **A value that is not a share of a view is dropped with a line in the log,
/// not clamped and not fatal.** The same policy as `cursor.size`, for the same
/// reason: a column at 0.001 of the view is a column nobody can see and nobody
/// asked for, so guessing what was meant is worse than saying so and carrying
/// on. Fatal is wrong here too -- this runs while the configuration is being
/// read, and a hard error over one number would cost every other setting in the
/// file, the layouts and the bindings included.
fn scroller_from(options: &Table) -> mlua::Result<solium_layout::scroller::Scroller> {
    let mut widths = Vec::new();
    if let Ok(Value::Table(list)) = options.get::<Value>("widths") {
        for (index, value) in list.sequence_values::<Value>().flatten().enumerate() {
            match number(&value) {
                // Above 1.0 refused as well as below 0: a column wider than the
                // view can never be brought fully into view, so the strip would
                // scroll for ever trying to. `layout` clamps to 1.0 anyway,
                // which is how this used to be silent.
                Some(share) if share.is_finite() && share > 0.0 && share <= 1.0 => {
                    widths.push(share);
                }
                _ => tracing::warn!(
                    target: "solium::script",
                    at = index + 1,
                    "scrolling.widths holds something that is not a share of the view \
                     (a number above 0 and at most 1); that entry is ignored"
                ),
            }
        }
    }
    // 1-based, the way Lua counts and the way `config.lua` documents it. The
    // subtraction belongs at this boundary and nowhere deeper.
    //
    // Filtered before the cast rather than clamped after it, and `f64::clamp`
    // is deliberately not used -- clippy will offer it. `clamp` *returns* NaN
    // for a NaN input, `NaN as usize` saturates to 0, and `0 - 1` on a `usize`
    // is the underflow. Nobody writes `default_width = 0/0` on purpose, but a
    // generated configuration can produce one, and "the compositor panicked"
    // is not an acceptable answer to a number in a settings file.
    let start = match options.get::<Value>("default_width") {
        // Not written at all, which is the ordinary case and nothing to say
        // about. `Nil` rather than an error: mlua answers a missing key with
        // one, so this arm is "the setting is absent" and the next is "the
        // setting is there and is not an index".
        Ok(Value::Nil) | Err(_) => 0,
        Ok(value) => match number(&value).filter(|index| index.is_finite() && *index >= 1.0) {
            Some(index) => {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "finite, at least 1, and capped at 1000 on the line \
                                  itself; a fractional index is truncated on purpose"
                )]
                let index = index.min(1000.0) as usize;
                index.saturating_sub(1)
            }
            None => {
                tracing::warn!(
                    target: "solium::script",
                    "scrolling.default_width is not an index into scrolling.widths \
                     (a whole number from 1 up); a new column opens at the first \
                     width instead"
                );
                0
            }
        },
    };
    // Against the list that is actually in force, which is `PRESETS` when the
    // configuration's own list turned out to hold nothing usable. Gated on
    // `!widths.is_empty()` until #117's review, which meant a file whose every
    // width was rejected got a line per width and nothing at all about the
    // index -- so the clamp, the one step that decides what a new column opens
    // at, was the only silent thing left in a read that says everything else
    // out loud.
    let in_force = if widths.is_empty() {
        solium_layout::scroller::PRESETS.len()
    } else {
        widths.len()
    };
    if start >= in_force {
        tracing::warn!(
            target: "solium::script",
            default_width = start + 1,
            widths = in_force,
            // Named, because "not in scrolling.widths" is confusing advice when
            // the list being counted against is not the one in the file.
            fallback = widths.is_empty(),
            "scrolling.default_width names a width that is not in the list of widths \
             in force; a new column opens at the last one instead"
        );
    }
    Ok(solium_layout::scroller::Scroller::with_widths(
        &widths, start,
    ))
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
    // Takes `config.scrolling` -- the whole section, not a rewrapping of it --
    // and reads the two keys that describe the strip itself. Optional, because
    // a script may want a plain one and because a `config.lua` written before
    // #117 has nothing to hand over.
    layout.set(
        "scroller",
        lua.create_function(|_, options: Option<Table>| {
            let Some(options) = options else {
                return Ok(Scrolling::default());
            };
            Ok(Scrolling(scroller_from(&options)?))
        })?,
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
    sol.set("_keeps", lua.create_table()?)?;
    // Where a binding came from, for the combinations a script chose to say.
    // Keyed the same way `_bindings` is -- the canonical spelling -- so the two
    // can be read together, and holding entries for combinations `_bindings`
    // does *not* have, which is how `sol.unbind` reports a shipped binding
    // somebody deliberately took away. See `Scripts::bindings`.
    sol.set("_binding_sources", lua.create_table()?)?;
    // Settings a configuration wrote that the defaults do not define. Filled by
    // `lua/config.lua`, whose `merge` is the only thing that knows both halves
    // of that comparison, and read by `solium --check`. See `sol.unknown`.
    sol.set("_unknown_settings", lua.create_table()?)?;

    // State that outlives `super+shift+r`.
    //
    //     local state = sol.keep("workspaces", { showing = {}, of = {} })
    //
    // Answers the table the last configuration had under this name, or the
    // defaults on the first load. Mutate it in place and the next reload gets
    // what you left in it; see [`Kept`] for what may be in one.
    //
    // **Why this is the host's job and not a script's.** Two shipped scripts
    // had already invented an answer to surviving a reload, and neither could
    // have got it right, because only the host knows when the Lua state dies.
    // `workspaces.lua` reasoned about the *compositor's* lifetime instead —
    // a selection outlives a reload, so it swept sixteen desk names once per
    // session to clear ones a shorter arrangement had abandoned. `modes.lua`
    // reasoned about nothing at all: it declared `current = "floating"` at its
    // top level, which is true at startup and a lie after every reload, with
    // every window still sitting in the tile a layout put it in.
    //
    // Those are the same defect. A script cannot see the seam it is being cut
    // at, so the seam is where the mechanism belongs.
    sol.set(
        "keep",
        lua.create_function(|lua, (name, defaults): (String, Table)| {
            let sol: Table = lua.globals().get("sol")?;
            let keeps: Table = sol.get("_keeps")?;
            // Asked for twice under one name — two scripts sharing it, or one
            // module required from two places — is the same table both times.
            // Handing out a second would make whichever was harvested last the
            // only one kept, which is a loss nothing would report.
            if let Value::Table(already) = keeps.get::<Value>(name.as_str())? {
                return Ok(already);
            }
            // Cloned out, and the borrow of the app data dropped, before any
            // table is built: `into_value` calls back into Lua.
            let carried = lua
                .app_data_ref::<Carried>()
                .and_then(|carried| carried.0.get(&name).cloned());
            let table = match carried {
                // Only a table is claimable. A keep that came back as a number
                // would mean the script changed shape between reloads, and the
                // defaults it just passed are the better answer.
                Some(kept @ Kept::Table(_)) => match kept.into_value(lua)? {
                    Value::Table(table) => table,
                    _ => defaults,
                },
                _ => defaults,
            };
            keeps.set(name, &table)?;
            Ok(table)
        })?,
    )?;

    // Every window a layout may arrange, topmost first.
    //
    // `modal` and `parent` are here rather than behind a `sol.dialog(id)` of
    // their own, and that is the point: a layout already walks this list once
    // per pass, and a second call per window to ask whether it is a dialog is a
    // second chance for the two answers to come from different moments. The
    // snapshot is one instant by construction — see the module header — and
    // anything a layout has to decide with belongs inside it.
    //
    // `parent` is a number, `false`, or absent; see `Parentage::to_value` for
    // why the middle one exists.
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
                entry.set("modal", window.modal)?;
                entry.set("parent", window.parent.to_value(lua)?)?;
                windows.set(index + 1, entry)?;
            }
            Ok(windows)
        })?,
    )?;

    // Anything the compositor should draw in QML: a wallpaper, a bar, a dock,
    // a heads-up display.
    //
    //     sol.surface("wallpaper", {
    //         scene = "wallpaper.qml",
    //         layer = "background",
    //         on    = "every-monitor",
    //         properties = { source = "…" },
    //     })
    //
    // `interactive = true` lets the pointer reach it: the scene sets an
    // `action` string and a `sol.on("surface", …)` handler is told about it,
    // which is how the tweaks panel works and how a bar's buttons would.
    //
    // `sol.surface(name, false)` takes one away. Re-declaring the same name
    // replaces it, so running the configuration again is idempotent.
    //
    // This is the primitive the wallpaper used to be a special case of. See
    // `scripted.rs` for why it is worth having rather than a `Command` each.
    sol.set(
        "surface",
        lua.create_function(|lua, (name, options): (String, Value)| {
            let options = match options {
                Value::Table(options) => options,
                // `false` and `nil` both remove it, so a configuration that
                // stops declaring something and one that declares it off mean
                // the same thing.
                _ => {
                    with_pending(lua, |pending| {
                        pending.commands.push(Command::SurfaceGone(name.clone()));
                    })?;
                    return Ok(Value::Nil);
                }
            };

            let scene: String = options.get("scene")?;
            let Some(scene) = crate::scripted::find_scene(&scene) else {
                // Named rather than ignored: a scene that is not there is a
                // typo, and a surface that silently does not appear is the
                // hardest kind of configuration mistake to find.
                tracing::error!(surface = name, scene, "no such QML scene");
                return Ok(Value::Nil);
            };

            let layer = options
                .get::<Option<String>>("layer")?
                .as_deref()
                .and_then(crate::scripted::Layer::parse)
                .unwrap_or_default();

            let on = match options.get::<Value>("on")? {
                Value::String(where_) => match where_.to_str()?.as_ref() {
                    "primary" => crate::scripted::On::Primary,
                    "every-monitor" => crate::scripted::On::EveryMonitor,
                    monitor => crate::scripted::On::Monitor(monitor.to_owned()),
                },
                Value::Table(rect) => match rect_from(&rect)? {
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "a rect from a script is screen-sized"
                    )]
                    Some(rect) => crate::scripted::On::Rect(smithay::utils::Rectangle::new(
                        (rect.x.round() as i32, rect.y.round() as i32).into(),
                        (
                            (rect.w.round() as i32).max(1),
                            (rect.h.round() as i32).max(1),
                        )
                            .into(),
                    )),
                    None => crate::scripted::On::EveryMonitor,
                },
                _ => crate::scripted::On::EveryMonitor,
            };

            let properties = match options.get::<Option<Table>>("properties")? {
                Some(table) => json_object(&table)?,
                None => "{}".to_owned(),
            };
            let interactive = options.get::<Option<bool>>("interactive")?.unwrap_or(false);

            with_pending(lua, |pending| {
                pending
                    .commands
                    .push(Command::Surface(Box::new(crate::scripted::Declaration {
                        name: name.clone(),
                        scene: scene.clone(),
                        layer,
                        on: on.clone(),
                        properties: properties.clone(),
                        interactive,
                    })));
            })?;
            Ok(Value::Nil)
        })?,
    )?;

    // The keyboard: which layouts exist, which is live, how keys repeat.
    //
    // Reads with no argument and configures with a table, the same shape as
    // `sol.monitors` below. Switching layout is `sol.keyboard{ active = 2 }`
    // rather than a second function, because "which layout is live" is a
    // property of the keyboard and not a different subject.
    //
    // Every key is optional and an absent one is left alone -- so a binding
    // that switches layout does not quietly reset the repeat rate somebody
    // configured, and a configuration that says nothing about the keyboard
    // leaves the `XKB_DEFAULT_*` environment in force.
    sol.set(
        "keyboard",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            let Some(options) = options else {
                let keyboard = snapshot(lua)?.keyboard;
                let table = lua.create_table()?;
                let layouts = lua.create_table()?;
                for (index, name) in keyboard.layouts.iter().enumerate() {
                    layouts.set(index + 1, name.clone())?;
                }
                table.set("layouts", layouts)?;
                table.set("active", keyboard.active)?;
                table.set("repeat_rate", keyboard.repeat_rate)?;
                table.set("repeat_delay", keyboard.repeat_delay)?;
                return Ok(Value::Table(table));
            };

            let text =
                |key: &str| -> mlua::Result<Option<String>> { options.get::<Option<String>>(key) };
            let rules = text("rules")?;
            let model = text("model")?;
            let layout = text("layout")?;
            let variant = text("variant")?;
            let keyboard_options = text("options")?;

            // A keymap is compiled only when a script named one of its parts.
            // Recompiling to change the active layout would throw away the
            // modifier state with it, and recompiling on every reload would
            // make every client rebuild its xkb state for nothing.
            let names = [&rules, &model, &layout, &variant, &keyboard_options];
            let keymap = names.iter().any(|name| name.is_some()).then(|| {
                crate::keymap::Keymap {
                    rules: rules.clone().unwrap_or_default(),
                    model: model.clone().unwrap_or_default(),
                    layout: layout.clone().unwrap_or_default(),
                    variant: variant.clone().unwrap_or_default(),
                    // `None`, not `Some("")`. An empty string is a real
                    // instruction to xkb meaning "no options at all", which is
                    // not the same as "whatever the environment says".
                    options: keyboard_options.clone().filter(|it| !it.is_empty()),
                }
            });

            let rate = options.get::<Option<i32>>("repeat_rate")?;
            let delay = options.get::<Option<i32>>("repeat_delay")?;
            let repeat = match (rate, delay) {
                (None, None) => None,
                // One given and not the other keeps the other as it is, which
                // is what every other key in this table does.
                (rate, delay) => Some((
                    rate.unwrap_or(crate::keymap::REPEAT_RATE),
                    delay.unwrap_or(crate::keymap::REPEAT_DELAY),
                )),
            };

            let request = crate::keymap::Request {
                keymap,
                repeat,
                active: options.get::<Option<usize>>("active")?,
            };
            if request == crate::keymap::Request::default() {
                return Ok(Value::Nil);
            }
            with_pending(lua, |pending| {
                pending.commands.push(Command::Keyboard(request.clone()));
            })?;
            Ok(Value::Nil)
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

                let name_for_warning = name.clone();
                places.push(crate::monitor::Placement {
                    name,
                    at,
                    beside,
                    mode,
                    vrr: row.get::<Option<bool>>("vrr")?,
                    transform,
                    enabled: row.get::<Option<bool>>("enabled")?.unwrap_or(true),
                    primary: row.get::<Option<bool>>("primary")?.unwrap_or(false),
                    scale: match row.get::<Value>("scale") {
                        Ok(Value::Number(value)) => match crate::monitor::scaling(&value) {
                            Some(scale) => scale,
                            None => {
                                tracing::warn!(
                                    monitor = name_for_warning,
                                    scale = value,
                                    "not a sensible scale -- between 0.5 and 8, or \"auto\""
                                );
                                crate::monitor::Scaling::Auto
                            }
                        },
                        Ok(Value::Integer(value)) => {
                            #[expect(
                                clippy::cast_precision_loss,
                                reason = "a scale is a small number"
                            )]
                            let value = value as f64;
                            crate::monitor::scaling(&value).unwrap_or(crate::monitor::Scaling::Auto)
                        }
                        // "auto", and anything unreadable, which means the
                        // same thing and has already been warned about.
                        _ => crate::monitor::Scaling::Auto,
                    },
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
            let (rect, opacity, matrix, deform) = match options.as_ref() {
                Some(options) => (
                    rect_from(options)?,
                    options.get::<Option<f32>>("opacity")?,
                    transform_from(options)?,
                    deform_from(options)?,
                ),
                None => (None, None, None, None),
            };
            // Outside the match, because these two resolve to a value where
            // the four above resolve to "said nothing": `sol.present(id)` and
            // `sol.present(id, {})` have to produce the same depth and the
            // same pivot, and one function answering for both tables is how
            // the two answers cannot drift apart.
            let z = depth_from(options.as_ref())?;
            let pivot = pivot_from(options.as_ref())?;
            with_pending(lua, |pending| {
                let animation = pending.animation;
                pending.commands.push(Command::Present {
                    id,
                    rect,
                    opacity,
                    matrix,
                    deform,
                    z,
                    pivot,
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
    // Whether the compositor was started with `--debug-mode`.
    //
    // A script's gate rather than the compositor's: the Developer Tweaks panel
    // used to be gated in Rust, and now `lua/tweaks.lua` declines to declare
    // itself. Anything else that only belongs in a development session can do
    // the same.
    sol.set(
        "debug_mode",
        lua.create_function(|_, ()| Ok(crate::dev::debug_mode()))?,
    )?;

    sol.set(
        "reload",
        lua.create_function(|lua, ()| {
            with_pending(lua, |pending| pending.commands.push(Command::Reload))
        })?,
    )?;

    sol.set(
        "pane",
        lua.create_function(|lua, name: Option<String>| {
            with_pending(lua, |pending| {
                pending.commands.push(Command::Decoration { name });
            })
        })?,
    )?;

    // The old name for `sol.pane`. A style used to be a single QML file and
    // is now a folder; the name changed with it. Kept so no configuration
    // written before the change breaks, and cheap enough to keep until there
    // is a reason to remove it.
    sol.set("decoration", sol.get::<mlua::Function>("pane")?)?;

    // Everything `sol.pane` could be handed, discovered from the
    // directories the compositor actually resolves against rather than from a
    // list kept in Lua. Same principle as `parse_easing`: the names come from
    // the machinery, so a style someone writes is offerable the moment it
    // exists and `lua/tweaks.lua` has nothing to edit.
    //
    // `{ name = "...", kind = "bundle" | "file" }` per entry, sorted, bundles
    // first, and each name appearing once -- the shadowing is already applied,
    // so a script can offer the list as it stands without implying a choice
    // the compositor would not make.
    sol.set(
        "decorations",
        lua.create_function(|lua, ()| {
            let list = lua.create_table()?;
            for (index, offered) in crate::decoration::available().into_iter().enumerate() {
                let entry = lua.create_table()?;
                entry.set("name", offered.name)?;
                entry.set("kind", offered.kind.as_str())?;
                list.set(index + 1, entry)?;
            }
            Ok(list)
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

    // What fills a window while a resize drag is ahead of its client.
    //
    // `Option<Table>` and not `Table`, for the reason `sol.cursor_theme` below
    // spells out at length: the documented way to override the configuration is
    // one `~/.config/solium/config.lua`, and a copy written before this setting
    // existed has no `resize` key at all. A `Table` parameter would fail the
    // *whole configuration* over a setting nobody asked for -- no layouts, no
    // bindings, no windows placed.
    sol.set(
        "resize",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            let options = match options {
                Some(table) => table,
                None => lua.create_table()?,
            };
            let mut resize = crate::resizing::Settings::default();
            // Read as a `Value` and matched rather than asked for as an
            // `Option<String>`: mlua *errors* on anything that is not
            // string-like instead of answering None, and the `?` would take the
            // whole handler down with it. Same reasoning as `scene` above, and
            // it is how bezier easings once silently stopped working.
            //
            // A name that is not one of the three is named in the log and the
            // default kept, rather than guessed at. Someone who wrote `fill =
            // "stretched"` wants to be told, and a compositor that silently
            // picks for them is one they cannot debug.
            if let Ok(Value::String(name)) = options.get::<Value>("fill")
                && let Ok(name) = name.to_str()
            {
                match crate::resizing::Fill::named(&name) {
                    Some(fill) => {
                        // Said out loud rather than left to be discovered.
                        // `scene` is a real setting and its render path is
                        // real, but the only pane that has a scene to draw is
                        // one whose application has not painted yet -- so on
                        // an ordinary window it is `hold` today, and someone
                        // who chose it and saw no difference deserves to be
                        // told why rather than left doubting their config.
                        // `crate::resizing::Fill::Scene` carries the whole of
                        // it, including what arming one would cost.
                        if fill == crate::resizing::Fill::Scene {
                            tracing::warn!(
                                "resize.fill = \"scene\" only draws a scene a window already \
                                 has, which today means one resized before its application \
                                 painted; anywhere else it behaves as \"hold\""
                            );
                        }
                        resize.fill = fill;
                    }
                    None => tracing::warn!(
                        fill = %name,
                        "resize.fill is one of stretch, hold or scene; keeping the default"
                    ),
                }
            }
            with_pending(lua, |pending| {
                pending.commands.push(Command::Resize(resize));
            })
        })?,
    )?;

    // The pointer's XCursor theme and its size, in logical pixels.
    //
    // An optional table, the same shape as `sol.loading` above and for the
    // same reason: a setting added later does not change the call. What is
    // *different* is what an absent key means. `sol.loading` leaves the
    // compositor's default standing; here an absent key -- or no table at
    // all, which is the same answer -- means "the configuration did not say", and
    // the environment gets its turn -- `XCURSOR_THEME` and `XCURSOR_SIZE` are
    // what every other application on this machine follows, so a compositor
    // that overwrote them with defaults of its own would be the one thing on
    // screen drawing a different pointer. The order is resolved in
    // `cursor::theme::Settings::resolve`, which is where it is written down.
    //
    // Takes effect immediately, so `super+shift+r` is how a theme is tried.
    //
    // **`cursor_theme` and not `cursor`, and that name is a bug fix rather
    // than a preference.** `sol.cursor` was already taken: it is the getter
    // above that returns where the pointer *is*, and `tiling.lua` calls it on
    // every window open to decide which pane the new window splits. Registering
    // a second function under the same key silently replaced it, so every
    // `sol.on("open")` failed with "error converting Lua nil to table" and no
    // window opened in a tiled layout was ever placed. Nothing caught it:
    // `sol.set` overwrites without complaint, the compositor starts, `--check`
    // passes, and the unit tests for each of the two functions pass
    // individually because each calls its own. Only running it showed it. See
    // `two_functions_cannot_share_one_name` below, which is now the guard.
    sol.set(
        "cursor_theme",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            // **`Option<Table>`, like `sol.keyboard` and `sol.monitors`, and
            // for the same reason those two have it.** The documented way to
            // override the configuration is a single `~/.config/solium/
            // config.lua`, and a copy written before this setting existed has
            // no `cursor` key at all -- so shipped `lua/init.lua` calls
            // `sol.cursor_theme(nil)` and a `Table` parameter fails the *whole
            // configuration*, not just the pointer. No layouts, no bindings,
            // no decorations, over a setting nobody asked for. That is the
            // same class of break as the `sol.cursor` name collision above,
            // which silently stopped every window being placed.
            //
            // Absent is not "reset it": it means the configuration did not
            // say, which is the default `Configured` below, which is what lets
            // `XCURSOR_THEME` and `XCURSOR_SIZE` have their turn.
            let options = options.unwrap_or(lua.create_table()?);
            let mut configured = crate::cursor::theme::Configured::default();
            // `theme` read as a `Value` and matched rather than as an
            // `Option<String>`: mlua *errors* on anything that is not
            // string-like instead of answering None, and the `?` would take
            // the whole handler down with it -- which is how bezier easings
            // once silently stopped working. Same reasoning as `scene` above.
            if let Ok(Value::String(name)) = options.get::<Value>("theme")
                && let Ok(name) = name.to_str()
                && !name.is_empty()
            {
                configured.theme = Some(name.to_string());
            }
            // And `size` the same way, for the same reason. `size = "big"` and
            // `size = 24.5` are both mlua *conversion errors* rather than a
            // None, and an `Option<i32>` with a `?` on it would take the whole
            // configuration down over a typo in one field -- exactly what the
            // line above is written the way it is to avoid.
            //
            // A size out of *range* is a different thing and is not refused
            // here. `Settings::resolve` is the one place that decides what a
            // size that is not a size means, and it needs to see it to fall
            // through to the environment rather than to a clamp -- a second
            // opinion in this line would agree with it today and be free to
            // drift. That is only true of numbers, which is what is passed on;
            // a value that is not a number at all is not a size `resolve`
            // could be shown.
            match options.get::<Value>("size") {
                Ok(Value::Nil) | Err(_) => {}
                Ok(value) => match value.as_i32() {
                    Some(size) => configured.size = Some(size),
                    None => tracing::warn!(
                        size = ?value,
                        "cursor size is not a whole number; ignoring it"
                    ),
                },
            }
            with_pending(lua, |pending| {
                pending.commands.push(Command::Cursor(configured.clone()));
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

    // Name a selection: windows, surfaces and whole monitors, under one name.
    //
    //     sol.group("desk-2", {
    //         windows  = { 3, 7 },
    //         surfaces = { "wallpaper-2" },
    //         monitor  = "DP-1",
    //     })
    //     sol.present_group("desk-2", { x = -2560 }, { duration = 300 })
    //
    // **A transform names a selection, and selections compose.** The wallpaper
    // travels because it is in the selection, not because the compositor knows
    // what a wallpaper is -- it does not, and `sol.surface` stays one primitive
    // doing five jobs. A member's own `sol.present` composes with the group's
    // rather than being replaced by it, so a window tilted inside a moving desk
    // stays tilted within it.
    //
    // `sol.group(name, false)` takes one away. Re-declaring the same name
    // replaces the membership and keeps the transform, so a mode that rebuilds
    // its groups every time it runs -- which is every mode -- does not restart
    // its own animation.
    sol.set(
        "group",
        lua.create_function(|lua, (name, options): (String, Value)| {
            let selection = match options {
                Value::Table(options) => Some(selection_from(&options)?),
                // `false` and `nil` both remove it, the same way `sol.surface`
                // reads them, so one spelling works for both primitives.
                _ => None,
            };
            with_pending(lua, |pending| {
                let animation = pending.animation;
                pending.commands.push(Command::Group {
                    name: name.clone(),
                    selection: selection.clone(),
                    animation,
                });
            })
        })?,
    )?;

    sol.set(
        "present_group",
        lua.create_function(
            |lua, (name, options, motion): (String, Option<Table>, Option<Table>)| {
                let to = match options.as_ref() {
                    Some(options) => shift_from(options)?,
                    None => crate::group::Shift::NONE,
                };
                let (duration, easing) = motion_from(motion.as_ref())?;
                with_pending(lua, |pending| {
                    let animation = pending.animation.with(duration, easing);
                    pending.commands.push(Command::PresentGroup {
                        name: name.clone(),
                        to,
                        animation,
                    });
                })
            },
        )?,
    )?;

    sol.set(
        "present_group_clear",
        lua.create_function(|lua, (name, motion): (String, Option<Table>)| {
            let (duration, easing) = motion_from(motion.as_ref())?;
            with_pending(lua, |pending| {
                let animation = pending.animation.with(duration, easing);
                pending.commands.push(Command::ClearGroup {
                    name: name.clone(),
                    animation,
                });
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

    // The later call wins, and always has: two scripts binding one combination
    // is how `init.lua` and `scrolling.lua` coexist, and the alternative --
    // refusing the second -- would make the order files happen to be required
    // in decide which of them works.
    //
    // The third argument is optional and is a *note*, not a name: a short
    // phrase saying where this binding came from, which `solium --check` prints
    // beside the combination. It exists because that "later call wins" rule is
    // the whole mechanism behind `config.bindings` replacing a shipped binding
    // (#117), and a replacement nobody can see is indistinguishable from a key
    // that mysteriously stopped working. Shipped bindings pass nothing, so
    // `--check` stays a plain list until a configuration has something to say.
    sol.set(
        "bind",
        lua.create_function(
            |lua, (combo, handler, note): (String, mlua::Function, Option<String>)| {
                let sol: Table = lua.globals().get("sol")?;
                let bindings: Table = sol.get("_bindings")?;
                let combo = normalise_combo(&combo);
                bindings.set(combo.clone(), handler)?;
                if let Some(note) = note {
                    let sources: Table = sol.get("_binding_sources")?;
                    sources.set(combo, note)?;
                }
                Ok(())
            },
        )?,
    )?;

    // Whether a combination is already bound, in its canonical spelling.
    //
    // Asking is the only way a script can tell that it is about to replace
    // somebody's binding rather than add one, and normalising is why it cannot
    // do this by reading `_bindings` itself: `Super+Q` and `shift+super+q` are
    // spellings, not keys, and only this side knows which spelling won.
    sol.set(
        "bound",
        lua.create_function(|lua, combo: String| {
            let sol: Table = lua.globals().get("sol")?;
            let bindings: Table = sol.get("_bindings")?;
            Ok(matches!(
                bindings.get::<Value>(normalise_combo(&combo))?,
                Value::Function(_)
            ))
        })?,
    )?;

    // Take a binding away.
    //
    // The counterpart to a configuration being able to replace one. Without it
    // a shipped binding is unremovable short of replacing `init.lua` -- and
    // binding it to a function that does nothing is not the same thing, because
    // the compositor would still swallow the key and `--check` would still list
    // it as bound.
    //
    // The note is kept even though the binding is gone, and that is the point
    // of keeping it: `--check` reads the combinations in `_binding_sources`
    // that are no longer in `_bindings` and reports them as deliberately
    // removed, so an absence is stated rather than merely being an absence.
    sol.set(
        "unbind",
        lua.create_function(|lua, (combo, note): (String, Option<String>)| {
            let sol: Table = lua.globals().get("sol")?;
            let bindings: Table = sol.get("_bindings")?;
            let combo = normalise_combo(&combo);
            bindings.set(combo.clone(), Value::Nil)?;
            if let Some(note) = note {
                let sources: Table = sol.get("_binding_sources")?;
                sources.set(combo, note)?;
            }
            Ok(())
        })?,
    )?;

    // A setting a configuration wrote that the defaults do not define.
    //
    // `lua/config.lua` merges a `user.lua` over its own table key by key, and
    // until #117 a key the defaults had never heard of was merged in exactly
    // like one they had: `tilling = { split = 0.6 }` became a new section that
    // nothing would ever read, and `--check` -- the one command whose job is
    // answering "did what I wrote take effect" -- said the configuration loaded
    // fine. It did. It just did not do anything.
    //
    // Reported from Lua rather than found from here because the comparison
    // needs both halves, and only `config.lua` has them: the defaults are its
    // table, and the user's file is something it alone reads. `meant` is a
    // near-miss suggestion when there is one worth making, and is optional
    // because most typos are not near misses of anything.
    sol.set(
        "unknown",
        lua.create_function(|lua, (key, meant): (String, Option<String>)| {
            let sol: Table = lua.globals().get("sol")?;
            let unknown: Table = sol.get("_unknown_settings")?;
            let entry = lua.create_table()?;
            entry.set("key", key.clone())?;
            if let Some(meant) = &meant {
                entry.set("meant", meant.clone())?;
            }
            unknown.push(entry)?;
            // Also in the log, because most of these are met in a running
            // session rather than in front of `--check`, and a reload that
            // quietly ignores half a file is the failure this is here to end.
            match meant {
                Some(meant) => tracing::warn!(
                    target: "solium::script",
                    setting = %key,
                    "the configuration sets something nothing reads; did you mean {meant}?"
                ),
                None => tracing::warn!(
                    target: "solium::script",
                    setting = %key,
                    "the configuration sets something nothing reads"
                ),
            }
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

/// A Lua table as a JSON object, for a QML scene's properties.
///
/// Shallow on purpose. QML properties are scalars, lists and objects, and a
/// deep converter would need to answer what a Lua table with both array and
/// map keys means -- which is a question with no good answer and no caller
/// asking it. One level of nesting covers every scene there is.
fn json_object(table: &Table) -> mlua::Result<String> {
    let mut out = String::from("{");
    let mut first = true;
    for pair in table.pairs::<String, Value>() {
        let (key, value) = pair?;
        let Some(rendered) = json_value(&value)? else {
            continue;
        };
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&crate::scripted::json_string(&key));
        out.push(':');
        out.push_str(&rendered);
    }
    out.push('}');
    Ok(out)
}

/// One value, or `None` for something JSON has no word for.
fn json_value(value: &Value) -> mlua::Result<Option<String>> {
    Ok(match value {
        Value::String(text) => Some(crate::scripted::json_string(&text.to_str()?)),
        Value::Integer(number) => Some(number.to_string()),
        // Infinities and NaN are not JSON, and a scene handed `Infinity` fails
        // to parse the whole bag rather than that one property.
        Value::Number(number) if number.is_finite() => Some(number.to_string()),
        Value::Boolean(yes) => Some(yes.to_string()),
        Value::Table(table) => {
            let nested = table
                .clone()
                .sequence_values::<Value>()
                .filter_map(|item| item.ok().and_then(|item| json_value(&item).ok().flatten()))
                .collect::<Vec<_>>();
            Some(if nested.is_empty() {
                json_object(table)?
            } else {
                format!("[{}]", nested.join(","))
            })
        }
        _ => None,
    })
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

        // The keyboard path: `axis` is "width" or "height" and `by` is a signed
        // fraction that always *grows* the window when positive, from whichever
        // of the two seams beside it exists. An axis and not an edge because a
        // keypress names no side — `super+equal` means "wider" and nothing
        // about which neighbour pays for it. See `Tiling::resize`.
        //
        // Existence and not room, which this said before and `Tiling::resize`'s
        // own doc has always had right. The trailing seam is preferred and the
        // leading one is a fallback only when there is no trailing seam at all
        // — the window is against its container on that side. A trailing seam
        // that exists but is already at the 0.95 clamp does nothing, and does
        // not hand the press to the seam on the other side. That is the
        // behaviour; whether it is the behaviour a user expects is a separate
        // question from whether the comment describes it.
        methods.add_method_mut("resize", |_, this, (id, axis, by): (u64, String, f64)| {
            this.0.resize(id, axis_named(&axis), by);
            Ok(())
        });

        // The drag path: `edge` is the side the pointer has hold of, which is
        // what the `resize` event hands the script. Not an axis — see
        // `Tiling::drag_seam` for why an axis picks the wrong seam for two of
        // the four sides.
        //
        // `edge_x`/`edge_y` are where that side should come to rest, not where
        // the pointer is, and the two stopped being the same thing in #124.
        // Named for what they are so a script passing the pointer here reads as
        // the mistake it is; `drag_seam` reads only the one `edge` names.
        methods.add_method_mut(
            "drag_seam",
            |_, this, (id, edge, edge_x, edge_y, options): (u64, String, f64, f64, Table)| {
                let Some(edge) = edge_named(&edge) else {
                    // Named rather than guessed at. Falling back to a side
                    // would move a seam the user did not grab, which is the
                    // failure #120 was, and a script with a typo would see it
                    // as the compositor being wrong.
                    tracing::warn!(
                        edge,
                        "not a side a window has; expected left, right, top or bottom"
                    );
                    return Ok(());
                };
                this.0.drag_seam(
                    id,
                    edge,
                    (edge_x, edge_y),
                    area(&options)?,
                    tuning(&options)?,
                );
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

/// How deep a script asked for a window to be drawn. See
/// [`crate::present::Frame::z`].
///
/// Nothing said — no table at all, or a table that does not mention it — is
/// `0.0`, which ties with every other window and so leaves the order the stack
/// gave them exactly as it was. That is what makes the default free.
///
/// **A NaN is dropped and reported, and this guard is load-bearing.**
/// `render::by_depth` compares with `partial_cmp(..).unwrap_or(Equal)`, so a
/// NaN ties with `1.0` and with `2.0` while those two do not tie with each
/// other — a comparator that is not a total order. `slice::sort_by`'s contract
/// for one of those is not merely an unspecified arrangement: **it may panic**,
/// and the call site is the render walk, where a panic is the session going
/// black rather than a stack in the wrong order. One division in a script
/// reaches it. So the window keeps its place rather than the script keeping its
/// typo — the answer `deform_from` gives an effect this build does not have,
/// and not a cosmetic tidy-up to be relaxed later.
///
/// **An infinity is kept, either sign.** It orders against every finite depth
/// and never reaches any arithmetic — `z` is read in exactly one place, as the
/// sort key — so `z = math.huge` means "above everything", `-math.huge` means
/// "behind everything", and both cost nothing to honour.
fn depth_from(options: Option<&Table>) -> mlua::Result<f32> {
    /// Ties with every other window, so the stack's own order survives.
    const LEVEL: f32 = 0.0;

    let Some(options) = options else {
        return Ok(LEVEL);
    };
    let Some(z) = options.get::<Option<f32>>("z")? else {
        return Ok(LEVEL);
    };
    if z.is_nan() {
        tracing::warn!(
            "a NaN `z` cannot be ordered against anything; drawing at the default depth"
        );
        return Ok(LEVEL);
    }
    Ok(z)
}

/// What a script asked a window's matrix to turn about, as a fraction of the
/// rect it is drawn at. See [`crate::present::Frame::pivot`].
///
/// **Each axis defaults on its own.** `pivot_x = 0` means the left edge and
/// says nothing about the vertical; defaulting the pair together would take a
/// script that named one axis and hinge its window about a corner it never
/// mentioned.
///
/// **Outside `0..1` is kept, deliberately.** `pivot_x = 2` turns the window
/// about a line off to its right, which is a hinge and not a mistake — a door
/// swinging on a frame beside it — and `warp.rs` is exactly as defined there
/// as it is at the centre. Clamping would quietly turn one deliberate effect
/// into a different one.
///
/// **Non-finite is not kept.** `warp.rs` computes `loc + size * pivot` for the
/// point the matrix turns about, so a NaN or an infinity makes that point
/// non-finite and every vertex of the mesh with it. Nothing downstream
/// declines to draw the result: `Mat4::project_with_w` guards with
/// `out_w <= 1e-6`, and every comparison against a NaN is false. A window
/// would vanish, with a damage rectangle to match, because a script divided by
/// zero. That axis falls back to the centre and says so.
fn pivot_from(options: Option<&Table>) -> mlua::Result<(f32, f32)> {
    /// The middle of the window, which is what `warp.rs` computed before there
    /// was a pivot to name.
    const CENTRE: f32 = 0.5;

    let Some(options) = options else {
        return Ok((CENTRE, CENTRE));
    };
    // By key, so the two axes cannot be read into each other: there is one
    // body and it is given the name of the axis it is answering for.
    let axis = |key: &str| -> mlua::Result<f32> {
        let Some(fraction) = options.get::<Option<f32>>(key)? else {
            return Ok(CENTRE);
        };
        if !fraction.is_finite() {
            tracing::warn!(
                key,
                "a pivot that is not a finite fraction would put every vertex of the window at NaN; turning about the centre on that axis"
            );
            return Ok(CENTRE);
        }
        Ok(fraction)
    };
    Ok((axis("pivot_x")?, axis("pivot_y")?))
}

/// A Lua table, read as an effect's parameters.
///
/// The bridge between `sol.present` and `crates/effects`, which has no
/// dependencies and so cannot be handed an `mlua::Table`. Each effect asks for
/// the parameters it has, by name, and defaults the rest -- which is what
/// makes adding one a file in that crate and nothing here.
///
/// A read that errors is reported as absent. `mlua` coerces freely, so the
/// only way to get an error out of these is a value of a kind that cannot
/// become a number or a string at all -- a table where a spread should be --
/// and for that the effect's own default is a better answer than refusing the
/// whole call.
struct Given<'a>(&'a Table);

impl solium_effects::Params for Given<'_> {
    fn number(&self, key: &str) -> Option<f64> {
        self.0.get::<Option<f64>>(key).ok().flatten()
    }

    fn word(&self, key: &str) -> Option<String> {
        self.0.get::<Option<String>>(key).ok().flatten()
    }
}

/// Read a deformation out of a `sol.present` options table.
///
/// ```lua
/// deform = { effect = "genie", axis = "down", spread = 1.4,
///            to = { x = 600, y = 1040, w = 120, h = 24 } }
/// ```
///
/// The effect is *named* rather than described, and the name is looked up in
/// the engine rather than matched here -- so an effect added to
/// `crates/effects` is available to every script the moment it compiles,
/// exactly as a curve added to `crates/animation` is. Its parameters come out
/// of the same table and are that effect's business, not this function's.
///
/// `to` is the **anchor**: what the window is being pulled into, or drawn out
/// of. See `present::Anchor` for why naming a thing rather than a rectangle is
/// the point of the key.
fn deform_from(options: &Table) -> mlua::Result<Option<Deform>> {
    let Some(deform) = options.get::<Option<Table>>("deform")? else {
        return Ok(None);
    };
    let Some(name) = deform.get::<Option<String>>("effect")? else {
        return Err(mlua::Error::runtime(
            "a deform needs an `effect` name, such as { effect = \"genie\" }",
        ));
    };
    // Warned about and dropped rather than refused, the way an unknown easing
    // is: a mode naming an effect this build does not have should lose the
    // effect and not the window. `script::shipped` is what stops one shipping.
    let Some(effect) = solium_effects::Deform::from_name(&name, &Given(&deform)) else {
        tracing::warn!(
            effect = name,
            known = ?solium_effects::Deform::all().map(|(known, _)| known),
            "unknown effect, drawing the window undeformed"
        );
        return Ok(None);
    };
    let Some(to) = deform.get::<Option<Table>>("to")? else {
        return Err(mlua::Error::runtime(
            "a deform needs a `to` to aim at: { window = id }, { surface = name } or a rect",
        ));
    };
    Ok(Some(Deform {
        effect,
        aim: aim_from(&to)?,
    }))
}

/// Read a deform's anchor: a thing to follow, or a place to aim at.
///
/// The two identities are the ones that matter -- they are resolved on every
/// frame that draws, so the effect tracks a dock icon or another window as it
/// moves. A rect aims at somewhere that does not move, such as the bottom edge
/// of a monitor, and is honest about being a snapshot because there is nothing
/// there to track.
fn aim_from(to: &Table) -> mlua::Result<Aim> {
    if let Some(id) = to.get::<Option<u64>>("window")? {
        return Ok(Aim::Window(id));
    }
    if let Some(name) = to.get::<Option<String>>("surface")? {
        return Ok(Aim::Surface(name));
    }
    let Some(rect) = rect_from(to)? else {
        return Err(mlua::Error::runtime(
            "a deform's `to` needs { window = id }, { surface = name } or a rect (x, y, w, h)",
        ));
    };
    Ok(Aim::Rect(rect))
}

/// Read a selection out of a `sol.group` table.
///
/// ```lua
/// sol.group("desk-2", {
///     windows  = { 3, 7 },
///     surfaces = { "wallpaper-2" },
///     monitors = { "DP-1" },      -- everything drawn there
///     monitor  = "DP-1",          -- which instance of each surface above
/// })
/// ```
///
/// Every key is optional and an absent one is an empty list, so a selection of
/// nothing is spellable and does nothing — which is what a mode building one
/// desk per monitor per workspace produces for the cells that are empty.
fn selection_from(options: &Table) -> mlua::Result<Selection> {
    let names = |key: &str| -> mlua::Result<Vec<String>> {
        match options.get::<Option<Table>>(key)? {
            Some(list) => list.sequence_values::<String>().collect(),
            None => Ok(Vec::new()),
        }
    };
    let windows = match options.get::<Option<Table>>("windows")? {
        Some(list) => list.sequence_values::<u64>().collect::<mlua::Result<_>>()?,
        None => Vec::new(),
    };
    Ok(Selection {
        windows,
        surfaces: names("surfaces")?,
        monitors: names("monitors")?,
        on: options.get::<Option<String>>("monitor")?,
    })
}

/// Read what a selection is carried by out of a `sol.present_group` table.
///
/// ```lua
/// sol.present_group("desk-2", { x = -2560, opacity = 0.4, rotate_y = 8 })
/// ```
///
/// `x` and `y` are a **displacement** and not a destination, which is the one
/// way this reads differently from `sol.present`: a selection has no rectangle
/// of its own to be moved to. `rotate_*` and `perspective` are read by the same
/// `transform_from` a window's own matrix comes from, so the two spell a
/// rotation identically.
///
/// **`z`, `pivot_x` and `pivot_y` are deliberately not read here**, and it is
/// not an oversight to be tidied up by copying the two lines from
/// `sol.present`. They are the two fields of a frame a selection cannot carry:
///
/// * A **pivot** would have to replace each member's own, because a member is
///   drawn through one matrix turning about one point, and `Shift::apply`
///   composes the group's matrix onto the member's. That contradicts the
///   promise this whole primitive is built on — "a window tilted inside a
///   moving desk stays tilted *within* it" — and it still would not be the
///   thing a script asking for it wants, which is the desk turning as one
///   about a point. That needs a *rectangle for the group*, which no selection
///   has; `group.rs`'s `Shift::matrix` already names it as the honest limit of
///   this stage.
/// * A **depth** would reach only some of a selection. `z` orders the pane
///   walk in `render.rs` and nothing else: scripted surfaces are drawn in
///   fixed layer passes, in declaration order, carried and faded but never
///   sorted. So a desk raised by `z` would lift its windows above the desk
///   next door and leave its own wallpaper behind — the one failure a
///   selection exists to make impossible.
///
/// Both are a *node's* answer, which is where the spec puts them, and
/// `sol.present` is how a script gives one. An unknown key in this table is
/// ignored like any other, so a script that writes one gets no window in the
/// wrong place — it gets nothing, which is the mild half of this note.
fn shift_from(options: &Table) -> mlua::Result<crate::group::Shift> {
    Ok(crate::group::Shift {
        dx: options.get::<Option<f64>>("x")?.unwrap_or(0.0),
        dy: options.get::<Option<f64>>("y")?.unwrap_or(0.0),
        opacity: options.get::<Option<f32>>("opacity")?.unwrap_or(1.0),
        matrix: transform_from(options)?.unwrap_or(Mat4::IDENTITY),
    })
}

/// What a call said about its own timing, before it is laid over the ambient
/// one `sol.animate` set.
///
/// Read outside the pending buffer and applied inside it, because reading a Lua
/// table can fail and the buffer is held by a closure that cannot. Two options
/// rather than an `AnimationSpec`, so "said nothing about the easing" and
/// "asked for the default easing" stay different answers.
///
/// `sol.present_group` takes a table of its own because a mode carrying two
/// selections at different speeds in one dispatch cannot say so with an ambient
/// setting. Absent, it is the ambient setting, so the two calls feel the same as
/// every other pair in this file.
fn motion_from(options: Option<&Table>) -> mlua::Result<(Option<Duration>, Option<Curve>)> {
    let Some(options) = options else {
        return Ok((None, None));
    };
    Ok((
        options
            .get::<Option<u64>>("duration")?
            .map(Duration::from_millis),
        easing_from(options)?,
    ))
}

impl AnimationSpec {
    /// This, with whatever a call actually named.
    const fn with(self, duration: Option<Duration>, easing: Option<Curve>) -> Self {
        Self {
            duration: match duration {
                Some(duration) => duration,
                None => self.duration,
            },
            easing: match easing {
                Some(easing) => easing,
                None => self.easing,
            },
        }
    }
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
            keyboard: crate::keymap::State::initial(),
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
                modal: false,
                parent: Parentage::None,
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

    /// **`sol.decoration` is still `sol.pane`.**
    ///
    /// The setting was renamed when a style stopped being one QML file and
    /// became a folder. `sol.decoration` stays because an `init.lua` someone
    /// wrote before that calls it by name, and Lua gives no warning for a call
    /// to a nil field -- it raises, the script that raised is abandoned, and a
    /// configuration that used to work comes up with no bindings and no
    /// layouts. One line to keep, and this is what says the line is there.
    ///
    /// Both are driven, and the *same* command has to come back from each: an
    /// alias bound to some other function would pass a test that only checked
    /// `sol.decoration` was callable.
    #[test]
    fn the_old_name_for_sol_pane_still_works() {
        let directory = std::env::temp_dir().join("solium-script-test-pane-alias");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("Super+P", function() sol.pane("border") end)
            sol.bind("Super+D", function() sol.decoration("border") end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let named = |scripts: &mut Scripts, combo: &str| {
            let outcome = scripts.key(combo, empty_snapshot());
            assert!(outcome.handled, "{combo} was not handled");
            match outcome.commands.as_slice() {
                [Command::Decoration { name }] => name.clone(),
                other => panic!("expected one style command from {combo}, got {other:?}"),
            }
        };

        assert_eq!(named(&mut scripts, "super+p").as_deref(), Some("border"));
        assert_eq!(
            named(&mut scripts, "super+d").as_deref(),
            Some("border"),
            "the old name reaches the same command as the new one"
        );
    }

    /// **A `user.lua` written before the rename still chooses a style.**
    ///
    /// The migration path the `sol.decoration` alias does *not* cover, and the
    /// one that would have broken quietly. `config.lua` merges the user's table
    /// over its defaults key by key, so a file setting the old `decoration`
    /// leaves `pane` at `"top"` -- their choice read, merged, and thrown away,
    /// with the shipped default on screen and nothing anywhere saying why.
    ///
    /// It runs the **shipped** `config.lua`, not a copy: the temporary
    /// directory holds only `init.lua` and `user.lua`, and `Scripts::load` puts
    /// that directory ahead of the shipped one on `package.path` -- so
    /// `require("user")` finds the fixture and `require("config")` finds the
    /// real thing. That is what makes this a test of the file under review
    /// rather than of a restatement of it.
    ///
    /// Both halves, because they are two edits with one purpose:
    ///
    /// * `config.pane` is the old key's value, which is the read-across;
    /// * `config.decoration` is `nil`, which is the stale key not surviving the
    ///   merge into the table the compositor reads.
    ///
    /// Guarded on a real `~/.config/solium`, which comes *first* on that path:
    /// a developer with their own `user.lua` would have it answer instead of
    /// the fixture, and the assertion would then be about their configuration.
    #[test]
    fn a_user_file_using_the_old_key_still_chooses_a_style() {
        let Some(own) = Scripts::user_config_dir() else {
            return;
        };
        if own.join("user.lua").exists() || own.join("config.lua").exists() {
            // This machine's own configuration would decide it, not the fixture.
            return;
        }

        let directory = std::env::temp_dir().join("solium-script-test-old-key");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        std::fs::write(
            directory.join("user.lua"),
            "return { decoration = \"border\" }\n",
        )
        .expect("writing the fixture user.lua");
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            local config = require("config")
            sol.bind("Super+P", function()
                sol.pane(config.pane)
                sol.status(tostring(config.decoration))
            end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let outcome = scripts.key("super+p", empty_snapshot());
        assert!(outcome.handled);

        match outcome.commands.as_slice() {
            [Command::Decoration { name }] => assert_eq!(
                name.as_deref(),
                Some("border"),
                "a user.lua setting the old `decoration` key still names the style"
            ),
            other => panic!("expected one style command, got {other:?}"),
        }
        assert_eq!(
            outcome.status.as_deref(),
            Some("nil"),
            "and the old key does not survive into the configuration table"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// A fixture `user.lua` in a directory of its own, and the *shipped*
    /// `init.lua` loaded against it.
    ///
    /// The shipped entry point rather than a stand-in, because the thing under
    /// test in every caller below is whether the file people actually run
    /// honours the configuration -- a copy that requires the right modules in
    /// the right order would pass while the real one did not, which is the
    /// class of bug being fixed.
    ///
    /// `None` when this machine has any Lua of its own under
    /// `~/.config/solium`: that directory comes *first* on `package.path`, so
    /// one file there answers `require` ahead of the fixture or the shipped
    /// script it shadows, and the assertions below would then be about
    /// somebody's own configuration. Any `.lua` at all rather than a list of
    /// names, because the list would have to grow every time a script is added
    /// and would fail open when somebody forgot. The same guard, for the same
    /// reason, as `a_user_file_using_the_old_key_still_chooses_a_style`.
    fn shipped_init_with_user(name: &str, user: &str) -> Option<(std::path::PathBuf, Scripts)> {
        let own = Scripts::user_config_dir()?;
        if let Ok(entries) = std::fs::read_dir(&own)
            && entries
                .filter_map(Result::ok)
                .any(|entry| entry.path().extension().is_some_and(|kind| kind == "lua"))
        {
            return None;
        }

        let directory = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).ok()?;
        std::fs::write(directory.join("user.lua"), user).ok()?;
        // Copied rather than required in place: `Scripts::load` puts the
        // config's *own* directory on `package.path` ahead of the shipped one,
        // and that is how the fixture `user.lua` beside it gets found at all.
        let shipped = crate::assets::lua().join("init.lua");
        let config = directory.join("init.lua");
        std::fs::copy(&shipped, &config).ok()?;

        let scripts = Scripts::load(&config).expect("loading the shipped init.lua");
        Some((directory, scripts))
    }

    /// Every spawn a key produced, as `(program, args)`.
    fn spawns(outcome: &Outcome) -> Vec<(String, Vec<String>)> {
        outcome
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::Spawn { program, args } => Some((program.clone(), args.clone())),
                _ => None,
            })
            .collect()
    }

    /// **A `user.lua` can add, replace and remove a binding.**
    ///
    /// The fault this is pinned against: bindings were `sol.bind` calls in
    /// `init.lua` and `user.lua` was a settings table, so the two could not
    /// meet. Binding `super+b` to a browser meant either editing a file the
    /// next update overwrites or writing your own `init.lua` -- two hundred
    /// lines of layout wiring, mode setup and terminal detection adopted in
    /// order to add one key (#117).
    ///
    /// Three assertions, because `config.bindings` makes three promises and
    /// only the first is the obvious one:
    ///
    ///  1. A combination nothing else uses is bound, and runs what it says.
    ///  2. A combination `init.lua` already bound is **replaced** -- the
    ///     decision recorded in `config.lua`'s comment, and the one that makes
    ///     the section worth having: refusing a clash would permit adding a
    ///     binding and forbid changing one, and changing one is what people
    ///     come here for.
    ///  3. `false` removes a shipped binding outright, so the compositor stops
    ///     swallowing the key rather than binding it to a handler that does
    ///     nothing.
    ///
    /// It runs the shipped `init.lua`, which is what makes it a test of the
    /// ordering as well as the mechanism: `require("bindings")` is the last
    /// line of that file precisely so the user's call to `sol.bind` is the
    /// later one, and moving it up would break assertion 2 alone.
    #[test]
    fn a_user_file_can_add_replace_and_remove_a_binding() {
        let Some((directory, mut scripts)) = shipped_init_with_user(
            "solium-script-test-user-bindings",
            r#"
            return {
                bindings = {
                    -- Added: nothing in the shipped configuration binds this.
                    ["super+f5"] = "fixture-browser",
                    -- Replaced: `init.lua` binds super+q to closing a window.
                    ["Super+Q"] = { "fixture-editor", "a file.txt" },
                    -- Removed: `init.lua` binds super+g to the tilt demo.
                    ["super+g"] = false,
                },
            }
            "#,
        ) else {
            return;
        };

        let added = scripts.key("super+f5", empty_snapshot());
        assert!(
            added.handled,
            "a binding from user.lua did not reach sol.bind"
        );
        assert_eq!(
            spawns(&added),
            vec![("fixture-browser".to_owned(), Vec::new())],
            "a string binding is a command line"
        );

        // Written `Super+Q` in the fixture and pressed as `super+q`: the
        // compositor normalises, so a configuration is not obliged to guess the
        // spelling `init.lua` happened to use.
        let replaced = scripts.key("super+q", empty_snapshot());
        assert!(replaced.handled);
        assert_eq!(
            spawns(&replaced),
            vec![("fixture-editor".to_owned(), vec!["a file.txt".to_owned()])],
            "the user's super+q did not win: a list keeps its spaces, and the shipped \
             binding closes a window rather than spawning anything"
        );
        assert!(
            !replaced
                .commands
                .iter()
                .any(|command| matches!(command, Command::Close { .. })),
            "the shipped super+q ran as well as the user's, which means both handlers \
             are registered and only one of them can be reached"
        );

        assert!(
            !scripts.has_binding("super+g"),
            "`false` left the combination bound, so the compositor still swallows the key"
        );

        // And `--check` says so, which is the other half of the decision: a
        // binding may replace a shipped one, but not quietly.
        let reported = scripts.bindings();
        let note = |combo: &str| {
            reported
                .iter()
                .find(|binding| binding.combo == combo)
                .unwrap_or_else(|| panic!("{combo} is not in what --check would print"))
                .note
                .clone()
        };
        assert_eq!(note("super+f5").as_deref(), Some("config.bindings"));
        assert_eq!(
            note("super+q").as_deref(),
            Some("config.bindings, replacing a shipped binding"),
            "--check must be able to tell a replacement from an addition"
        );
        let removed = scripts.unbound();
        assert_eq!(
            removed
                .iter()
                .map(|binding| binding.combo.as_str())
                .collect::<Vec<_>>(),
            vec!["super+g"],
            "a binding removed on purpose looks, in a list of what survived, exactly \
             like one that was never there -- so it is reported separately"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **`open.motion` and `open.scale` decide the open animation.**
    ///
    /// Both were documented in `config.lua` and read by nothing: `open.lua`
    /// held `220 / outBack` and `0.88` as local constants while the
    /// configuration advertised `200 / outCubic` and `0.92`, so editing either
    /// setting did nothing and there was no way to tell that apart from having
    /// misunderstood what it meant (#117).
    ///
    /// The numbers in the fixture are deliberately unlike both pairs, so this
    /// cannot pass by agreeing with whichever set of constants happens to be in
    /// the file.
    #[test]
    fn the_open_animation_is_the_one_the_configuration_names() {
        let Some((directory, mut scripts)) = shipped_init_with_user(
            "solium-script-test-open-animation",
            r#"
            return {
                open = {
                    motion = { duration = 777, easing = "linear" },
                    scale = 0.5,
                },
            }
            "#,
        ) else {
            return;
        };

        // 800x600 at the origin, from `one_screen`. At a scale of 0.5 the
        // window starts 400x300 about its own centre, so at 200,150.
        let outcome = scripts.opened(7, one_screen(&[7]));
        let from = outcome
            .commands
            .iter()
            .find_map(|command| match command {
                Command::PresentFrom {
                    id: 7,
                    rect,
                    animation,
                    ..
                } => Some((*rect, *animation)),
                _ => None,
            })
            .expect("the open animation did not run");

        assert_eq!(
            from.1.duration,
            Duration::from_millis(777),
            "open.motion.duration is not what the window arrives with"
        );
        assert_eq!(
            from.1.easing,
            Curve::Linear,
            "open.motion.easing is not what the window arrives with"
        );
        assert!(
            (from.0.w - 400.0).abs() < 0.5 && (from.0.h - 300.0).abs() < 0.5,
            "open.scale did not decide how small the window starts: {:?}",
            from.0
        );
        assert!(
            (from.0.x - 200.0).abs() < 0.5 && (from.0.y - 150.0).abs() < 0.5,
            "and it is still shrunk about its own centre: {:?}",
            from.0
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **`scrolling.widths` and `scrolling.default_width` decide how wide a
    /// column opens.**
    ///
    /// The other two settings that were documented and read by nothing: the
    /// strip used the constant list in `crates/layout/src/scroller.rs`, so a
    /// configuration asking for quarters got thirds (#117).
    ///
    /// Driven through the shipped `scrolling.lua` rather than by calling
    /// `sol.layout.scroller` from a fixture, because the missing link was the
    /// *call site* -- the constructor took no argument, and no test of the
    /// layout crate could have seen that. 0.8 of a 1600-wide screen is
    /// unmistakably neither of the shipped presets near it.
    #[test]
    fn the_configured_widths_are_what_a_column_opens_at() {
        let Some((directory, mut scripts)) = shipped_init_with_user(
            "solium-script-test-scrolling-widths",
            r#"
            return {
                gap = 0,
                scrolling = { widths = { 0.25, 0.8 }, default_width = 2 },
            }
            "#,
        ) else {
            return;
        };

        scripts.monitors_changed(one_screen(&[]));
        // super+s is the shipped binding that puts the scrolling layout in
        // charge; without it the strip is built but nothing places anything.
        scripts.key("super+s", one_screen(&[]));
        let outcome = scripts.opened(7, one_screen(&[7]));

        let placed = outcome
            .commands
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::Place { id: 7, rect, .. } => Some(*rect),
                _ => None,
            })
            .expect("the scrolling layout placed nothing");
        assert!(
            (placed.w - 1280.0).abs() < 1.0,
            "a new column did not open at `default_width` of `widths` -- 0.8 of a \
             1600-wide view is 1280, and the shipped presets would give 533, 800 or \
             1066: {placed:?}"
        );

        // And the cycle is over the configured list, not the shipped one: one
        // step from the second of two wraps to the first.
        let cycled = scripts.key("super+r", one_screen(&[7]));
        let after = cycled
            .commands
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::Place { id: 7, rect, .. } => Some(*rect),
                _ => None,
            })
            .expect("super+r placed nothing");
        assert!(
            (after.w - 400.0).abs() < 1.0,
            "super+r did not cycle through the configured widths: 0.25 of 1600 is 400, \
             and a lap of a two-entry list is two steps: {after:?}"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A key the defaults do not define is reported, not swallowed.**
    ///
    /// `config.lua`'s `merge` validated nothing, so `tilling = { ... }` was
    /// merged in as a new section, read by nothing, for ever -- and `solium
    /// --check`, the one command whose job is answering "did my configuration
    /// work", said it had loaded fine (#117). It had. That was never the
    /// question.
    ///
    /// Four fixtures, and the last two are the ones that make this worth
    /// having: a check that reports real typos and also reports working
    /// settings is a check nobody will keep running.
    #[test]
    fn an_unrecognised_setting_is_reported_rather_than_merged_in_silence() {
        let Some((directory, scripts)) = shipped_init_with_user(
            "solium-script-test-unknown-settings",
            r#"
            return {
                -- A typo at the top level, and one inside a section.
                tilling = { split = 0.6 },
                open = { scal = 0.5 },
                -- A near miss on the *deprecated* spelling of `pane`. Reported,
                -- because it is not a key anything reads -- and reported with no
                -- suggestion, because the only word within two edits of it is
                -- `decoration`, and answering a typo with a deprecated spelling
                -- walks somebody past the key that actually works (#117 review).
                decoraton = "border",
                -- Not typos, and must not be reported: `keyboard` is empty on
                -- purpose, so its keys cannot be checked against the defaults,
                -- and a binding combination is whatever you press. `active`
                -- belongs with a dual layout and is exactly the pair that used
                -- to be called a typo.
                keyboard = { layout = "us,ua", active = 2 },
                bindings = { ["super+f6"] = "fixture-nothing" },
            }
            "#,
        ) else {
            return;
        };

        let found = scripts.unknown_settings();
        let reported: Vec<(&str, Option<&str>)> = found
            .iter()
            .map(|setting| (setting.key.as_str(), setting.meant.as_deref()))
            .collect();
        assert_eq!(
            reported,
            vec![
                ("decoraton", None),
                ("open.scal", Some("open.scale")),
                ("tilling", Some("tiling"))
            ],
            "the unrecognised keys are wrong: a near miss inside a section must be \
             reported by its full path, a near miss on a deprecated name must not be \
             answered with that name, and neither the `keyboard` section nor a binding \
             combination may be called a typo"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// A configured cursor theme and size reach the compositor as written.
    ///
    /// The wiring between `sol.cursor_theme` and `Command::Cursor`, which is the
    /// one part of the cursor work that no test in `cursor.rs` can reach: that
    /// module's tests start from a `Configured` that they built themselves.
    #[test]
    fn a_configured_cursor_theme_reaches_the_compositor() {
        let directory = std::env::temp_dir().join("solium-script-test-cursor");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("Super+C", function()
                sol.cursor_theme({ theme = "Fixture", size = 40 })
            end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let outcome = scripts.key("super+c", empty_snapshot());
        assert!(outcome.handled);
        match outcome.commands.as_slice() {
            [Command::Cursor(configured)] => {
                assert_eq!(configured.theme.as_deref(), Some("Fixture"));
                assert_eq!(configured.size, Some(40));
            }
            other => panic!("expected one cursor command, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **The shipped `config.lua` says nothing about the cursor, and that is
    /// the setting.**
    ///
    /// The one property of this configuration surface that a plausible-looking
    /// edit would destroy without failing anything else. `cursor = {}` is not
    /// an empty placeholder waiting to be filled in with sensible defaults:
    /// writing `cursor = { size = 24 }` there would make the configuration
    /// *always* have an opinion, `XCURSOR_SIZE` would never be reached, and
    /// the precedence tests in `cursor::theme` would all still pass because
    /// they never read this file.
    ///
    /// Runs the shipped `config.lua` rather than a copy, the same way the
    /// old-key test above does and with the same guard on a real
    /// `~/.config/solium`, which would otherwise answer first.
    #[test]
    fn the_shipped_configuration_leaves_the_cursor_to_the_environment() {
        let Some(own) = Scripts::user_config_dir() else {
            return;
        };
        if own.join("user.lua").exists() || own.join("config.lua").exists() {
            return;
        }

        let directory = std::env::temp_dir().join("solium-script-test-cursor-default");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            local config = require("config")
            sol.bind("Super+C", function()
                sol.cursor_theme(config.cursor)
            end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let outcome = scripts.key("super+c", empty_snapshot());
        assert!(outcome.handled);
        match outcome.commands.as_slice() {
            [Command::Cursor(configured)] => assert_eq!(
                configured,
                &crate::cursor::theme::Configured::default(),
                "the shipped config.lua names a cursor theme or size, so XCURSOR_THEME and \
                 XCURSOR_SIZE can never win"
            ),
            other => panic!("expected one cursor command, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A configuration with no `cursor` key at all still loads.**
    ///
    /// The documented way to change one setting is a single
    /// `~/.config/solium/config.lua`, and shipped `init.lua` calls
    /// `sol.cursor_theme(config.cursor)` against it. A copy written before this
    /// setting existed — which is every copy made before #81 — has no `cursor`
    /// key, so that call is `sol.cursor_theme(nil)`. Taking a `Table` made that
    /// an error at load, and an error at load is not a pointer that falls back:
    /// it is *no configuration*, no layouts, no bindings and no decorations,
    /// over a setting the user never heard of.
    ///
    /// The same class of break as `two_functions_cannot_share_one_name` below,
    /// and the same shape of guard. `sol.keyboard` and `sol.monitors` take an
    /// optional table for exactly this reason.
    ///
    /// Asserted at load *and* in a binding, because those are two different
    /// call sites with two different consequences: the one in `init.lua` runs
    /// while the configuration is being built, and the one under a key runs
    /// after.
    #[test]
    fn a_configuration_that_says_nothing_about_the_cursor_still_loads() {
        let directory = std::env::temp_dir().join("solium-script-test-cursor-nil");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            -- A user's own config.lua, written before the cursor setting
            -- existed: no `cursor` key, so this is sol.cursor_theme(nil).
            local config = { pane = {} }
            sol.cursor_theme(config.cursor)
            sol.bind("Super+C", function()
                sol.cursor_theme(config.cursor)
                sol.status("alive")
            end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config)
            .expect("a configuration with no cursor key failed to load at all");
        let outcome = scripts.key("super+c", empty_snapshot());
        assert!(outcome.handled);
        assert_eq!(outcome.status.as_deref(), Some("alive"));
        match outcome.commands.as_slice() {
            [Command::Cursor(configured)] => assert_eq!(
                configured,
                &crate::cursor::theme::Configured::default(),
                "no table is not 'reset it': it means the configuration did not say, so \
                 XCURSOR_THEME and XCURSOR_SIZE still get their turn"
            ),
            other => panic!("expected one cursor command, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// And a `size` that is not a whole number is ignored rather than fatal.
    ///
    /// `Settings::resolve` is the one place that decides what a size *out of
    /// range* means, and it is handed the number to decide about. A value that
    /// is not a number is a different thing: mlua answers an `Option<i32>` with
    /// a conversion *error* for `"big"` and for `24.5`, and a `?` on it would
    /// take the whole configuration down — the same failure as the missing
    /// table above, reached through a typo in one field. The `theme` key eight
    /// lines above has always read this way; this makes the pair agree.
    #[test]
    fn a_cursor_size_that_is_not_a_number_is_ignored_and_not_fatal() {
        let directory = std::env::temp_dir().join("solium-script-test-cursor-size");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("Super+C", function()
                sol.cursor_theme({ theme = "Fixture", size = "big" })
            end)
            sol.bind("Super+D", function()
                sol.cursor_theme({ theme = "Fixture", size = 24.5 })
            end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        for key in ["super+c", "super+d"] {
            let outcome = scripts.key(key, empty_snapshot());
            assert!(
                outcome.handled,
                "{key}: the listener failed, so the whole configuration went with it"
            );
            match outcome.commands.as_slice() {
                [Command::Cursor(configured)] => {
                    assert_eq!(
                        configured.theme.as_deref(),
                        Some("Fixture"),
                        "{key}: the rest of the table was lost with the bad size"
                    );
                    assert_eq!(
                        configured.size, None,
                        "{key}: a size that is not a whole number reached the compositor"
                    );
                }
                other => panic!("{key}: expected one cursor command, got {other:?}"),
            }
        }

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// `sol.cursor` is still where the pointer is, and setting the theme is a
    /// different name.
    ///
    /// **The regression this exists for shipped, and was invisible to the
    /// gate.** #81 registered the theme setter under `sol.cursor`, which was
    /// already the getter that answers with the pointer's position. `sol.set`
    /// replaces without complaining, so the getter simply stopped existing.
    /// What broke was `tiling.lua`, which asks where the pointer is on every
    /// window open to decide which pane the new window splits: every
    /// `sol.on("open")` failed with "error converting Lua nil to table", and
    /// no window opened in a tiled layout was placed. Nothing caught it — the
    /// compositor starts, `--check` passes, and each function's own test
    /// passes because each calls its own name. It took running the thing.
    ///
    /// So this asserts the pair, in one script, in one dispatch: the position
    /// comes back as the numbers the snapshot was built with, *and* the theme
    /// reaches the compositor as a command. Either name overwriting the other
    /// fails this, whichever way round it is done.
    #[test]
    fn two_functions_cannot_share_one_name() {
        let directory = std::env::temp_dir().join("solium-script-test-cursor-names");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("Super+C", function()
                local at = sol.cursor()
                sol.status(string.format("%d,%d", at.x, at.y))
                sol.cursor_theme({ theme = "Fixture" })
            end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let mut snapshot = empty_snapshot();
        snapshot.cursor = (12.0, 34.0);
        let outcome = scripts.key("super+c", snapshot);
        assert!(
            outcome.handled,
            "the listener failed, which is exactly how the collision showed up"
        );
        assert_eq!(
            outcome.status.as_deref(),
            Some("12,34"),
            "sol.cursor() did not answer with the pointer's position"
        );
        match outcome.commands.as_slice() {
            [Command::Cursor(configured)] => {
                assert_eq!(configured.theme.as_deref(), Some("Fixture"));
            }
            other => panic!("sol.cursor_theme() did not reach the compositor: {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **The three answers about a parent survive the trip into Lua.**
    ///
    /// The whole value of [`Parentage`] is that "no parent" and "a parent I
    /// cannot point at" stay apart, and the only place that can be lost is the
    /// one line that turns it into a Lua value. A script that has to centre a
    /// dialog asks `if window.parent then`, so both of the negative answers
    /// have to be falsey -- and a script that wants to tell them apart asks
    /// `== false`, so they have to be different values. Both halves are checked
    /// here, because getting `Unknown` wrong in the other direction -- `nil` --
    /// costs nothing today and quietly deletes the distinction.
    ///
    /// `modal` rides along: it is set on the same line and read by the same
    /// scripts, and a boolean that arrives as nil reads as false.
    #[test]
    fn a_parent_reaches_a_script_as_a_number_false_or_nothing() {
        let directory = std::env::temp_dir().join("solium-script-test-parentage");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("Super+P", function()
                for _, window in ipairs(sol.windows()) do
                    -- Reported as a place, because `sol.place` is the only way
                    -- out of a dispatch that carries four numbers.
                    sol.place(window.id, {
                        x = window.parent == nil and -1
                            or (window.parent == false and -2 or window.parent),
                        y = window.modal and 1 or 0,
                        w = 1,
                        h = 1,
                    })
                end
            end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let mut snapshot = empty_snapshot();
        let window = |id: u64, modal: bool, parent: Parentage| WindowInfo {
            id,
            rect: Rect::default(),
            drawn: Rect::default(),
            title: String::new(),
            focused: false,
            monitor: "test-1".to_owned(),
            modal,
            parent,
        };
        snapshot.windows = vec![
            window(1, false, Parentage::None),
            window(2, true, Parentage::Unknown),
            window(3, true, Parentage::Window(1)),
        ];

        let outcome = scripts.key("super+p", snapshot);
        assert!(outcome.handled);
        let mut seen = Vec::new();
        for command in &outcome.commands {
            if let Command::Place { id, rect, .. } = command {
                seen.push((*id, rect.x, rect.y));
            }
        }
        assert_eq!(
            seen,
            vec![
                // No parent at all: absent from the table, so `nil`.
                (1, -1.0, 0.0),
                // A parent that was named and cannot be pointed at: `false`,
                // which is falsey for `if window.parent then` and still tells
                // a script that asks that something was named.
                (2, -2.0, 1.0),
                // A parent that can be pointed at: its id, as a number.
                (3, 1.0, 1.0),
            ],
            "the parent of a window did not survive the trip into Lua"
        );
    }

    /// A snapshot with nothing in it, for the tests that only want a call made.
    fn empty_snapshot() -> Snapshot {
        Snapshot {
            keyboard: crate::keymap::State::initial(),
            windows: Vec::new(),
            monitors: Vec::new(),
            work_area: Rect::default(),
            cursor: (0.0, 0.0),
        }
    }

    /// **A script names an effect, and what comes back is the engine's.**
    ///
    /// The round trip nothing else covers: `crates/effects` has thorough unit
    /// tests and knows nothing about Lua, and `script::shipped` reads names out
    /// of files without running them. This is the join -- an effect resolved by
    /// name, its parameters read through `Given`, and both kinds of anchor.
    ///
    /// The anchor is the half worth pinning. `{ window = 9 }` must survive as
    /// an *identity* all the way to the command, because the moment it becomes
    /// a rectangle here it is a rectangle measured when the key was pressed.
    #[test]
    fn a_script_names_an_effect_and_the_engine_answers() {
        let directory = std::env::temp_dir().join("solium-script-test-deform");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("super+1", function()
                sol.present(1, { deform = { effect = "genie", axis = "left",
                                            spread = 2.5,
                                            to = { x = 10, y = 20, w = 30, h = 40 } } })
                sol.present(2, { deform = { effect = "genie", to = { window = 9 } } })
                sol.present(3, { deform = { effect = "nonsense", to = { window = 9 } } })
                sol.present(4, {})
                sol.present(5, { deform = { effect = "genie", to = { surface = "dock" } } })
            end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let outcome = scripts.key("super+1", Snapshot::default());
        let deforms: Vec<Option<Deform>> = outcome
            .commands
            .iter()
            .map(|command| match command {
                Command::Present { deform, .. } => deform.clone(),
                other => panic!("expected a present command, got {other:?}"),
            })
            .collect();
        assert_eq!(deforms.len(), 5);

        assert_eq!(
            deforms[0],
            Some(Deform {
                effect: solium_effects::Deform::Genie {
                    // Not named, so the effect's own default: all the way in,
                    // which is what one animates towards.
                    progress: 1.0,
                    spread: 2.5,
                    axis: solium_effects::Axis::Left,
                },
                aim: Aim::Rect(Rect {
                    x: 10.0,
                    y: 20.0,
                    w: 30.0,
                    h: 40.0
                }),
            })
        );
        assert_eq!(
            deforms[1],
            Some(Deform {
                effect: solium_effects::Deform::Genie {
                    progress: 1.0,
                    spread: 1.0,
                    axis: solium_effects::Axis::Down,
                },
                aim: Aim::Window(9),
            })
        );
        // An effect this build does not have loses the effect, not the window.
        assert_eq!(deforms[2], None);
        assert_eq!(deforms[3], None);
        // **A surface survives this far as a name.** It becomes a
        // `scripted::SurfaceId` in `Solium::aimed`, which is the first place
        // that can see whether there is a surface by that name -- and the id is
        // what keeps `present::Frame` `Copy`.
        assert_eq!(
            deforms[4],
            Some(Deform {
                effect: solium_effects::Deform::Genie {
                    progress: 1.0,
                    spread: 1.0,
                    axis: solium_effects::Axis::Down,
                },
                aim: Aim::Surface("dock".to_owned()),
            })
        );
    }

    /// **A script says how deep a window is drawn and what it turns about.**
    ///
    /// All of that pair's surface in one script: both keys read, a table
    /// mentioning neither producing exactly the frame every window has had
    /// until now, and each pivot axis defaulting on its own -- `pivot_x = 0`
    /// means the left edge and says nothing about the vertical, so a script
    /// naming one axis must not be given two.
    ///
    /// **The two pivot numbers are deliberately different, and neither is a
    /// default.** `(0.5, 0.5)` and `(0.0, 0.0)` are each their own transpose,
    /// so a fixture built from either cannot tell `pivot_x` read into the
    /// wrong half of the pair from the code being right. `(0.25, 1.0)` can,
    /// and the two one-axis cases pin the same swap from the other side:
    /// naming only `pivot_x` must move the *first* number and only that one.
    #[test]
    fn a_script_says_how_deep_a_window_is_and_what_it_turns_about() {
        let directory = std::env::temp_dir().join("solium-script-test-depth");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("super+3", function()
                sol.present(1, { z = 2.5, pivot_x = 0.25, pivot_y = 1.0 })
                -- All four, because `rect_from` refuses half a rect: a table
                -- that mentions neither new key still has to be a table a
                -- script could really write.
                sol.present(2, { x = 10, y = 20, w = 300, h = 200 })
                sol.present(3, { pivot_x = 0.25 })
                sol.present(4, { pivot_y = 1.0 })
                sol.present(5)
            end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let outcome = scripts.key("super+3", Snapshot::default());
        let drawn = presented(&outcome.commands);
        assert_eq!(drawn.len(), 5);

        assert!(
            (drawn[0].0 - 2.5).abs() < f32::EPSILON,
            "the depth the script asked for, got {}",
            drawn[0].0
        );
        assert_eq!(drawn[0].1, (0.25, 1.0), "x into x, y into y");

        // A table mentioning neither is the frame every window on the machine
        // has had until now: depth zero, which ties with every other window
        // and so keeps the order the stack gave them, turning about its own
        // centre.
        assert!((drawn[1].0 - 0.0).abs() < f32::EPSILON);
        assert_eq!(drawn[1].1, (0.5, 0.5));

        // One axis named leaves the other in the middle. Asserted from both
        // sides, because one of them alone is satisfied by a reader that
        // defaults the pair together on whichever axis it was given.
        assert_eq!(drawn[2].1, (0.25, 0.5), "only the horizontal moved");
        assert_eq!(drawn[3].1, (0.5, 1.0), "only the vertical moved");

        // And no options table at all answers the same as a table that says
        // nothing -- `sol.present(id)` clears a window back to its own
        // geometry, and it must not sort or hinge differently for it.
        assert!((drawn[4].0 - 0.0).abs() < f32::EPSILON);
        assert_eq!(drawn[4].1, (0.5, 0.5));

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A number no window can be drawn with is dropped, per key and per
    /// axis.**
    ///
    /// One division reaches NaN from a script, and the two fields answer it
    /// very differently if nothing stops it here.
    ///
    /// A **pivot** is the dangerous one. `warp.rs` computes
    /// `loc + size * pivot` for the point the matrix turns about, so a
    /// non-finite one makes that centre non-finite and every vertex of the
    /// mesh with it -- and nothing further down declines to draw the result,
    /// because `Mat4::project_with_w` guards with `out_w <= 1e-6` and every
    /// comparison against a NaN is false. The window disappears and its damage
    /// rectangle is nonsense, from a typo.
    ///
    /// A **depth** is quieter to look at and no safer. `render::by_depth`
    /// answers `Equal` when `partial_cmp` declines, so a NaN ties with 1.0 and
    /// with 2.0 while those two do not tie with each other -- not a total
    /// order, and `slice::sort_by` handed one of those **may panic**. That is
    /// the render walk, so the failure is the session going black.
    ///
    /// So both fall back to the default and say so in the log -- the answer
    /// `deform_from` gives an effect this build does not have, and
    /// `easing_from` an easing nobody wrote: the script loses the key it
    /// mistyped and keeps its window. An **infinite depth is kept**, in both
    /// directions, because it orders perfectly well: `z = math.huge` is a
    /// legible spelling of "above everything" and `-math.huge` of "behind
    /// everything". Both are asserted, because a guard written
    /// `z.is_nan() || z == f32::NEG_INFINITY` passes every other case here.
    #[test]
    fn a_depth_or_a_pivot_that_cannot_be_drawn_with_falls_back() {
        let directory = std::env::temp_dir().join("solium-script-test-nonfinite");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("super+4", function()
                sol.present(1, { z = 0/0, pivot_x = 0.25 })
                sol.present(2, { pivot_x = 0/0, pivot_y = 1.0 })
                sol.present(3, { pivot_x = 0.25, pivot_y = 1/0 })
                sol.present(4, { pivot_y = -1/0 })
                sol.present(5, { z = 1/0 })
                sol.present(6, { z = -1/0 })
            end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let outcome = scripts.key("super+4", Snapshot::default());
        let drawn = presented(&outcome.commands);
        assert_eq!(drawn.len(), 6);

        // The key that was wrong is the only key that loses anything: the
        // pivot beside the NaN depth is the one the script wrote.
        assert!(
            (drawn[0].0 - 0.0).abs() < f32::EPSILON,
            "a NaN depth is zero"
        );
        assert_eq!(drawn[0].1, (0.25, 0.5));

        // And the axis that was wrong is the only axis. This is the second
        // reading of the transpose: a NaN given as `pivot_x` must come back as
        // a centred *first* number beside the 1.0 that was given as `pivot_y`.
        assert_eq!(drawn[1].1, (0.5, 1.0), "the horizontal fell back, alone");
        assert_eq!(
            drawn[2].1,
            (0.25, 0.5),
            "and an infinity no less than a NaN"
        );
        assert_eq!(drawn[3].1, (0.5, 0.5), "in either direction");

        // A depth is not a coordinate and an infinite one sorts, so it is the
        // one non-finite number here that survives -- **in both directions**.
        // Asserted from each end because a guard that refused only one of them
        // (`z.is_nan() || z == f32::NEG_INFINITY`, the plausible one: NaN and
        // the negative infinity are what a division by zero yields when the
        // numerator went wrong) passes every other case in this test.
        assert!(
            drawn[4].0.is_infinite() && drawn[4].0 > 0.0,
            "an infinite depth orders, so it is kept, got {}",
            drawn[4].0
        );
        assert!(
            drawn[5].0.is_infinite() && drawn[5].0 < 0.0,
            "and so does a negative one, which means behind everything, got {}",
            drawn[5].0
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// The depth and pivot of each `Present` in a batch, in order.
    fn presented(commands: &[Command]) -> Vec<(f32, (f32, f32))> {
        commands
            .iter()
            .map(|command| match command {
                Command::Present { z, pivot, .. } => (*z, *pivot),
                other => panic!("expected a present command, got {other:?}"),
            })
            .collect()
    }

    /// **A script names a selection, and both halves of it come back.**
    ///
    /// The round trip for the other primitive this file gained: who is in a
    /// group, and what carrying it means. The displacement is the part worth
    /// pinning -- `x` on `sol.present_group` is a *delta* where `x` on
    /// `sol.present` is a destination, because a selection has no rectangle of
    /// its own to be moved to, and reading it as a destination would put every
    /// member of every group in the same place.
    #[test]
    fn a_script_names_a_selection_and_says_where_to_carry_it() {
        let directory = std::env::temp_dir().join("solium-script-test-group");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("super+2", function()
                sol.animate({ duration = 300, easing = "inOutQuad" })
                sol.group("desk-2", {
                    windows = { 3, 7 },
                    surfaces = { "wallpaper-2" },
                    monitors = { "DP-1" },
                    monitor = "DP-1",
                })
                sol.present_group("desk-2", { x = -2560, opacity = 0.5 })
                sol.present_group("desk-1", { y = 40 }, { duration = 90 })
                sol.present_group_clear("desk-3")
                sol.group("desk-4", false)
            end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let outcome = scripts.key("super+2", Snapshot::default());
        assert_eq!(outcome.commands.len(), 5);

        match &outcome.commands[0] {
            Command::Group {
                name,
                selection: Some(selection),
                animation,
            } => {
                assert_eq!(name, "desk-2");
                assert_eq!(selection.windows, vec![3, 7]);
                assert_eq!(selection.surfaces, vec!["wallpaper-2".to_owned()]);
                assert_eq!(selection.monitors, vec!["DP-1".to_owned()]);
                assert_eq!(selection.on.as_deref(), Some("DP-1"));
                // A membership change is animated too, and by the ambient
                // setting: that is how long a window takes to get back to where
                // it was looking when it changes desks.
                assert_eq!(animation.duration, Duration::from_millis(300));
            }
            other => panic!("expected a group command, got {other:?}"),
        }

        match &outcome.commands[1] {
            Command::PresentGroup {
                name,
                to,
                animation,
            } => {
                assert_eq!(name, "desk-2");
                assert_eq!(to.offset(), (-2560.0, 0.0));
                assert!((to.opacity - 0.5).abs() < f32::EPSILON);
                assert_eq!(animation.duration, Duration::from_millis(300));
                assert_eq!(animation.easing, Curve::InOutQuad);
            }
            other => panic!("expected a present_group command, got {other:?}"),
        }

        // Its own table overrides the ambient duration and keeps the easing.
        match &outcome.commands[2] {
            Command::PresentGroup { animation, to, .. } => {
                assert_eq!(to.offset(), (0.0, 40.0));
                assert_eq!(animation.duration, Duration::from_millis(90));
                assert_eq!(animation.easing, Curve::InOutQuad);
            }
            other => panic!("expected a present_group command, got {other:?}"),
        }

        assert!(
            matches!(&outcome.commands[3], Command::ClearGroup { name, .. } if name == "desk-3")
        );
        // `false` removes a selection, the same spelling `sol.surface` takes.
        assert!(matches!(
            &outcome.commands[4],
            Command::Group {
                name,
                selection: None,
                ..
            } if name == "desk-4"
        ));
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

    /// Who each named selection holds, after a batch of commands.
    ///
    /// Folded rather than indexed, because `regroup` re-declares every desk on
    /// every event and a test that asserted on `commands[4]` would be pinning
    /// the order of a loop over monitors.
    fn membership(commands: &[Command]) -> std::collections::HashMap<String, Vec<u64>> {
        let mut out = std::collections::HashMap::new();
        for command in commands {
            if let Command::Group {
                name,
                selection: Some(selection),
                ..
            } = command
            {
                out.insert(name.clone(), selection.windows.clone());
            }
        }
        out
    }

    /// Where each named selection was last asked to sit, after a batch.
    fn carried(commands: &[Command]) -> std::collections::HashMap<String, (f64, f64)> {
        let mut out = std::collections::HashMap::new();
        for command in commands {
            match command {
                Command::PresentGroup { name, to, .. } => {
                    out.insert(name.clone(), to.offset());
                }
                Command::ClearGroup { name, .. } => {
                    out.insert(name.clone(), (0.0, 0.0));
                }
                _ => {}
            }
        }
        out
    }

    /// One 1600x900 screen and three windows on it, for the reload test.
    fn one_screen(windows: &[u64]) -> Snapshot {
        Snapshot {
            keyboard: crate::keymap::State::initial(),
            windows: windows
                .iter()
                .map(|id| WindowInfo {
                    id: *id,
                    rect: Rect {
                        x: 0.0,
                        y: 0.0,
                        w: 800.0,
                        h: 600.0,
                    },
                    drawn: Rect::default(),
                    title: String::new(),
                    focused: false,
                    monitor: "test-1".to_owned(),
                    // Ordinary windows: this fixture is about what a reload
                    // keeps, and #72's modality plays no part in it. Spelled
                    // out rather than defaulted so that a third field added to
                    // `WindowInfo` fails here and is decided for this test
                    // rather than silently inherited.
                    modal: false,
                    parent: Parentage::None,
                })
                .collect(),
            monitors: vec![MonitorInfo {
                name: "test-1".to_owned(),
                area: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 1600.0,
                    h: 900.0,
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
                y: 0.0,
                w: 1600.0,
                h: 900.0,
            },
            cursor: (0.0, 0.0),
        }
    }

    /// **A reload does not lose the session, which is the whole of issue #116.**
    ///
    /// The bug as it was reported: `super+shift+r` on workspace 3, and the
    /// desktop is drawn two screen-widths off-stage with no key that brings it
    /// back. Underneath it are three separate losses, and this drives all
    /// three through the same sequence `Solium::reload` runs -- `kept` and
    /// `load_carrying`, then `restore`, `monitors`, `layout`:
    ///
    ///  * which workspace each monitor is showing, so the desks are carried
    ///    relative to the one in view and not to desk 1;
    ///  * which workspace each *window* is on, so three windows on two desks
    ///    are still on two desks rather than swept onto one;
    ///  * which layout is in charge, because a tiled session that comes back
    ///    believing it is floating toggles the wrong way on the next key.
    ///
    /// It runs the **shipped** `workspaces.lua`, `modes.lua` and `tiling.lua`
    /// rather than copies, which is what makes it a test of the files under
    /// review. That also means a developer's own `~/.config/solium` would
    /// answer `require` ahead of them -- it comes first on `package.path` --
    /// so the run is skipped there rather than asserting about somebody's
    /// configuration. The same guard, for the same reason, as
    /// `a_user_file_using_the_old_key_still_chooses_a_style`.
    #[test]
    fn a_reload_keeps_the_workspace_the_windows_and_the_layout() {
        if let Some(own) = Scripts::user_config_dir()
            && (own.join("config.lua").exists()
                || own.join("user.lua").exists()
                || own.join("workspaces.lua").exists()
                || own.join("modes.lua").exists())
        {
            return;
        }

        let directory = std::env::temp_dir().join("solium-script-test-reload");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            local workspaces = require("workspaces")
            local modes = require("modes")
            local tiling = require("tiling")

            sol.bind("super+f1", function()
                modes.use("tiling")
                workspaces.go(3)
            end)

            -- What the session believes about itself, in one string, so the
            -- assertions read the scripts' own answer rather than a
            -- reconstruction of it.
            sol.bind("super+f2", function()
                sol.status(string.format(
                    "%d %s", workspaces.on("test-1"), tostring(tiling.active)))
            end)
            "#,
        )
        .expect("writing the test script");

        let first = &[7_u64, 8];
        let all = &[7_u64, 8, 9];

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        // The screens arrive, which is the moment the desks can be built at
        // all -- a script's top level runs before any output is placed.
        scripts.monitors_changed(one_screen(first));
        // Two windows open on workspace 1, then the session moves to 3 with
        // tiling on, and a third window opens there.
        scripts.opened(7, one_screen(first));
        scripts.opened(8, one_screen(first));
        scripts.key("super+f1", one_screen(first));
        scripts.opened(9, one_screen(all));

        let before = scripts.key("super+f2", one_screen(all));
        assert_eq!(
            before.status.as_deref(),
            Some("3 true"),
            "the session was not set up: this is before any reload"
        );

        // Exactly what `Solium::reload` does, in the order it does it.
        let carried_over = scripts.kept();
        let mut scripts =
            Scripts::load_carrying(&config, carried_over).expect("reloading the test script");
        let restored = scripts.restored(one_screen(all));
        let monitors = scripts.monitors_changed(one_screen(all));
        let layout = scripts.relayout(one_screen(all));

        let after = scripts.key("super+f2", one_screen(all));
        assert_eq!(
            after.status.as_deref(),
            Some("3 true"),
            "after the reload the scripts believe they are somewhere else: the workspace in \
             view and the layout in charge are both part of the session, not of the file"
        );

        let mut commands = restored.commands;
        commands.extend(monitors.commands);
        commands.extend(layout.commands);

        let who = membership(&commands);
        assert_eq!(
            who.get("desk-1@test-1").map(Vec::as_slice),
            Some([7, 8].as_slice()),
            "the windows that were on workspace 1 are not on desk 1 any more; \
             memberships found: {who:?}"
        );
        assert_eq!(
            who.get("desk-3@test-1").map(Vec::as_slice),
            Some([9].as_slice()),
            "the window that was on workspace 3 is not on desk 3 any more; \
             memberships found: {who:?}"
        );

        // And the desks sit relative to the one in view. Desk 3 is what is
        // being looked at, so it is carried nowhere; desk 1 is two cells to
        // its left, at 1600 x 1.06 each. Off by this is the off-stage
        // desktop the issue was reported as.
        let where_they_sit = carried(&commands);
        assert_eq!(
            where_they_sit.get("desk-3@test-1"),
            Some(&(0.0, 0.0)),
            "the workspace in view is not at the origin; offsets: {where_they_sit:?}"
        );
        let (dx, dy) = where_they_sit
            .get("desk-1@test-1")
            .copied()
            .unwrap_or_default();
        assert!(
            (dx - (-2.0 * 1600.0 * 1.06)).abs() < 0.5 && dy == 0.0,
            "desk 1 sits at {dx},{dy}, which is not two screens to the left of desk 3"
        );
    }

    /// **A reload that shortens the arrangement still leaves every window
    /// somewhere you can reach.**
    ///
    /// The failure the fix for #116 introduced, which the bug it fixed did not
    /// have. `workspaces.of` is now carried across the reload, and the
    /// `restore` sweep forgot the abandoned *desks* without touching the
    /// entries naming them -- so `columns = 4` edited to `columns = 2` left a
    /// window remembering workspace 4, in no group at all:
    ///
    ///   * `regroup` loops `1..count`, so nothing ever names it -- it is drawn
    ///     over whatever workspace is in view, on top of windows that belong
    ///     there;
    ///   * `visible()` filters it out, so no layout arranges it;
    ///   * and `go()` clamps to `count()`, so no key switches to where it
    ///     thinks it is.
    ///
    /// Before #116 `of` was wiped on every reload and the window came back on
    /// the workspace in view. The same is true of `showing`, and worse: a
    /// session looking at workspace 4 when the arrangement shrinks to two
    /// carries *every* surviving desk off to the left of a workspace that no
    /// longer exists, which is the off-stage desktop the issue was reported as,
    /// reached by the other door.
    ///
    /// Same shape as the reload test above, with the configuration edited
    /// between the two loads -- which is what a reload is for.
    #[test]
    fn a_reload_with_fewer_workspaces_leaves_no_window_on_a_desk_that_is_gone() {
        if let Some(own) = Scripts::user_config_dir()
            && (own.join("config.lua").exists()
                || own.join("user.lua").exists()
                || own.join("workspaces.lua").exists()
                || own.join("modes.lua").exists())
        {
            return;
        }

        let directory = std::env::temp_dir().join("solium-script-test-reload-shorter");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            local workspaces = require("workspaces")

            sol.bind("super+f1", function() workspaces.go(3) end)
            sol.bind("super+f2", function() workspaces.go(4) end)
            sol.bind("super+f3", function()
                sol.status(string.format("%d/%d",
                    workspaces.on("test-1"), workspaces.count()))
            end)
            "#,
        )
        .expect("writing the test script");

        let first = &[7_u64];
        let all = &[7_u64, 9];

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        scripts.monitors_changed(one_screen(first));
        // One window on workspace 1, then to workspace 3, a window opens there,
        // and the session is left looking at workspace 4.
        scripts.opened(7, one_screen(first));
        scripts.key("super+f1", one_screen(first));
        scripts.opened(9, one_screen(all));
        scripts.key("super+f2", one_screen(all));

        let before = scripts.key("super+f3", one_screen(all));
        assert_eq!(
            before.status.as_deref(),
            Some("4/4"),
            "the session was not set up: this is before any reload"
        );

        // The edit. `require` searches the configuration's own directory first,
        // so the *second* load finds this and the first did not.
        std::fs::write(
            directory.join("user.lua"),
            "return { workspaces = { columns = 2 } }\n",
        )
        .expect("writing the fixture user.lua");

        let carried_over = scripts.kept();
        let mut scripts =
            Scripts::load_carrying(&config, carried_over).expect("reloading the test script");
        let restored = scripts.restored(one_screen(all));
        let monitors = scripts.monitors_changed(one_screen(all));
        let layout = scripts.relayout(one_screen(all));

        let after = scripts.key("super+f3", one_screen(all));
        assert_eq!(
            after.status.as_deref(),
            Some("2/2"),
            "after the reload the scripts are still looking at a workspace the \
             arrangement no longer has, and `go` clamps to the count -- so no key \
             switches away from it"
        );

        let mut commands = restored.commands;
        commands.extend(monitors.commands);
        commands.extend(layout.commands);

        let who = membership(&commands);
        assert_eq!(
            who.get("desk-1@test-1").map(Vec::as_slice),
            Some([7].as_slice()),
            "the window on workspace 1 is not on desk 1 any more; memberships: {who:?}"
        );
        assert_eq!(
            who.get("desk-2@test-1").map(Vec::as_slice),
            Some([9].as_slice()),
            "the window kept on workspace 3 is in no selection at all: nothing names \
             it, so no layout arranges it and no key reaches it; memberships: {who:?}"
        );

        // And the desk in view is the one in view, rather than one cell to the
        // left of a workspace that no longer exists.
        let where_they_sit = carried(&commands);
        assert_eq!(
            where_they_sit.get("desk-2@test-1"),
            Some(&(0.0, 0.0)),
            "every surviving desk is carried off-stage, relative to a workspace the \
             arrangement no longer has; offsets: {where_they_sit:?}"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A keyboard section that says what layout to start on is not a typo.**
    ///
    /// `config.lua` restates `sol.keyboard`'s key set, because `keyboard = {}`
    /// is empty on purpose and so cannot be the list. The restatement left
    /// `active` out, and `sol.keyboard` reads it -- so a dual-layout
    /// configuration, the only kind that has an `active` to name, was told its
    /// working setting is read by nothing and `solium --check` exited 1 (#117
    /// review).
    ///
    /// That is worse than the silence #117 replaced. `--check` answers "did my
    /// configuration work", and a check that is wrong about a setting people
    /// really write is a check they stop running. See
    /// `every_key_a_section_accepts_is_one_the_compositor_reads`, which is what
    /// stops the list drifting again.
    #[test]
    fn a_keyboard_section_naming_its_layouts_and_which_is_live_is_not_a_typo() {
        let Some((directory, scripts)) = shipped_init_with_user(
            "solium-script-test-keyboard-active",
            r#"
            return {
                keyboard = { layout = "us,ru", active = 2 },
            }
            "#,
        ) else {
            return;
        };

        let reported: Vec<String> = scripts
            .unknown_settings()
            .into_iter()
            .map(|setting| setting.key)
            .collect();
        assert!(
            reported.is_empty(),
            "a configuration naming two layouts and which of them is live is reported \
             as read by nothing, and `--check` exits 1 over a setting that works: \
             {reported:?}"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// Every key `config.lua`'s `open_sections` accepts for one section.
    ///
    /// Scraped from the shipped file rather than restated here: a third copy of
    /// a list whose second copy is the defect under test would be the same
    /// mistake with more steps.
    fn accepted_keys(section: &str) -> Vec<String> {
        let path = crate::assets::lua().join("config.lua");
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let Some(table) = text.split("local open_sections = {").nth(1) else {
            return Vec::new();
        };
        let Some(rest) = table.split(&format!("{section} = {{")).nth(1) else {
            return Vec::new();
        };
        // Comments stripped before anything else is looked for, because a line
        // documenting `sol.keyboard{ active = 2 }` holds both a closing brace
        // and an `=` -- so leaving them in ends the table early and drops the
        // very key the comment is about. No string in this table holds a `--`.
        let without_comments: String = rest
            .lines()
            .map(|line| line.split_once("--").map_or(line, |(code, _)| code))
            .collect::<Vec<&str>>()
            .join("\n");
        let Some(body) = without_comments.split('}').next() else {
            return Vec::new();
        };
        let mut found: Vec<String> = body
            .split(',')
            .filter_map(|entry| entry.split_once('='))
            .filter(|(_, value)| value.trim() == "true")
            .map(|(key, _)| key.trim().to_owned())
            .collect();
        found.sort();
        found
    }

    /// The keys a compositor function reads out of the table it is handed.
    ///
    /// Asked of the function by handing it one that records what is looked up
    /// in it, so this cannot be a restatement of the reads and cannot drift
    /// from them however they are spelled -- `sol.keyboard` reads five of its
    /// keys through a closure and three directly, and an `options.get(` scan
    /// would see only three.
    fn keys_read_by(name: &str) -> Vec<String> {
        let directory = std::env::temp_dir().join(format!("solium-script-test-reads-{name}"));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            format!(
                r#"
                sol.bind("Super+P", function()
                    local seen = {{}}
                    sol.{name}(setmetatable({{}}, {{
                        __index = function(_, key) seen[#seen + 1] = tostring(key) end,
                    }}))
                    table.sort(seen)
                    sol.status(table.concat(seen, " "))
                end)
                "#
            ),
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let outcome = scripts.key("super+p", empty_snapshot());
        let _ = std::fs::remove_dir_all(&directory);
        let mut found: Vec<String> = outcome
            .status
            .unwrap_or_default()
            .split_whitespace()
            .map(ToOwned::to_owned)
            .collect();
        // Sorted on the Lua side already; a key read twice is one key.
        found.dedup();
        found
    }

    /// **The key sets `config.lua` restates are the ones the compositor reads.**
    ///
    /// `keyboard` and `cursor` default to `{}` -- empty *means* "whatever the
    /// session already said" -- so the defaults cannot double as the list of
    /// what exists, and `config.lua` writes the list out. #117 said in a comment
    /// that a reader growing a key would make this report that key as a typo,
    /// and called that loud. It was not loud: the list shipped already missing
    /// `active`, and nothing failed.
    ///
    /// So the two halves are compared here instead, and in both directions. A
    /// key the compositor reads and the list omits is a working setting called
    /// a typo, which exits `--check` non-zero over nothing; a key the list
    /// accepts and nothing reads is #117's original fault, read from the other
    /// side -- a real typo waved through.
    #[test]
    fn every_key_a_section_accepts_is_one_the_compositor_reads() {
        for (section, function) in [("keyboard", "keyboard"), ("cursor", "cursor_theme")] {
            let read = keys_read_by(function);
            let accepted = accepted_keys(section);
            // Either scrape coming back empty would make this agree with
            // anything, and both have a way of doing that quietly: a renamed
            // `open_sections`, or a `sol.` function that stopped being reached.
            assert!(
                read.len() >= 2 && accepted.len() >= 2,
                "sol.{function} was seen reading {read:?} and config.lua's \
                 open_sections.{section} accepts {accepted:?}; one of the two walks \
                 is broken, not the lists"
            );
            assert_eq!(
                accepted, read,
                "config.lua's open_sections.{section} and the keys sol.{function} \
                 actually reads have drifted apart: a key only the compositor reads \
                 is a working setting `--check` calls a typo, and a key only the \
                 list has is a typo `--check` waves through"
            );
        }
    }
}

/// **The configuration that ships may only ask for things the compositor has.**
///
/// `init.lua` asked for an easing called `inOutCubic` and the engine had no
/// such curve. `parse_easing` warned, fell back to the default, and the session
/// carried on -- so the genie was quietly the wrong animation for as long as it
/// took somebody to read a hardware log and notice five identical warnings in
/// it. Nothing in the gate was looking, because nothing in the gate reads Lua
/// for anything but syntax: `solium --check` loads the scripts, and a name
/// inside a binding's body is not looked up until the binding runs.
///
/// That is a class rather than an incident. A script names things -- easings,
/// events, layers, QML scenes, decorations, keys -- and every one of those
/// lookups either warns and carries on or, in the case of an event and a key,
/// misses in complete silence. So this walks `lua/*.lua` and resolves each name
/// through the same function the compositor uses at run time.
///
/// **What it cannot see.** Only names written as literals. `shell.lua` takes
/// its scene from the environment and `workspaces.lua` builds `"super+" ..
/// index` in a loop; a name assembled at run time is outside this and outside
/// any static check. Lua comments are stripped, so a documented example that is
/// deliberately a placeholder -- a path into somebody's home directory -- does
/// not fail a build.
#[cfg(test)]
mod shipped {
    use super::{Curve, Scripts, normalise_combo};

    /// The Lua the compositor ships, as `(file, text)`.
    ///
    /// From `CARGO_MANIFEST_DIR` rather than a path relative to the process,
    /// because a test's working directory is the workspace root and this file
    /// should not have to know that.
    fn scripts() -> Vec<(String, String)> {
        let directory = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/lua"));
        let Ok(entries) = std::fs::read_dir(directory) else {
            return Vec::new();
        };
        let mut found: Vec<(String, String)> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|end| end == "lua"))
            .filter_map(|path| {
                let name = path.file_name()?.to_str()?.to_owned();
                Some((name, std::fs::read_to_string(&path).ok()?))
            })
            .collect();
        found.sort();
        // A check that walks an empty directory passes, which is the one way
        // this could be green and mean nothing at all.
        assert!(
            found.len() >= 8,
            "found {} shipped scripts in {}; the walk is broken, not the scripts",
            found.len(),
            directory.display()
        );
        found
    }

    /// One line with its Lua comment removed.
    ///
    /// `--` outside a string starts a comment. Tracked rather than searched for
    /// because `config.lua` has a `"module 'user' not found"` in it and a naive
    /// cut would one day land inside a string like that one.
    fn code(line: &str) -> &str {
        let bytes = line.as_bytes();
        let mut quoted = false;
        let mut index = 0;
        while index < bytes.len() {
            match bytes[index] {
                b'\\' if quoted => index += 1,
                b'"' => quoted = !quoted,
                b'-' if !quoted && bytes.get(index + 1) == Some(&b'-') => return &line[..index],
                _ => {}
            }
            index += 1;
        }
        line
    }

    /// Every double-quoted literal that follows `marker`, with where it was.
    ///
    /// `marker` is matched literally and the literal must come next, with only
    /// spaces between: `easing =` finds `easing = "outCubic"` and `sol.on(`
    /// finds `sol.on("open", ...)`. Anything else after the marker -- a
    /// variable, a table, a concatenation -- is skipped rather than guessed at.
    /// That is the limit this module states up front, and it is why
    /// `shell.lua`'s `scene = scene` never appears here.
    fn named(text: &str, marker: &str) -> Vec<(usize, String)> {
        // An empty marker matches at every position and consumes none of them,
        // so the walk below would never move. Refused here rather than left to
        // a caller, because the symptom is a test run that never finishes --
        // which is how this was found.
        assert!(!marker.is_empty(), "a marker has to be something");
        let mut found = Vec::new();
        for (number, line) in text.lines().enumerate() {
            let line = code(line);
            let mut from = 0;
            while let Some(at) = line[from..].find(marker) {
                let after = from + at + marker.len();
                from = after;
                let rest = line[after..].trim_start_matches(' ');
                let Some(rest) = rest.strip_prefix('"') else {
                    continue;
                };
                let Some(end) = rest.find('"') else {
                    continue;
                };
                found.push((number + 1, rest[..end].to_owned()));
            }
        }
        found
    }

    /// Every double-quoted literal in a chunk of Lua, in order.
    ///
    /// For a list rather than an assignment: `{ "top", "left", ... }` has no
    /// marker in front of each item, only in front of the whole thing.
    fn quoted(text: &str) -> Vec<String> {
        let mut found = Vec::new();
        for line in text.lines() {
            let mut rest = code(line);
            while let Some(at) = rest.find('"') {
                let after = &rest[at + 1..];
                let Some(end) = after.find('"') else {
                    break;
                };
                found.push(after[..end].to_owned());
                rest = &after[end + 1..];
            }
        }
        found
    }

    /// Everything in the shipped Lua that follows `marker`, for every file.
    fn everywhere(marker: &str) -> Vec<(String, usize, String)> {
        scripts()
            .into_iter()
            .flat_map(|(file, text)| {
                named(&text, marker)
                    .into_iter()
                    .map(move |(line, value)| (file.clone(), line, value))
            })
            .collect()
    }

    /// Where the QML that ships lives.
    fn shipped_qml() -> std::path::PathBuf {
        std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/qml"))
    }

    /// **Every easing the shipped configuration asks for exists.**
    ///
    /// The one that did not: `sol.animate({ duration = 520, easing =
    /// "inOutCubic" })`, in `init.lua` for `super+m` and again in `tweaks.lua`
    /// for the genie tweak. `Curve::from_name` knew five names and that was not
    /// one of them, so both fell back to `outCubic` -- a different animation,
    /// chosen by nobody, announced only in a log line.
    #[test]
    fn every_easing_named_is_one_the_engine_has() {
        let asked = everywhere("easing =");
        assert!(!asked.is_empty(), "no easings found; the scan is broken");
        for (file, line, name) in asked {
            assert!(
                Curve::from_name(&name).is_some(),
                "{file}:{line} asks for easing {name:?}, which Curve::from_name cannot read. \
                 Names: {:?}",
                Curve::all().map(|(known, _)| known)
            );
        }
    }

    /// **Every effect the shipped configuration asks for exists.**
    ///
    /// The easing case above, one layer over. `sol.present` takes `deform = {
    /// effect = "..." }` and the name is resolved in `crates/effects` rather
    /// than matched in `script.rs`, which is what makes adding *fold* or
    /// *curl* a file in that crate — and which also means a misspelling is no
    /// longer a Lua error but a warning and an undeformed window, seen by
    /// nobody who was not reading the log.
    #[test]
    fn every_effect_named_is_one_the_engine_has() {
        let asked = everywhere("effect =");
        assert!(!asked.is_empty(), "no effects found; the scan is broken");
        for (file, line, name) in asked {
            assert!(
                solium_effects::Deform::from_name(&name, &()).is_some(),
                "{file}:{line} asks for effect {name:?}, which Deform::from_name cannot read. \
                 Names: {:?}",
                solium_effects::Deform::all().map(|(known, _)| known)
            );
        }
    }

    /// **And every axis, which is worse when it is wrong.**
    ///
    /// An unknown *effect* is at least logged. An unknown parameter cannot be:
    /// `crates/effects` has no logger by construction, and a word it cannot
    /// read is indistinguishable there from one nobody wrote — so a genie
    /// asked to sweep `"downwards"` silently sweeps `down`, which is right
    /// four times in four and wrong the once somebody puts the dock on the
    /// left.
    #[test]
    fn every_axis_named_is_one_the_engine_has() {
        let asked = everywhere("axis =");
        assert!(!asked.is_empty(), "no axes found; the scan is broken");
        for (file, line, name) in asked {
            assert!(
                solium_effects::Axis::from_name(&name).is_some(),
                "{file}:{line} asks for axis {name:?}, which Axis::from_name cannot read. \
                 Names: {:?}",
                solium_effects::Axis::all().map(|(known, _)| known)
            );
        }
    }

    /// **Every event a shipped script listens for is one the compositor sends.**
    ///
    /// Worse than an unknown easing, because there is no warning at all:
    /// `sol.on` puts the handler in a table under whatever name it was given,
    /// and a name nothing dispatches is a handler that is simply never called.
    /// A layout that misspells `"monitors"` does not fail -- it stops
    /// rearranging when a screen is unplugged, which reads as a compositor bug.
    ///
    /// The vocabulary is read out of `script.rs` itself rather than listed
    /// here, because a list is a second copy and two copies of a vocabulary
    /// drifting apart is the defect this whole module exists for.
    #[test]
    fn every_event_listened_for_is_one_that_is_sent() {
        // The production half of this file. Cut at the first `#[cfg(test)]`
        // because everything below it -- including this test -- talks *about*
        // `call_listeners` and would otherwise be counted as calling it.
        const SOURCE: &str = include_str!("script.rs");
        let Some(production) = SOURCE.split("#[cfg(test)]").next() else {
            panic!("script.rs is empty, which cannot be");
        };
        let dispatched: Vec<String> = named(production, "call_listeners(sol,")
            .into_iter()
            .map(|(_, name)| name)
            .collect();

        // The scrape has to see every dispatch there is, or it narrows the
        // vocabulary in silence and this test starts agreeing with anything.
        // One `call_listeners` is the definition; the rest are calls.
        let calls = production.matches("call_listeners").count() - 1;
        assert_eq!(
            dispatched.len(),
            calls,
            "{calls} dispatches in script.rs but only {} are `call_listeners(sol, \"name\")`; \
             one of them is written some other way and this check cannot see it",
            dispatched.len()
        );
        assert!(
            calls >= 8,
            "only {calls} dispatches found; the scan is broken"
        );

        for (file, line, event) in everywhere("sol.on(") {
            assert!(
                dispatched.contains(&event),
                "{file}:{line} listens for {event:?}, which nothing dispatches. \
                 Events: {dispatched:?}"
            );
        }
    }

    /// **Every layer a shipped surface names is one that exists.**
    ///
    /// `Layer::parse` answers `None` and `unwrap_or_default` turns that into
    /// `Background`, with no warning anywhere. A bar that asked for `"Top"` and
    /// got the background is a bar drawn underneath every window on the screen,
    /// and the symptom is that the bar has vanished.
    #[test]
    fn every_layer_named_is_one_that_exists() {
        let asked = everywhere("layer =");
        assert!(!asked.is_empty(), "no layers found; the scan is broken");
        for (file, line, name) in asked {
            assert!(
                crate::scripted::Layer::parse(&name).is_some(),
                "{file}:{line} puts a surface on layer {name:?}, which is not one of \
                 background, bottom, top, overlay"
            );
        }
    }

    /// **Every QML scene a shipped script names is one that ships.**
    ///
    /// Two lookups, because there are two kinds of scene and a script writes
    /// them the same way: a scripted surface's, found on the QML search path by
    /// `scripted::find_scene`, and a loading scene's, which is a name under
    /// `qml/loading`. A scene that is not there is an `ERROR` and a hole in the
    /// picture -- `sol.surface` says "no such QML scene" and draws nothing.
    ///
    /// The resolved path has to land inside the shipped QML directory. Without
    /// that, a developer with their own `~/.config/solium/qml/wallpaper.qml`
    /// would have a green test for a file that does not ship.
    #[test]
    fn every_scene_named_is_one_that_ships() {
        let shipped = shipped_qml();
        let asked = everywhere("scene =");
        assert!(!asked.is_empty(), "no scenes found; the scan is broken");
        for (file, line, name) in asked {
            let surface = crate::scripted::find_scene(&name)
                .is_some_and(|path| path.starts_with(&shipped) && path.is_file());
            let loading = shipped
                .join("loading")
                .join(format!("{name}.qml"))
                .is_file();
            assert!(
                surface || loading,
                "{file}:{line} names scene {name:?}, and there is no {name} in \
                 {} or in its loading directory",
                shipped.display()
            );
        }
    }

    /// **Every surface a shipped selection names is one a shipped script
    /// declares.**
    ///
    /// `sol.group` is the third place a script writes a surface's name down, and
    /// the quietest of the three. A name nothing has declared is not an error:
    /// the selection simply does not contain it, so the group is declared, the
    /// transform is applied, and the wallpaper stays exactly where it was —
    /// which is the bug this whole item exists to remove, wearing a typo.
    ///
    /// Both halves are read as literals, so what this can see is bounded the way
    /// the module header says. The shipped desks build their surface names from
    /// a workspace index (`wallpaper.for_desk`), and a name assembled at run time
    /// is outside this and outside any static check — so the *membership* side
    /// may legitimately be empty. What is asserted non-empty is the declaration
    /// side, which proves the scan itself still works on this corpus; a literal
    /// written into a group tomorrow is checked against it from that day on.
    #[test]
    fn every_surface_a_selection_names_is_one_that_is_declared() {
        let declared: Vec<String> = everywhere("sol.surface(")
            .into_iter()
            .map(|(_, _, name)| name)
            .collect();
        assert!(
            !declared.is_empty(),
            "no `sol.surface` declarations found; the scan is broken"
        );

        // Every literal inside a `surfaces = { ... }` list, with its file.
        let mut asked: Vec<(String, usize, String)> = Vec::new();
        for (file, text) in scripts() {
            for (line, rest) in surface_lists(&text) {
                for name in quoted(&rest) {
                    asked.push((file.clone(), line, name));
                }
            }
        }
        for (file, line, name) in asked {
            assert!(
                declared.contains(&name),
                "{file}:{line} puts surface {name:?} in a selection, and nothing                  declares one by that name. Declared: {declared:?}"
            );
        }
    }

    /// The text of each `surfaces = { ... }` list in a chunk of Lua, with the
    /// line it starts on.
    ///
    /// A list rather than an assignment, so [`named`] cannot see it: there is no
    /// marker in front of each item, only in front of the whole thing. Single
    /// line only, which is how every one of them is written and what the whole
    /// of this module can see.
    fn surface_lists(text: &str) -> Vec<(usize, String)> {
        let mut found = Vec::new();
        for (number, line) in text.lines().enumerate() {
            let line = code(line);
            let Some(at) = line.find("surfaces = {") else {
                continue;
            };
            let rest = &line[at..];
            let end = rest.find('}').unwrap_or(rest.len());
            found.push((number + 1, rest[..end].to_owned()));
        }
        found
    }

    /// **Every pane style the shipped configuration names is one that ships.**
    ///
    /// A name that is nowhere is a frame that fails to build once per window --
    /// every window undecorated, and a log line each.
    ///
    /// Resolved through `decoration::ships` rather than by joining a path onto
    /// the name here, which is this module's own rule -- "resolves each name
    /// through the same function the compositor uses at run time" -- and which
    /// this test was the one exception to. It matters rather than being a
    /// tidy-up, and Task 7 is where it would have bitten: the eight names
    /// `config.lua` has always carried stopped being files under
    /// `qml/decorations` and became folders under `qml/panes` on one commit,
    /// and a hand-joined `<name>.qml` would have gone red for all eight while
    /// the compositor drew them perfectly. Through the resolver there was
    /// nothing to remember.
    ///
    /// Both markers, because the setting is `pane` and `decoration` is still
    /// read: a user.lua written before the rename still names a style, and a
    /// name that has to resolve is a name this has to check. `everywhere` sees
    /// only the shipped scripts, so what this really pins is that the alias in
    /// `config.lua` cannot be left pointing at a style that was deleted.
    #[test]
    fn every_pane_style_named_is_one_that_ships() {
        let ships = crate::decoration::ships();
        // The walk before anything is decided by it: a catalogue that came back
        // empty would pass every name below by having no opinion, which is the
        // one way this can be green and mean nothing.
        assert!(
            ships.len() >= 8,
            "this build offers {} styles, which is fewer than the eight that \
             `config.lua` documents -- the walk is broken, not the configuration",
            ships.len()
        );
        let mut asked = everywhere("pane =");
        asked.extend(everywhere("decoration ="));
        assert!(
            !asked.is_empty(),
            "no pane styles found; the scan is broken"
        );
        for (file, line, name) in asked {
            // `none` is a real setting and draws no frame at all, deliberately.
            assert!(
                name == "none" || ships.iter().any(|offered| offered.name == name),
                "{file}:{line} asks for pane style {name:?}, and this build ships no style \
                 of that name -- it ships {:?}",
                ships
                    .iter()
                    .map(|offered| offered.name.as_str())
                    .collect::<Vec<_>>()
            );
        }
    }

    /// **Every key a shipped script binds is spelled the way a key arrives.**
    ///
    /// A binding is a table lookup on a string. `input::combo_for` builds that
    /// string from `xkb::keysym_get_name` and `normalise_combo` lowercases it,
    /// so a binding whose key is not that exact spelling is not a binding at
    /// all -- it is an entry in a table nothing will ever look up, with no
    /// warning at load and nothing at the press but a `no script has bound
    /// this` at info.
    ///
    /// Round-tripped rather than merely resolved: `keysym_from_name` is
    /// forgiving and `keysym_get_name` is not, and it is the second one that
    /// decides what a key is called at run time.
    ///
    /// Only the bindings written as literals after `sol.bind(`. `workspaces.lua`
    /// builds nine of them from a loop counter and they cannot be read from the
    /// text -- see this module's header -- and `scrolling.lua` binds through a
    /// local `bind` helper this marker does not match.
    ///
    /// And it checks the *spelling*, not that the key can be reached. Those are
    /// not the same question: `super+shift+1` is spelled perfectly, and until
    /// #121 no key on a `us` keyboard produced it, because `shift+1` arrives as
    /// `exclam` and that was the only name a press was looked up by.
    /// Reachability is `every_shipped_binding_is_reachable_on_us` below, which
    /// reads the bindings from the loaded scripts rather than from the text and
    /// so sees the loop-built and helper-built ones this one skips.
    #[test]
    fn every_key_bound_is_spelled_the_way_it_arrives() {
        use smithay::input::keyboard::xkb;

        let bound = everywhere("sol.bind(");
        assert!(!bound.is_empty(), "no bindings found; the scan is broken");
        for (file, line, combo) in bound {
            let combo = normalise_combo(&combo);
            let Some(key) = combo.rsplit('+').next() else {
                continue;
            };
            // A combo built by concatenation -- `"super+" .. index` -- reaches
            // this as a bare `super+` with nothing after it. There is no key
            // here to check; the header says so.
            if key.is_empty() {
                continue;
            }
            let keysym = xkb::keysym_from_name(key, xkb::KEYSYM_CASE_INSENSITIVE);
            assert!(
                keysym != xkb::keysyms::KEY_NoSymbol.into(),
                "{file}:{line} binds {combo:?}, and xkb has no key called {key:?}"
            );
            let canonical = xkb::keysym_get_name(keysym).to_ascii_lowercase();
            assert_eq!(
                canonical, key,
                "{file}:{line} binds {combo:?}, but that key arrives called {canonical:?} -- \
                 the binding would never fire"
            );
        }
    }

    /// A `us` keymap, compiled from the real rules, and its modifier indices.
    ///
    /// Real rather than described, because the question these tests ask is
    /// what xkb does and not what anyone believes it does.
    fn us_keymap() -> (smithay::input::keyboard::xkb::Keymap, UsModifiers) {
        use smithay::input::keyboard::xkb;

        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let Some(keymap) = xkb::Keymap::new_from_names(
            &context,
            "",
            "",
            "us",
            "",
            None,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        ) else {
            // No xkb rules on this machine means no keymap to ask, and a test
            // that invented an answer here would be worse than one that says
            // it could not look.
            panic!("no `us` keymap; xkb data is missing, so this proves nothing");
        };
        let modifiers = UsModifiers {
            shift: keymap.mod_get_index(xkb::MOD_NAME_SHIFT),
            ctrl: keymap.mod_get_index(xkb::MOD_NAME_CTRL),
            alt: keymap.mod_get_index(xkb::MOD_NAME_ALT),
            logo: keymap.mod_get_index(xkb::MOD_NAME_LOGO),
        };
        (keymap, modifiers)
    }

    /// Where `us_keymap` keeps each modifier the combination syntax can name.
    struct UsModifiers {
        shift: u32,
        ctrl: u32,
        alt: u32,
        logo: u32,
    }

    /// Every name a press of xkb keycode `code` answers to while `held` is
    /// down -- the list `input::keyboard` tries bindings in.
    ///
    /// Built the way that filter builds it: `modified` is `key_get_one_sym`
    /// under the held modifiers, which is what smithay's `modified_sym` calls,
    /// and `raw` is the key's only keysym at level 0 of its layout, which is
    /// what `raw_syms` returns and the filter keeps. Then the same
    /// `input::combos_for`, so the string compared below is the string the
    /// compositor looks up and not a second opinion of it.
    fn names_for(
        keymap: &smithay::input::keyboard::xkb::Keymap,
        indices: &UsModifiers,
        code: u32,
        held: &smithay::input::keyboard::ModifiersState,
    ) -> Vec<String> {
        use smithay::input::keyboard::xkb;

        let mut mask = 0;
        for (on, index) in [
            (held.shift, indices.shift),
            (held.ctrl, indices.ctrl),
            (held.alt, indices.alt),
            (held.logo, indices.logo),
        ] {
            if on {
                mask |= 1 << index;
            }
        }
        let mut state = xkb::State::new(keymap);
        state.update_mask(mask, 0, 0, 0, 0, 0);
        let code = xkb::Keycode::new(code);
        let modified = state.key_get_one_sym(code);
        let raw = match keymap.key_get_syms_by_level(code, state.key_get_layout(code), 0) {
            [only] => Some(*only),
            _ => None,
        };
        crate::input::combos_for(held, modified, raw)
    }

    /// The modifiers a canonical combination names, as a keyboard would hold
    /// them.
    fn held_for(combo: &str) -> smithay::input::keyboard::ModifiersState {
        let named: Vec<&str> = combo.split('+').collect();
        let modifiers = named
            .split_last()
            .map(|(_, modifiers)| modifiers)
            .unwrap_or_default();
        smithay::input::keyboard::ModifiersState {
            ctrl: modifiers.contains(&"ctrl"),
            alt: modifiers.contains(&"alt"),
            shift: modifiers.contains(&"shift"),
            logo: modifiers.contains(&"super"),
            ..Default::default()
        }
    }

    /// **The two height binds in `tiling.lua` are combinations a `us` keyboard
    /// can actually produce.**
    ///
    /// Reachability, which the spelling test above deliberately does not check:
    /// it asks whether a key is *spelled* the way xkb names it, and
    /// `underscore` passes that while being a combination no one can press
    /// with the modifiers the binding also names.
    ///
    /// So this goes the other way round. It starts from the physical keys --
    /// `AE11` and `AE12`, the `-` and `=` of the top row, which are evdev 12
    /// and 13 and so xkb keycodes 20 and 21 -- presses them under the modifiers
    /// the bindings name, and asks `names_for` what that press is called.
    ///
    /// These were bound on ctrl because until #121 the shift spelling was
    /// dead: shift+`-` arrives as `underscore`, and that was the only name a
    /// press had. Ctrl selects no shift level, so it is right before that fix
    /// and after it. The second half pins the fix itself for this key: the
    /// spelling #120 started with is now one a press answers to, so the next
    /// reader who tidies these back to shift gets a working binding rather
    /// than a dead one.
    #[test]
    fn every_height_bind_is_a_key_that_arrives() {
        use smithay::input::keyboard::ModifiersState;

        let (keymap, indices) = us_keymap();
        let minus = 20;
        let equal = 21;
        let with_ctrl = ModifiersState {
            ctrl: true,
            logo: true,
            ..Default::default()
        };
        let with_shift = ModifiersState {
            shift: true,
            logo: true,
            ..Default::default()
        };

        // What `tiling.lua` binds, and what the keyboard sends. These have to
        // be the same string or the binding is an entry in a table nothing
        // looks up.
        for (code, bound) in [(minus, "super+ctrl+minus"), (equal, "super+ctrl+equal")] {
            let sent = names_for(&keymap, &indices, code, &with_ctrl);
            assert!(
                sent.contains(&normalise_combo(bound)),
                "`tiling.lua` binds {bound:?}, and that keypress answers only to {sent:?}"
            );
        }

        // And the spelling it started as, which #121 made reachable. Shift
        // still changes the keysym -- the press is called `underscore` first
        // -- but it also answers to the key it was.
        for (code, shifted, bound) in [
            (minus, "super+shift+underscore", "super+shift+minus"),
            (equal, "super+shift+plus", "super+shift+equal"),
        ] {
            let sent = names_for(&keymap, &indices, code, &with_shift);
            assert_eq!(
                sent,
                [normalise_combo(shifted), normalise_combo(bound)],
                "shift on this key should arrive as {shifted:?} and fall back to \
                 {bound:?}, modified spelling first"
            );
        }
    }

    /// **Every key a shipped script binds can be pressed on a `us` keyboard.**
    ///
    /// The question the spelling test cannot ask. This one loads the shipped
    /// `init.lua` -- so the bindings are what the running compositor has, the
    /// nine `super+shift+N` built by a loop in `workspaces.lua` and the keys
    /// `scrolling.lua` binds through its own helper included -- and for each,
    /// looks for a physical key that, held under the modifiers the binding
    /// names, answers to that exact string.
    ///
    /// Eleven failed this before #121: all nine send-to-workspace keys and both
    /// `super+shift+bracketleft`/`bracketright`, because shift turned the key
    /// into `exclam` or `braceleft` and that was the only name looked up.
    ///
    /// `package.path` is written by the entry rather than left to
    /// `Scripts::load`, which prepends the developer's own `~/.config/solium`
    /// and would make this a test of somebody's configuration -- the trap the
    /// layout harness below documents.
    #[test]
    fn every_shipped_binding_is_reachable_on_us() {
        let shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua");
        let directory = std::env::temp_dir().join("solium-reachable");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("creating the entry directory");
        let entry = directory.join("init.lua");
        std::fs::write(
            &entry,
            format!(
                "package.path = {shipped:?} .. \"/?.lua\"\ndofile({shipped:?} .. \"/init.lua\")\n"
            ),
        )
        .expect("writing the entry point");
        let scripts = Scripts::load(&entry).expect("loading the shipped init.lua");
        let bound: Vec<String> = scripts
            .bindings()
            .into_iter()
            .map(|binding| binding.combo)
            .collect();

        // The ones this is for, by name, so a load that quietly bound nothing
        // -- or a scan that lost the loop -- cannot pass by having nothing to
        // check.
        let expected = (1..=9).map(|index| format!("shift+super+{index}")).chain([
            "shift+super+bracketleft".to_owned(),
            "shift+super+bracketright".to_owned(),
        ]);
        for combo in expected {
            assert!(
                bound.contains(&combo),
                "the shipped scripts no longer bind {combo:?}; they bind {bound:?}"
            );
        }

        // Every one that fails, not the first: eleven at once is a different
        // bug from one, and a report that stops at the first cannot say which.
        let (keymap, indices) = us_keymap();
        let codes = keymap.min_keycode().raw()..=keymap.max_keycode().raw();
        let dead: Vec<&String> = bound
            .iter()
            .filter(|combo| {
                let combo = normalise_combo(combo);
                let held = held_for(&combo);
                !codes
                    .clone()
                    .any(|code| names_for(&keymap, &indices, code, &held).contains(&combo))
            })
            .collect();
        assert!(
            dead.is_empty(),
            "shipped scripts bind {dead:?}, and for each no key on a `us` keyboard, held \
             under the modifiers it names, answers to that name -- they can never fire"
        );
    }
}

/// The layouts, against the real `tiling.lua` and `scrolling.lua`.
///
/// Issue #72's harder half is not the protocol plumbing -- that is one state, a
/// handler and a delegate macro, and it is exercised by the compositor starting
/// at all. It is what the *layouts* do with a modal dialog, and that is Lua, so
/// a test of it has to run Lua.
///
/// So this runs the shipped scripts, unmodified, resolved through the same
/// `package.path` the compositor sets, against a snapshot of synthetic windows
/// -- and reads the `sol.place` commands that come back. That is the whole of
/// what a layout does, so it is the whole of what there is to check.
///
/// **Both layouts are driven by the same assertions, from one list.** A modal
/// that is right in tiling and wrong in scrolling is the failure this is most
/// likely to have, and two near-identical test functions is how it would be
/// missed: somebody fixes one and copies the test, and the copy asserts about
/// the layout it was copied from.
///
/// What is *not* here: `set_modal` arriving over the wire. That needs a Wayland
/// client, a display and a socket, and there is nothing in this crate's tests
/// that can build one -- so the step from "a client called `set_modal`" to
/// "`WindowInfo::modal` is true" is covered by the type system and by running
/// the compositor, and not by this. The snapshot is built by hand here, which
/// is honest about where the seam is.
#[cfg(test)]
mod dialogs {
    use super::{Command, MonitorInfo, Parentage, Rect, Scripts, Snapshot, WindowInfo};

    /// One screen, no bar, round numbers so a wrong answer reads as a wrong
    /// place rather than as arithmetic.
    const AREA: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 2560.0,
        h: 1440.0,
    };

    /// The second screen, where there are two: the same size, immediately to the
    /// right of the first. See [`second_monitor`].
    const BESIDE: Rect = Rect {
        x: 2560.0,
        y: 0.0,
        w: 2560.0,
        h: 1440.0,
    };

    /// Both layouts, by the module that defines them and the key that turns
    /// them on. Every assertion below runs against both; see the module note.
    const LAYOUTS: [(&str, &str); 2] = [("tiling", "super+t"), ("scrolling", "super+s")];

    /// The shipped scripts, with one layout required and nothing else.
    ///
    /// `package.path` is written by the script rather than left to
    /// `Scripts::load`, which prepends the *developer's* `~/.config/solium` --
    /// so on a machine with a user configuration this would silently test that
    /// instead. The same trap `group.rs`'s `desk` fixture documents.
    ///
    /// No `config.lua` of its own: the shipped defaults are what users get, and
    /// a dialog that is only centred under a hand-written configuration is a
    /// dialog that is not centred.
    ///
    /// A directory per *call*, not per layout, and the serial number is not
    /// decoration. `cargo test` runs these on several threads and more than one
    /// test arranges the same layout, so a path derived from the layout name is
    /// two threads writing and reading one `init.lua` at once. The symptom is
    /// not a wrong configuration -- every test writes the same bytes -- it is a
    /// truncated read, and it surfaces as "the layout key was not handled" in
    /// whichever test lost the race. Observed while mutation-testing this
    /// module: a change that should have failed one assertion failed a
    /// different test's, in a different file. `group.rs`'s `desk` fixture
    /// documents the same trap one step less far.
    fn scripts(name: &str, layout: &str) -> Scripts {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!("solium-dialogs-{name}-{serial}"));
        let _ = std::fs::create_dir_all(&directory);
        let entry = directory.join("init.lua");
        std::fs::write(
            &entry,
            format!(
                "package.path = {shipped:?} .. \"/?.lua\"\n\
                 require(\"modes\")\n\
                 require({layout:?})\n",
                shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
            ),
        )
        .expect("writing the entry point");
        Scripts::load(&entry).expect("loading the shipped layout")
    }

    fn monitor() -> MonitorInfo {
        MonitorInfo {
            name: "DP-1".to_owned(),
            area: AREA,
            whole: AREA,
            scale: 1.0,
            focused: true,
            primary: true,
            transform: "normal".to_owned(),
        }
    }

    /// The screen to the right of [`monitor`], in the one global coordinate
    /// space the compositor has: same size, offset by its whole width.
    ///
    /// Not focused and not primary, because the point of it is to be the screen
    /// nothing falls back to. Every wrong answer about where a dialog goes lands
    /// on `DP-1` — it is the monitor at the origin, where every toplevel is
    /// mapped, and the one `sol.monitor()` answers — so an assertion that a
    /// dialog is on `DP-2` cannot pass by accident.
    fn second_monitor() -> MonitorInfo {
        MonitorInfo {
            name: "DP-2".to_owned(),
            area: BESIDE,
            whole: BESIDE,
            scale: 1.0,
            focused: false,
            primary: false,
            transform: "normal".to_owned(),
        }
    }

    /// A window the compositor has decided is on this monitor.
    ///
    /// Both halves, because the compositor sets both and a layout reads them
    /// from different places: `monitor` is what `monitors.each` groups by, and
    /// the rect is what decides the answer again on the next pass
    /// (`Solium::output_of`). A fixture that moved one without the other would
    /// be a state the compositor never produces.
    fn on(monitor: &MonitorInfo, mut window: WindowInfo) -> WindowInfo {
        window.monitor.clone_from(&monitor.name);
        window.rect.x = monitor.area.x;
        window.rect.y = monitor.area.y;
        window
    }

    /// A window as it is before any layout has had a say: the size its client
    /// chose, in the corner a Wayland toplevel is mapped at.
    fn window(id: u64, w: f64, h: f64) -> WindowInfo {
        WindowInfo {
            id,
            rect: Rect {
                x: 0.0,
                y: 0.0,
                w,
                h,
            },
            drawn: Rect::default(),
            title: format!("window {id}"),
            focused: false,
            monitor: "DP-1".to_owned(),
            modal: false,
            parent: Parentage::None,
        }
    }

    fn modal(id: u64, w: f64, h: f64, parent: Parentage) -> WindowInfo {
        WindowInfo {
            modal: true,
            parent,
            ..window(id, w, h)
        }
    }

    /// What the compositor looked like, with these monitors and these windows.
    ///
    /// The monitor list is a parameter rather than a constant because it was a
    /// constant, and a one-screen fixture cannot see a whole class of bug: every
    /// window is mapped at (0, 0), so with one monitor every window is on the
    /// right monitor no matter what a layout does with the answer. `work_area`
    /// follows the focused screen, which is what `sol.monitor()` reports.
    fn snapshot_on(monitors: Vec<MonitorInfo>, windows: Vec<WindowInfo>) -> Snapshot {
        let work_area = monitors
            .iter()
            .find(|monitor| monitor.focused)
            .or_else(|| monitors.first())
            .map_or(AREA, |monitor| monitor.area);
        Snapshot {
            windows,
            monitors,
            work_area,
            ..Snapshot::default()
        }
    }

    fn snapshot(windows: Vec<WindowInfo>) -> Snapshot {
        snapshot_on(vec![monitor()], windows)
    }

    /// Where each window was told to go, by id.
    ///
    /// A layout may place the same window twice in one pass; the last one is
    /// what the compositor applies, so it is what this reports.
    fn placed(commands: &[Command]) -> std::collections::HashMap<u64, Rect> {
        let mut out = std::collections::HashMap::new();
        for command in commands {
            if let Command::Place { id, rect, .. } = command {
                out.insert(*id, *rect);
            }
        }
        out
    }

    /// Rects out of Lua are `f64` through a divide or two, so they are compared
    /// with a tolerance rather than exactly. A pixel is far below anything
    /// these assertions are about and far above the error.
    fn about(left: f64, right: f64) -> bool {
        (left - right).abs() < 1.0
    }

    fn centre(rect: Rect) -> (f64, f64) {
        (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0)
    }

    fn overlap(left: Rect, right: Rect) -> bool {
        left.x < right.x + right.w
            && right.x < left.x + left.w
            && left.y < right.y + right.h
            && right.y < left.y + left.h
    }

    /// Turn a layout on with these windows, and report where everything went.
    ///
    /// Switching the mode on runs `started`, which adopts the windows and lays
    /// them out -- the same path a user pressing the key takes.
    fn arrange_on(
        name: &str,
        monitors: Vec<MonitorInfo>,
        windows: Vec<WindowInfo>,
    ) -> std::collections::HashMap<u64, Rect> {
        let (module, key) = LAYOUTS
            .iter()
            .find(|(module, _)| *module == name)
            .copied()
            .expect("a layout this module knows");
        let mut scripts = scripts(name, module);
        let outcome = scripts.key(key, snapshot_on(monitors, windows));
        assert!(outcome.handled, "{name}: the layout key was not handled");
        placed(&outcome.commands)
    }

    fn arrange(name: &str, windows: Vec<WindowInfo>) -> std::collections::HashMap<u64, Rect> {
        arrange_on(name, vec![monitor()], windows)
    }

    /// **A modal dialog takes no share of the screen.**
    ///
    /// Stated as "the other windows are laid out exactly as they would be if
    /// the dialog were not there", which is the strongest form of it and the
    /// one that cannot pass by accident: a dialog that is merely *also* placed
    /// on top of its parent, while still holding a slot in the tree, gives the
    /// other windows different rects and fails here.
    #[test]
    fn a_modal_does_not_take_a_share_of_the_screen() {
        for (name, _) in LAYOUTS {
            let without = arrange(name, vec![window(1, 800.0, 600.0), window(2, 800.0, 600.0)]);
            let with = arrange(
                name,
                vec![
                    window(1, 800.0, 600.0),
                    window(2, 800.0, 600.0),
                    modal(3, 600.0, 400.0, Parentage::Window(1)),
                ],
            );

            for id in [1, 2] {
                let before = without.get(&id).copied().unwrap_or_default();
                let after = with.get(&id).copied().unwrap_or_default();
                assert!(
                    about(before.x, after.x)
                        && about(before.y, after.y)
                        && about(before.w, after.w)
                        && about(before.h, after.h),
                    "{name}: window {id} was given {after:?} with a dialog on screen and \
                     {before:?} without one -- the dialog is taking a share of the screen"
                );
            }
            assert!(
                with.contains_key(&3),
                "{name}: the dialog was left out of the arrangement and then never \
                 placed at all, which for a Wayland toplevel is the top-left corner"
            );
        }
    }

    /// **And it sits over the window waiting on it.**
    ///
    /// Centred on its parent's *new* slot, not on the rect the snapshot
    /// carried: the same pass that places the dialog has just moved the parent,
    /// so the snapshot rect is stale by the time it is read. Asserted against
    /// `placed[1]` for exactly that reason -- comparing with the input rect
    /// would pass whichever answer the script gave.
    #[test]
    fn a_modal_is_centred_on_its_parent() {
        for (name, _) in LAYOUTS {
            let out = arrange(
                name,
                vec![
                    window(1, 800.0, 600.0),
                    window(2, 800.0, 600.0),
                    modal(3, 600.0, 400.0, Parentage::Window(1)),
                ],
            );
            let parent = out.get(&1).copied().expect("the parent was placed");
            let dialog = out.get(&3).copied().expect("the dialog was placed");

            assert!(
                about(centre(parent).0, centre(dialog).0)
                    && about(centre(parent).1, centre(dialog).1),
                "{name}: the dialog is at {dialog:?}, centred on {:?}, but its parent is \
                 at {parent:?}, centred on {:?}",
                centre(dialog),
                centre(parent)
            );
            // Its own size, kept. A dialog knows how big it needs to be and a
            // layout that resizes it wraps the question onto four lines.
            assert!(
                about(dialog.w, 600.0) && about(dialog.h, 400.0),
                "{name}: the dialog was resized to {}x{}",
                dialog.w,
                dialog.h
            );
            // The other window is elsewhere, so this is a dialog over its own
            // parent rather than a dialog in the middle of the screen that
            // happens to be over everything.
            let other = out.get(&2).copied().expect("the other window was placed");
            assert!(
                !about(centre(parent).0, centre(other).0)
                    || !about(centre(parent).1, centre(other).1),
                "{name}: both windows are in the same place, so centring on either \
                 proves nothing"
            );
        }
    }

    /// **And it follows its parent onto the other screen.**
    ///
    /// The case the single-monitor fixture could not see, and the reason the
    /// fixture takes a monitor list now. Every toplevel is mapped at (0, 0) and
    /// the compositor decides which screen a window is on from the centre of its
    /// rect, so a dialog for a window on `DP-2` *arrives* belonging to `DP-1` —
    /// this is the state the compositor really hands a layout, not a contrived
    /// one. Clamp it into its own screen's work area and it is pinned to that
    /// screen's edge; its centre stays there, so the next pass reads back the
    /// same monitor and clamps it identically. It never converges, which is why
    /// a second pass could exist for exactly this case and not deliver it.
    ///
    /// Asserted two ways on purpose. Centred on its parent is the property; on
    /// `DP-2` at all is the one that fails loudly when the clamp is taken from
    /// the wrong screen, because the wrong answer is not a few pixels out — it
    /// is a whole monitor away, against the edge nearest the parent.
    #[test]
    fn a_modal_is_centred_on_its_parents_monitor_and_not_its_own() {
        for (name, _) in LAYOUTS {
            let screens = vec![monitor(), second_monitor()];
            let out = arrange_on(
                name,
                screens,
                vec![
                    // The parent, on the second screen.
                    on(&second_monitor(), window(1, 800.0, 600.0)),
                    // An unrelated window on the first, so the dialog cannot
                    // land on an empty screen and look right by default.
                    on(&monitor(), window(2, 800.0, 600.0)),
                    // The dialog, where the compositor puts a new toplevel: the
                    // origin, which is the *first* screen.
                    modal(3, 600.0, 400.0, Parentage::Window(1)),
                ],
            );

            let parent = out.get(&1).copied().expect("the parent was placed");
            let dialog = out.get(&3).copied().expect("the dialog was placed");

            assert!(
                parent.x >= BESIDE.x,
                "{name}: the parent was laid out at {parent:?}, which is not on DP-2 \
                 ({BESIDE:?}) -- the fixture is wrong before the dialog is even asked \
                 about"
            );
            assert!(
                about(centre(parent).0, centre(dialog).0)
                    && about(centre(parent).1, centre(dialog).1),
                "{name}: the dialog is at {dialog:?}, centred on {:?}, and its parent is \
                 on the other screen at {parent:?}, centred on {:?}",
                centre(dialog),
                centre(parent)
            );
            assert!(
                dialog.x >= BESIDE.x
                    && dialog.y >= BESIDE.y
                    && dialog.x + dialog.w <= BESIDE.x + BESIDE.w
                    && dialog.y + dialog.h <= BESIDE.y + BESIDE.h,
                "{name}: the dialog at {dialog:?} is not on DP-2 ({BESIDE:?}) at all -- it \
                 was clamped into the work area of the screen it was mapped on rather \
                 than its parent's"
            );
        }
    }

    /// **A modal on a modal is centred on where that modal ended up.**
    ///
    /// A file chooser is modal for the document and its "Replace?" prompt is
    /// modal for the chooser: two dialogs, one waiting on the other, and nothing
    /// arranges either of them. So the chooser's rect in this pass is one only
    /// this pass knows — which is what `placed` is for, and a dialog has to be
    /// written into it like any other placement.
    ///
    /// The prompt is listed *first*, which is the order that matters: the
    /// snapshot is topmost first and a prompt is above the chooser that opened
    /// it, so the naive pass reaches the child before the parent exists
    /// anywhere. Left out of `placed`, the child falls through to the snapshot
    /// — where its parent is still at the (0, 0) every toplevel is mapped at —
    /// and lands in the top-left corner.
    #[test]
    fn a_modal_waiting_on_a_modal_is_centred_on_it() {
        for (name, _) in LAYOUTS {
            let out = arrange_on(
                name,
                vec![monitor()],
                vec![
                    modal(3, 300.0, 200.0, Parentage::Window(2)),
                    modal(2, 600.0, 400.0, Parentage::Window(1)),
                    window(1, 800.0, 600.0),
                    window(4, 800.0, 600.0),
                ],
            );

            let document = out.get(&1).copied().expect("the document was placed");
            let chooser = out.get(&2).copied().expect("the chooser was placed");
            let prompt = out.get(&3).copied().expect("the prompt was placed");

            assert!(
                about(centre(document).0, centre(chooser).0)
                    && about(centre(document).1, centre(chooser).1),
                "{name}: the chooser at {chooser:?} is not over the document at \
                 {document:?}, so nothing below proves anything"
            );
            assert!(
                about(centre(chooser).0, centre(prompt).0)
                    && about(centre(chooser).1, centre(prompt).1),
                "{name}: the prompt is at {prompt:?}, centred on {:?}, and the chooser it \
                 is waiting on is at {chooser:?}, centred on {:?} -- the prompt was \
                 centred on where the chooser was in the snapshot rather than on where \
                 this pass put it",
                centre(prompt),
                centre(chooser)
            );
        }
    }

    /// **A modal whose parent cannot be found still floats.**
    ///
    /// Both ways of having nothing to centre on: the client named a parent this
    /// compositor cannot point at (`Unknown` -- not mapped, not ours, or gone
    /// while the dialog was up) and the client named none at all (`None`).
    ///
    /// The two things that must not happen are "tiled" and "lost": it keeps its
    /// size, it does not push the other windows around, and every edge of it is
    /// on the screen. A dialog at the origin passes "not tiled" and is still
    /// under the panel in the corner, so the second half is the one that
    /// matters.
    #[test]
    fn a_modal_with_no_parent_to_find_is_centred_on_the_screen() {
        for (name, _) in LAYOUTS {
            for parent in [Parentage::Unknown, Parentage::None] {
                let without = arrange(name, vec![window(1, 800.0, 600.0)]);
                let out = arrange(
                    name,
                    vec![window(1, 800.0, 600.0), modal(3, 600.0, 400.0, parent)],
                );
                let dialog = out.get(&3).copied().expect("the dialog was placed");

                let before = without.get(&1).copied().unwrap_or_default();
                let after = out.get(&1).copied().unwrap_or_default();
                assert!(
                    about(before.w, after.w) && about(before.h, after.h),
                    "{name}/{parent:?}: the window lost room to a dialog with no parent"
                );

                assert!(
                    about(centre(dialog).0, centre(AREA).0)
                        && about(centre(dialog).1, centre(AREA).1),
                    "{name}/{parent:?}: a parentless dialog went to {dialog:?} rather than \
                     the middle of the screen"
                );
                assert!(
                    dialog.x >= AREA.x
                        && dialog.y >= AREA.y
                        && dialog.x + dialog.w <= AREA.x + AREA.w
                        && dialog.y + dialog.h <= AREA.y + AREA.h,
                    "{name}/{parent:?}: the dialog at {dialog:?} hangs off the work area \
                     {AREA:?}"
                );
            }
        }
    }

    /// **`unset_modal` puts it back in the arrangement.**
    ///
    /// The compositor turns `unset_modal` into a relayout -- see
    /// `XdgDialogHandler::modal_changed` -- so the second pass here is exactly
    /// what the compositor does: the same scripts, the same windows, one of
    /// them no longer modal.
    ///
    /// Rejoining is asserted two ways, because either alone is weak. The
    /// arrangement resizes it -- a floating dialog keeps the size its client
    /// chose, and this one no longer has it; and nothing overlaps anything,
    /// which is a property an arrangement has and a floating window does not,
    /// since a modal overlaps its parent by construction.
    ///
    /// What is deliberately *not* asserted is "the other windows gave up room".
    /// It is true of tiling and false of scrolling on purpose: a column's width
    /// is a share of the view, so a third column makes the strip longer rather
    /// than the first two thinner. That is the first paragraph of
    /// `scrolling.lua` and the whole reason a scroller is not a row of windows
    /// squeezed to fit, so a test that demanded it would be a test demanding
    /// the wrong layout.
    #[test]
    fn unset_modal_returns_a_dialog_to_the_arrangement() {
        for (name, key) in LAYOUTS {
            let mut scripts = scripts(&format!("{name}-unset"), name);
            let outcome = scripts.key(
                key,
                snapshot(vec![
                    window(1, 800.0, 600.0),
                    window(2, 800.0, 600.0),
                    modal(3, 600.0, 400.0, Parentage::Window(1)),
                ]),
            );
            assert!(outcome.handled, "{name}: the layout key was not handled");
            let floating = placed(&outcome.commands);

            // The client called `unset_modal`; everything else is unchanged.
            let outcome = scripts.relayout(snapshot(vec![
                window(1, 800.0, 600.0),
                window(2, 800.0, 600.0),
                window(3, 600.0, 400.0),
            ]));
            let tiled = placed(&outcome.commands);

            let was = floating.get(&3).copied().expect("the dialog was placed");
            let now = tiled.get(&3).copied().expect("the dialog is still placed");
            assert!(
                about(was.w, 600.0) && about(was.h, 400.0),
                "{name}: the dialog was {was:?} while modal, so it was never floating \
                 at its own size and this test proves nothing about it stopping"
            );
            assert!(
                !about(now.w, 600.0) || !about(now.h, 400.0),
                "{name}: the dialog still has the size its client chose ({now:?}) after \
                 `unset_modal`, so it is floating rather than arranged"
            );

            let rects: Vec<(u64, Rect)> = [1, 2, 3]
                .into_iter()
                .map(|id| {
                    (
                        id,
                        tiled
                            .get(&id)
                            .copied()
                            .unwrap_or_else(|| panic!("{name}: window {id} was not placed")),
                    )
                })
                .collect();
            for (index, (id, rect)) in rects.iter().enumerate() {
                for (other, against) in rects.iter().skip(index + 1) {
                    assert!(
                        !overlap(*rect, *against),
                        "{name}: {id} at {rect:?} overlaps {other} at {against:?}, so one \
                         of them is still floating rather than arranged"
                    );
                }
            }
        }
    }
}
