//! What a script asks for: the commands a dispatch queues, and what the
//! dispatch answers. Plain data, in no language.

use std::time::Duration;

use crate::mat4::Mat4;
use crate::present::Curve;
use crate::script::{Press, Rect, TouchAction};

/// How a batch of transforms should animate.
#[derive(Clone, Copy, Debug, PartialEq)]
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

/// A deformation as a script asked for it: the effect folder by name, its
/// params, its axis (`"auto"` or none resolved when it is applied, once),
/// how far, what it is aimed at, and its own seed and policies, each `None`
/// for `effects.present`'s. Resolved through the effect host where the
/// command is applied (`Solium::aimed`).
/// `tests::a_script_names_an_effect_and_the_engine_answers`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DeformSpec {
    pub(crate) effect: String,
    pub(crate) params: Vec<(String, solium_effects::spec::Value)>,
    pub(crate) axis: Option<String>,
    pub(crate) progress: f64,
    pub(crate) aim: Aim,
    pub(crate) seed: Option<f64>,
    pub(crate) failed: Option<crate::effect::settings::PresentFailed>,
    pub(crate) on_reload: Option<crate::effect::settings::PresentReload>,
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
        /// The client's opacity inside its pane, the style's layers left at
        /// `opacity` (#296). See [`crate::present::Frame::client_opacity`].
        client_opacity: Option<f32>,
        /// A 3D transform about `pivot`, which is the drawn rect's centre
        /// unless the script moved it. `None` keeps the window flat and on the
        /// cheap path.
        matrix: Option<Mat4>,
        /// A deformation the drawn rect cannot express, such as a genie.
        deform: Option<DeformSpec>,
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
        /// A timing per property (#293), each already resolved against the
        /// batch's own; `None` when the present named none, which moves every
        /// property on `animation`.
        channels: Option<Box<crate::present::Channels<AnimationSpec>>>,
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
        /// The client's opacity inside its pane at the start (#296).
        client_opacity: Option<f32>,
        animation: AnimationSpec,
        /// As [`Command::Present`]'s: a timing per property (#293).
        channels: Option<Box<crate::present::Channels<AnimationSpec>>>,
        /// The deformation it starts in, relaxed to none as it lands: a
        /// launch that grows out of its icon already in the icon's shape.
        deform: Option<DeformSpec>,
    },
    /// Give a window the keyboard: `sol.focus`. `source` is what the
    /// handler that asked was answering, which the `focus` event tells
    /// (#220), and `warp` a warp to carry out once the window has it.
    Focus {
        id: u64,
        source: FocusSource,
        warp: Option<Warp>,
    },
    /// Take the pointer somewhere: `sol.warp` (#220). Carried out once every
    /// command of the dispatch has been applied; the last one asked for wins.
    Warp(Warp),
    Touch {
        slot: i32,
        action: TouchAction,
    },
    Click {
        x: f64,
        y: f64,
    },
    /// Send a window fullscreen on the monitor it is on, or bring it back
    /// from fullscreen: what the client itself would ask for, asked on its
    /// behalf. `sol.toggle_fullscreen`, and `super+f` (#150).
    ToggleFullscreen {
        id: u64,
    },
    /// Maximise a window, or put it back: the frame's button, from a script.
    /// `sol.toggle_maximize`, and `super+shift+m` (#150).
    ToggleMaximize {
        id: u64,
    },
    /// Hold a window a move drag has where it is while the pointer goes on,
    /// or let it go on from where it stands: `sol.drag_hold` (#452).
    DragHold {
        id: u64,
        held: bool,
    },
    /// Where the pointer holds a window a move drag has: `along` of its
    /// frame's width and height, `by` pixels further. `sol.drag_grip`
    /// (#452).
    DragGrip {
        id: u64,
        along: (f64, f64),
        by: (f64, f64),
    },
    /// Keep a window an edge or modifier resize has at its size and place
    /// while the pointer goes on, or let the resize go on from where the
    /// pointer is: `sol.resize_hold` (#454).
    ResizeHold {
        id: u64,
        held: bool,
    },
    /// Minimize a window (#30): `sol.minimize`, and `super+m`. Nothing for
    /// a window already minimized. See `WindowInfo::minimized`.
    Minimize {
        id: u64,
    },
    /// Show a minimized window again: `sol.unminimize`. Nothing for a
    /// window that is not minimized.
    Unminimize {
        id: u64,
    },
    /// Show a minimized window anyway, where its present puts it, or stop:
    /// `sol.show_minimized`. See `Pane::shown_minimized`.
    ShowMinimized {
        id: u64,
        shown: bool,
    },
    /// Keep a window above every window that is not, or stop (#424):
    /// `sol.keep_above`. `None` toggles it.
    KeepAbove {
        id: u64,
        on: Option<bool>,
    },
    /// One press and release delivered to a window, as a device would
    /// deliver it: `sol.press`. A key gives the window the keyboard first,
    /// and the focus is told with `source`, what the handler that asked was
    /// answering. See `Solium::press`.
    Press {
        id: u64,
        press: Press,
        source: FocusSource,
    },
    /// Frame every window with a named decoration.
    Decoration {
        name: Option<String>,
    },
    /// How long a pass may spend building the frames of a new pane style:
    /// `config.pane_build` (#226).
    PaneBuild(Duration),
    /// Hand every pane's layers these values, merged into what they have.
    /// `decoration::tests::pane_values_merge_by_key_and_count_only_changes`.
    PaneValues(std::collections::BTreeMap<String, crate::json::Json>),
    /// Hand one window's frame these values, as `windowValues`, merged into
    /// what it has (#417). `tests::sol_frame_hands_one_window_its_values`.
    FrameValues {
        id: u64,
        fields: std::collections::BTreeMap<String, crate::json::Json>,
    },
    /// End the session.
    Quit,
    /// Read the configuration again.
    Reload,
    /// Take back the compositor's shortcuts from the window holding them.
    TakeShortcutsBack,
    /// Take the pointer back from the window holding it: `sol.release_pointer`
    /// (#427). See `state::pointer_hold`.
    ReleasePointer,
    /// Lock the session, as `lock.*` says: `sol.lock_now`. Only ever locks.
    LockNow,
    /// Pull Solium's own lock scenes this far down over the unlocked
    /// desktop, 0 to 1, or let them go back up: `sol.lock_peek`. A picture
    /// only: it never locks or unlocks. See `Solium::lock_peek`.
    LockPeek(Option<f64>),
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
        /// Whether `rect` is a tile the client is held inside (#133) — true
        /// unless the script said `tile = false`, which `dialogs.lua` does for
        /// a modal it centres over its parent. See `state::Standing`.
        tile: bool,
        /// The tile `rect` sits inside, when the window is placed smaller than
        /// the tile it has: `tile = { x, y, w, h }`, which `tiling.lua` says
        /// for a window centred at its own maximum size (#115). The client is
        /// held inside this, and an edge drag moves the seam from its edges,
        /// since the tile and not the window is what a seam bounds. `None` is
        /// `rect` itself. See `state::Standing::Within`.
        inside: Option<Rect>,
        /// Whether the layout says this tile is smaller than the window's own
        /// minimum. See `WindowInfo::cramped`.
        cramped: bool,
        /// Whether this is the window a `sol.move_direction` is moving: the
        /// focused one, placed while the `direction` listeners run. A
        /// fullscreen or maximised window is placed nowhere by a layout, and
        /// this is the one exception: moved onto another monitor, it is
        /// fullscreen or maximised there. See `Solium::carry_across`,
        /// `a_fullscreen_or_maximised_window_moved_to_another_monitor_is_so_on_that_one`
        /// and `a_fullscreen_window_whose_column_scrolls_over_the_next_monitor_stays_on_its_own`.
        moved: bool,
    },
    /// No layout holds this window in a tile any more: whatever it commits,
    /// it is drawn at. What `modes.use` sends for every window when the layout
    /// in charge changes, since only the script knows that it has.
    Unplace {
        id: u64,
    },
    /// Ask a window to close. A request, not a kill: the client decides.
    Close {
        id: u64,
    },
    /// One of the compositor's verbs, from `sol.act`; its outcome reaches the
    /// attempt's `done` (Ruling 15).
    /// `tests::sol_act_returns_an_attempt_and_done_hears_the_outcome_once`.
    Act {
        attempt: u64,
        action: String,
        data: crate::json::Json,
        /// What the handler that asked was answering, for a verb that
        /// focuses a window: `windows.focus` from a bar's chip is told as
        /// `"surface"`.
        source: FocusSource,
    },
    /// Start a program, connected to this compositor.
    Spawn {
        program: String,
        args: Vec<String>,
    },
    /// `sol.run`: start a program that shows no window, a command run in the
    /// background (#319): no loading window opened for it, no activation
    /// token.
    Run {
        program: String,
        args: Vec<String>,
    },
    /// `sol.run({ argv }, options)`: a command whose output, exit and lines
    /// come back to the functions kept under its id in `sol._runs` (#472).
    /// See `crate::runs`.
    RunRead(crate::runs::Request),
    /// `sol.after` (`period` none) and `sol.every`: timer `id`, whose
    /// function is kept in `sol._timers`, falls due `after` from now (#472).
    /// See `crate::timers`.
    Timer {
        id: u64,
        after: Duration,
        period: Option<Duration>,
    },
    /// A timer's `cancel`, or one whose function is gone.
    TimerCancel(u64),
    /// `sol.read` (#473): read a file on a thread of its own, for the
    /// function kept under its id in `sol._files`. See `crate::files`.
    FileRead(crate::files::ReadRequest),
    /// `sol.write` (#473): write a file atomically, in order with the other
    /// writes to its path.
    FileWrite(crate::files::WriteRequest),
    /// `sol.watch` (#473): follow the file at `path`, whose function is kept
    /// under `id` in `sol._watches`; `shown` is the path as Lua wrote it.
    Watch {
        id: u64,
        path: std::path::PathBuf,
        shown: String,
    },
    /// A watch's `cancel`, or one whose function is gone.
    WatchCancel(u64),
    /// `sol.store` read store `name`'s file, which held `text`: not saved
    /// again until it differs (#473).
    StoreLoaded {
        name: String,
        text: String,
    },
    /// A store changed since it was last saved: save it, as it is now.
    StoreSave {
        name: String,
        value: crate::json::Json,
    },
    /// How often the stores are asked for, to save them: `files.store_interval`.
    StoreInterval(Duration),
    /// `folder.trust { uri }`, validated and resolved to an absolute path by
    /// `Solium::act` (04-ui.md §4.9): mark a desktop launcher trusted,
    /// durably, so `folder.open` will run it. Its own command, rather than a
    /// direct mutation inside `act`, for the same "intent, then applied"
    /// split every other verb here keeps.
    FolderTrust {
        absolute: String,
    },
    /// How a window behaves between being asked for and its application
    /// arriving. See `Loading`.
    Loading(Loading),
    /// Whose own size limits a floating drag is held to. See [`ClientSizes`].
    ClientSizes(ClientSizes),
    /// What fills a window between an edge drag asking for a size and the
    /// client painting it. See `crate::resizing::Settings`.
    Resize(crate::resizing::Settings),
    /// What a fullscreen window covers: `fullscreen.covers`. See
    /// `crate::stack::Covers`.
    Fullscreen(crate::stack::Covers),
    /// The idle blank, and what a window on a screen that is off is told.
    /// See `crate::idle::Settings`.
    Idle(crate::idle::Settings),
    /// Whether a change under the configuration reloads on its own, and how
    /// long a burst of writes waits to go quiet first (#223). See
    /// `crate::autoreload::Settings`.
    AutoReload(crate::autoreload::Settings),
    /// libinput device settings: a default per device type, and overrides
    /// matched by name or by vendor/product. See `crate::input::devices`.
    Input(crate::input::devices::InputConfig),
    ShortcutsInhibit(crate::shortcuts_inhibit::Settings),
    /// logind's `Lock` and sleep signals: the locker to run, and whether to
    /// hold sleep for it. See `crate::logind::Settings`.
    Lock(crate::logind::Settings),
    /// `sol.notifications{ ... }`: whether Solium serves notifications, and
    /// how long banners last and how much history is kept (#346). See
    /// `crate::services::notifications::Config`.
    Notifications(crate::services::notifications::Config),
    /// `sol.notify{ ... }`: one of the shell's own notifications (#346).
    Notify(crate::services::notifications::Own),
    /// `sol.audio{ ... }`, or `sol.audio()` with nothing to set (`None`):
    /// either way, audio is wanted, and starts (#347).
    Audio(Option<crate::services::audio::model::Settings>),
    /// `sol.tray{ ... }`: whether Solium is the tray (#363). See
    /// `crate::services::tray::Config`.
    Tray(crate::services::tray::Config),
    /// `sol.media{ ... }`: whether Solium follows the session's media
    /// players (#379). See `crate::services::media::Config`.
    Media(crate::services::media::Config),
    /// How Solium's own lock checks a password: `sol.lock`'s `lockout`,
    /// `secret_layout`, `helper_timeout_ms`, `retry` and `keypad` (#281). See
    /// `crate::lock_auth::Settings`.
    LockAuth(crate::lock_auth::Settings),
    /// What Solium's own lock shows: `sol.lock`'s `native`, `scene`,
    /// `prompt_on`, `enter_ms`, `exit_ms` and `keypad_margin` (#281). See
    /// `crate::lock_scene::Settings`.
    LockScene(crate::lock_scene::Settings),
    /// Which `WM_CLASS` names are refused a tile, decoration or bar entry
    /// outright: `sol.x11`, from `config.x11` (#221). See
    /// `crate::xwayland::Settings`.
    X11(crate::xwayland::Settings),
    /// Whether `ext-workspace-v1` is offered: `sol.workspaces_offered`, from
    /// `config.workspaces.offered` (#60). See `crate::ext_workspace`.
    WorkspacesOffered(bool),
    /// Which of the window protocols a shell that is its own program is
    /// offered: `sol.toplevels`, from `config.toplevels` (#50). See
    /// `crate::toplevels::Settings`.
    Toplevels(crate::toplevels::Settings),
    /// Whether `zwlr-output-management-v1` is offered:
    /// `sol.output_management`, from `config.output_management` (#51). See
    /// `crate::output_management`.
    OutputManagement(bool),
    /// Who draws a Wayland window's frame, Solium or the application:
    /// `sol.frames`, from `config.frames` (#462). See `crate::framing`.
    Frames(crate::framing::Settings),
    /// Which of a client's layer surfaces below the windows take the pointer
    /// and touch: `sol.layer_shell`, from `config.layer_shell` (#461). See
    /// `crate::layer::Settings`.
    LayerShell(crate::layer::Settings),
    /// Override the input profile's focus policy: `sol.focus_mode`, which
    /// `lua/modes.lua` calls on every mode change and `init.lua` once at
    /// load, both from `config.focus` (#219).
    ///
    /// Every field is `Option`, and unlike [`Command::Idle`] a missing one is
    /// not a fixed default -- there is none a mode-aware call could fall back
    /// to that would still know the machine's own form factor (#159,
    /// `crate::input::profile::Profile::for_form_factor`). **`None` is the
    /// machine's own answer**, the form factor's, and never what the call
    /// before it said. It used to mean "leave the current answer alone",
    /// which made a mode that names nothing inherit the mode before it: the
    /// shipped floating's click-only, so tiling never followed the pointer
    /// (#128). `after_close`'s `None` is
    /// [`crate::successor::Successor::UNCONFIGURED`].
    /// `a_mode_that_names_nothing_gets_the_machines_own_focus_not_the_last_modes`
    /// in `state/tests.rs`.
    FocusMode {
        click: Option<bool>,
        follow: Option<bool>,
        clear_on_empty_click: Option<bool>,
        /// Where the keyboard goes when the window that has it closes, word
        /// by word (#128): `config.focus.after_close` for the mode. See
        /// `crate::successor`.
        after_close: Option<Vec<crate::successor::Successor>>,
    },
    /// Turn one monitor, by name, or every one, on or off. See `power.rs`.
    Power {
        monitor: Option<String>,
        on: bool,
    },
    /// What the workspaces are, as Lua declares them each time they change.
    /// `tests::sol_workspaces_declares_groups_and_windows`.
    Workspaces(crate::models::workspaces::Declared),
    /// Which XCursor theme the pointer is drawn from, and how big it is.
    ///
    /// Carries what the *configuration* said and nothing else — `None` in a
    /// field means "`config.lua` did not say", which is what lets
    /// `XCURSOR_THEME` and `XCURSOR_SIZE` be consulted next. The precedence is
    /// resolved in `cursor::theme::Settings::resolve`, not here, so that it is
    /// in one place and testable without Lua.
    Cursor(crate::cursor::theme::Configured),
    /// `sol.effects{ rules = … }`: the user's rules, parsed, or every error
    /// in them, applied whole or not at all
    /// (`state::tests::a_broken_rule_keeps_the_rules_that_ran`); and the
    /// engine's own keys beside them, `sandbox`, `limits`, `present`, `xray`
    /// and `memo` (`tests::sol_effects_reaches_the_compositor_with_its_settings`,
    /// `tests::sol_effects_reads_xray_and_memo`).
    Effects {
        rules: Result<Vec<crate::effect::rules::Rule>, Vec<crate::effect::rules::RuleError>>,
        settings: Result<crate::effect::settings::Settings, String>,
    },
    /// `sol.startup{ first_frame = … }`: the first frame's gate, read and
    /// checked (`warm::startup`), or why it was refused
    /// (`tests::sol_startup_reaches_the_compositor`).
    Startup(Result<crate::warm::FirstFrame, String>),
}

/// What moved the keyboard, as the `focus` event tells it and
/// `sol.focus_source()` answers (#220). A policy hangs off it: the shipped
/// one takes the pointer along for a key and not for a click
/// (`lua/follow.lua`).
///
/// A `sol.focus` is told with what its handler was answering, and a call
/// inside the handler can say more: a key binding's handler is `Binding`,
/// but a `sol.focus` a `sol.focus_direction` asks for while its listeners
/// run is `Direction`, whatever called it. The compositor's own changes of
/// focus say what they are.
/// `tests::the_focus_event_says_what_moved_the_keyboard`,
/// `state::tests::real_client::follow::the_compositors_own_focus_changes_say_what_they_are`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum FocusSource {
    /// A key binding's handler.
    Binding,
    /// A `sol.focus_direction`'s listeners.
    Direction,
    /// A `sol.move_direction`'s listeners.
    Move,
    /// A press on a window, or a `click` listener while a script holds
    /// input.
    Click,
    /// A `scroll` listener: the wheel with Super held.
    Scroll,
    /// A `drop` listener: a dragged window let go.
    Drop,
    /// A touch on a window.
    Touch,
    /// The pointer onto a window, with focus following the mouse.
    Hover,
    /// A hosted scene's action: a bar's window chip, the dock, the overview.
    Surface,
    /// A window opening.
    Open,
    /// An application asking for its window to be brought forward.
    Activate,
    /// The keyboard finding a window after the one it was on went.
    Settle,
    /// `sol.focus` from anything else.
    #[default]
    Script,
}

impl FocusSource {
    /// The word a script is told.
    /// `tests::the_focus_event_says_what_moved_the_keyboard`.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Binding => "binding",
            Self::Direction => "direction",
            Self::Move => "move",
            Self::Click => "click",
            Self::Scroll => "scroll",
            Self::Drop => "drop",
            Self::Touch => "touch",
            Self::Hover => "hover",
            Self::Surface => "surface",
            Self::Open => "open",
            Self::Activate => "activate",
            Self::Settle => "settle",
            Self::Script => "script",
        }
    }
}

/// Where a warp takes the pointer, as a script said it (#220).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum WarpTo {
    /// A point in the global space.
    Point { x: f64, y: f64 },
    /// A window, by the id a script holds it as, and where on it as a share
    /// of its width and height: the centre is `(0.5, 0.5)`. Measured where
    /// the window is headed once the dispatch's commands are applied, so a
    /// window a layout moved in the same handler is the place it was moved
    /// to. See `Solium::warp_target`.
    Window { id: u64, at: (f64, f64) },
}

/// A warp: where the pointer goes, how its picture flies there, and where
/// the mouse carries on from if it moves before the picture lands. The
/// pointer itself goes at once; with no motion its picture does too.
/// See `crate::input::warp`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Warp {
    pub(crate) to: WarpTo,
    pub(crate) motion: Option<AnimationSpec>,
    pub(crate) interrupt: Interrupt,
}

/// Where real motion mid-flight carries on from: a warp's `interrupt`
/// (#220).
/// `state::tests::real_client::follow::real_motion_mid_flight_wins_at_once`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Interrupt {
    /// From where the warp put the pointer, which is what it is over and
    /// where a press goes: the picture is there at once, and the mouse moves
    /// on from it. With focus following the mouse, the window the warp went
    /// to keeps the keyboard.
    /// `state::tests::real_client::follow::with_focus_following_the_mouse_a_nudge_mid_flight_keeps_the_keyboard_where_the_key_put_it`.
    #[default]
    Pointer,
    /// From where the picture has got to, which is where the user sees it.
    Picture,
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

/// Whose own size limits the compositor believes where it applies them itself
/// (#115): a floating window's edge drag, which no layout is asked about.
///
/// The layouts ask the same question of the same settings in `sizes.lua`,
/// which is what hands them over, with `sol.client_sizes`, when it loads:
/// `floating.client_limits`, and `tiling.client_size_ignore`. The default is
/// the one a configuration that never says so gets -- every application
/// believed. See `Solium::outer_limits`, and
/// `real_client::client_sizes::a_floating_drag_of_an_application_not_believed_is_not_held`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClientSizes {
    /// Whether a floating drag is held to a client's limits at all:
    /// `floating.client_limits`, anything but `"ignore"`.
    pub(crate) floating: bool,
    /// Applications whose limits are not believed, by the name
    /// `Solium::script_app_id` gives them.
    pub(crate) ignored: Vec<String>,
}

impl Default for ClientSizes {
    fn default() -> Self {
        Self {
            floating: true,
            ignored: Vec::new(),
        }
    }
}

impl ClientSizes {
    /// Whether the application named `app_id` gets a say in how large a
    /// floating drag makes its window. A window with no name is never on the
    /// list, as `sizes.lua` has it, even a list with an empty name on it.
    /// `real_client::client_sizes::a_floating_drag_of_an_application_not_believed_is_not_held`.
    pub(crate) fn believes(&self, app_id: &str) -> bool {
        self.floating && (app_id.is_empty() || !self.ignored.iter().any(|app| app == app_id))
    }
}

/// What became of one `sol.act`: whether the compositor did it, and if not,
/// why: `"unknown-action"`, `"unknown-window"` or `"bad-data"` (Ruling 15).
/// `tests::sol_act_returns_an_attempt_and_done_hears_the_outcome_once`,
/// `state::tests::real_client::reflow_on_close::hosted::sol_act_answers_why_it_could_not`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Settled {
    pub(crate) attempt: u64,
    pub(crate) ok: bool,
    pub(crate) reason: Option<&'static str>,
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
    /// The timing the last `sol.animate` in this dispatch set, or `None` when
    /// nothing called it: what a `fullscreen` or `maximize` listener answers
    /// the compositor's own move with.
    /// `tests::a_change_is_answered_with_the_motion_sol_animate_set`.
    pub(crate) motion: Option<AnimationSpec>,
}

impl AnimationSpec {
    /// This, with whatever a call actually named.
    pub(crate) const fn with(self, duration: Option<Duration>, easing: Option<Curve>) -> Self {
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

/// The properties a present can time apart, each with a motion of its own
/// (#293): what `sol.present`'s `motion` is keyed by.
pub(crate) const PRESENT_MOTIONS: [&str; 5] = ["x", "y", "size", "opacity", "client_opacity"];
