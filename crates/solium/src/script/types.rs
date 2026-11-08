//! What a script is told: windows, monitors, the snapshot a dispatch reads,
//! and the bindings it made. Plain data, in no language.

use smithay::utils::{Logical, Rectangle, Size};

use crate::present::Frame;

/// A routed finger event, expressed entirely in script coordinates.
#[derive(Clone, Debug)]
pub(crate) struct TouchEvent {
    pub(crate) kind: &'static str,
    pub(crate) slot: i32,
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) output: String,
    pub(crate) target: &'static str,
    pub(crate) window: Option<u64>,
    pub(crate) control: bool,
    pub(crate) edges: Option<&'static str>,
    pub(crate) time: u32,
    /// The event's own stamp, in seconds: monotonic, from the kernel's
    /// microseconds, unlike `time`, which wraps (#284).
    pub(crate) t: f64,
    /// Pixels a second, x and y, fitted by `input::velocity`.
    pub(crate) velocity: (f64, f64),
}

#[derive(Clone, Debug)]
pub(crate) enum TouchAction {
    Capture,
    Move(u64),
    Resize(
        u64,
        smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge,
    ),
}

/// A rectangle as a script sees it: plain numbers, no coordinate-space types.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Rect {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) w: f64,
    pub(crate) h: f64,
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
    /// What its frame takes of `rect` on each side right now, a titlebar the
    /// frame has grown (#423) included; nothing for a window drawn bare
    /// (#435).
    pub(crate) insets: crate::decoration::Insets,
    /// Where it is being drawn right now, which in a mode is somewhere else --
    /// in the two terms the compositor's own hit test asks. See [`Drawn`].
    pub(crate) drawn: Drawn,
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
    /// Whether this window is on its way out.
    ///
    /// True from `closing` until the window is given back (`refused`), and in
    /// `close`'s own snapshot, where the window that went is still listed so a
    /// script can ask which one it was -- whether or not it was ever
    /// `closing`, because a client that quits by itself goes straight to
    /// `close`. In no snapshot after that one: the pane waits for the end of
    /// the frame to be retired, and nothing may lay it out meanwhile. See
    /// `Solium::snapshot`.
    ///
    /// For a layout that works from this list rather than from a structure of
    /// its own: a leaving window is fading where it stands, and a slot kept
    /// for it is a slot kept for nothing. `tiling.adopt` and
    /// `scrolling.adopt` are the shipped readers.
    pub(crate) leaving: bool,
    /// Whether this window is minimized (#30): `sol.minimize`, or its own
    /// client asking. Still mapped and alive, but not drawn once a present
    /// running on it has played out, under no pointer or finger, and never
    /// holding the keyboard. The shipped layouts leave a minimized window out
    /// of their arrangement and take it back where it was when it is shown
    /// again (`unminimize`).
    /// `real_client::reflow_on_close::minimize::sol_minimize_marks_the_row_fires_once_and_leaves_the_client_mapped`.
    pub(crate) minimized: bool,
    /// Whether this window is kept above every window that is not (#424):
    /// `sol.keep_above`. Cleared when it closes; kept across a reload.
    /// `real_client::reflow_on_close::above::a_kept_above_window_stays_over_a_window_focused_after_it_and_takes_its_click`.
    pub(crate) above: bool,
    /// Where a dock shows this window, as a client of
    /// `wlr-foreign-toplevel-management` said with `set_rectangle` (#50): what
    /// a minimize can fly to. `None` until one says, and again once it takes
    /// it back. See `crate::toplevels`.
    pub(crate) icon: Option<Rect>,
    /// Which application this is, by the name it gives itself: its xdg
    /// `app_id`, or an X11 window's `WM_CLASS` class. Empty for a window
    /// whose application has not arrived, or said nothing. See
    /// `Solium::script_app_id`.
    pub(crate) app_id: String,
    /// The smallest and the largest this window's own application says it
    /// can be (#115), in the same terms as `rect` -- the frame's insets added
    /// to what the client said of its own rectangle -- so a layout compares
    /// them with a tile directly. `None` when the client limited neither side;
    /// 0 on one side is no limit on that side. See `state::limits_of` for
    /// where each protocol keeps them.
    pub(crate) min: Option<Size<i32, Logical>>,
    pub(crate) max: Option<Size<i32, Logical>>,
    /// Whether the layout in charge said, when it last placed this window,
    /// that its tile is smaller than the window's own minimum: `sol.place`'s
    /// `cramped`. A layout's word, not the compositor's measure, because only
    /// the layout knows whether it is respecting that minimum at all.
    pub(crate) cramped: bool,
    /// Whether the window's client has been shown: its first frame taken
    /// (`present::mark_shown`). False for a window launched with `sol.spawn`
    /// from its `open` until then, which is how `tiling.lua` tells a window
    /// whose first limits arrive in time to decide where it goes from one
    /// already on screen whose limits changed (#115). See
    /// `real_client::client_sizes::a_launched_window_whose_minimum_does_not_fit_goes_where_overflow_says`.
    pub(crate) shown: bool,
    /// Whether the compositor last told it to be fullscreen: the state its
    /// configures carry, which a rule's `fullscreen` match reads.
    /// `real_client::sol_windows_carries_every_window_match_key`.
    pub(crate) fullscreen: bool,
    /// Whether the compositor last told it to be maximised, as `fullscreen`
    /// says of fullscreen: what a mode switch reads to let a window out of
    /// the maximise the arrangement it is leaving gave it (#443).
    /// `real_client::a_window_maximised_while_floating_joins_its_tile_when_a_layout_takes_over`.
    pub(crate) maximized: bool,
    /// The pane style it is framed in, or `"none"` when it is drawn bare:
    /// fullscreen, or under `pane = "none"`. What a rule's `style` match
    /// reads (`state::style_fact`).
    /// `real_client::sol_windows_carries_every_window_match_key`.
    pub(crate) style: String,
    /// Whether its application draws its own titlebar, and its frame is the
    /// style's client-side one (#462): what a rule's `client_side` match reads.
    pub(crate) client_side: bool,
    /// Whether its client asked for what is behind it to be blurred, through
    /// `ext-background-effect` (#78): what a rule's `blur_region` matches.
    pub(crate) blur_region: bool,
    /// Its `_NET_WM_WINDOW_TYPE`, by the name `xwayland::window_type_name`
    /// gives it -- `"normal"` for an X11 window with none, same as
    /// `xwayland::places_itself` and `floats_over_its_parent` already read it
    /// -- or absent for a Wayland window, which has no such property to ask.
    ///
    /// The raw material a window rule (#56) needs for a case the compositor's
    /// own default does not already cover generically -- floating every
    /// `"dialog"`, say, on top of a layout that does not already do that on
    /// its own.
    pub(crate) x11_type: Option<String>,
    /// Whether the window may ever be given the keyboard: an X11 client's own
    /// `WM_HINTS.input`, or `true` for a Wayland window, which has no
    /// equivalent to decline with -- `xdg_toplevel` offers the keyboard to
    /// every surface that can be focused and nothing else.
    ///
    /// **Not what decides whether a window is hidden.** That used to be this
    /// field alone (#221's first cut), and it was wrong: ICCCM's 'Globally
    /// Active' input model is an ordinary, focusable application that sets
    /// `input: false` on purpose and takes the keyboard itself through
    /// `WM_TAKE_FOCUS`, so a row here can read `false` for a window a person
    /// is actually using. `config.x11.hidden`, matched against `class` and
    /// `instance` below, is what `map_window_request` hides on now; this
    /// field is kept, unused for that decision, so a window rule (#56) still
    /// has it to key some other decision on.
    pub(crate) accepts_input: bool,
    /// Its `WM_CLASS` class (`X11Surface::class()`), or absent for a Wayland
    /// window, which has no such property. Matched case-insensitively against
    /// `config.x11.hidden`, along with `instance`, to decide whether the
    /// window was ever shown at all (#221); kept here too for a window rule
    /// (#56) that wants to act on the same identity.
    pub(crate) class: Option<String>,
    /// Its `WM_CLASS` instance (`X11Surface::instance()`), or absent for a
    /// Wayland window. See `class`.
    pub(crate) instance: Option<String>,
    /// What the window says it is showing, through `wp_content_type_v1`
    /// (#71): `"photo"`, `"video"`, `"game"`, or `"none"` for a window that
    /// never said. What the client has committed; see `content_type_of` in
    /// `state/snapshot.rs`.
    ///
    /// The compositor decides nothing with it. It is here so a choice that
    /// would otherwise be one switch for every window -- tearing (#37), a
    /// variable refresh rate, keeping the screen awake -- can be made by a
    /// script for the window that is actually a game or a film.
    pub(crate) content_type: &'static str,
    /// The process the window belongs to, or `None` when it cannot be
    /// told: see `Solium::window_pid`. For a configuration that matches a
    /// window to a process it knows, such as one it launched.
    /// `real_client::follow::press::sol_windows_rows_carry_the_clients_pid`.
    pub(crate) pid: Option<u32>,
}

/// How a window is drawn this instant, as `Solium::window_under` reads it:
/// the slot it lives at, which is what decides the screens that draw it at all
/// ([`crate::render::drawn_on`]), and the frame it is drawn with, which is
/// what decides the pixels it owns on them ([`Frame::covers`]).
///
/// **A rectangle until #134's fourth review**, and a rectangle can say
/// neither. `sol.window_at` asked it whether it contained the point, so a
/// window on a hidden workspace carried over the next monitor -- which never
/// draws it -- was under the cursor there, and so was a window fading out at
/// opacity zero over the neighbour that grew into its place (#135). Handed to
/// scripts only through `sol.window_at`, which asks [`crate::state::owns`] of
/// it, the rule the Rust hit test asks; nothing reads it in Lua.
/// `on_two_monitors_sol_window_at_answers_what_the_right_monitor_draws` and
/// `sol_window_at_over_a_window_fading_out_answers_the_neighbour_in_its_place`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Drawn {
    pub(crate) slot: Rectangle<i32, Logical>,
    pub(crate) frame: Frame,
}

impl Default for Drawn {
    fn default() -> Self {
        Self {
            slot: Rectangle::default(),
            frame: Frame::real(Rectangle::default()),
        }
    }
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
    /// Whether it has been turned off, which leaves it here, where it was.
    /// `sol.monitors()` says `power = "off"`. See `power.rs`.
    pub(crate) off: bool,
}

/// What the compositor looked like when a handler was called.
#[derive(Clone, Debug, Default)]
pub(crate) struct Snapshot {
    /// Topmost first, so hit-testing walks it in order.
    pub(crate) windows: Vec<WindowInfo>,
    /// Every monitor, in the order the compositor holds them.
    pub(crate) monitors: Vec<MonitorInfo>,
    /// Installed, visible applications, by name (`crate::apps::scan`'s own
    /// order). `sol.apps()`. Shared, not copied, from one snapshot to the
    /// next: a snapshot is taken for every event a script hears.
    pub(crate) apps: std::rc::Rc<Vec<crate::apps::Entry>>,
    /// The keyboard, for `sol.keyboard()` and for a shell that wants to draw
    /// a layout indicator.
    pub(crate) keyboard: crate::keymap::State,
    pub(crate) shortcuts_owner: Option<u64>,
    /// The active monitor's work area — what `sol.monitor()` answers.
    ///
    /// Kept as its own field rather than found in `monitors` every time,
    /// because almost every script wants exactly this and nothing else.
    pub(crate) work_area: Rect,
    pub(crate) cursor: (f64, f64),
    /// Every monitor's rectangle, as `Solium::window_under` is handed them:
    /// the screens [`crate::state::owns`] asks about for `sol.window_at`.
    /// Empty in a snapshot with no screens, which `owns` reads as "not known
    /// to be nowhere", as the compositor's own walk does.
    /// `on_two_monitors_sol_window_at_answers_what_the_right_monitor_draws`.
    pub(crate) screens: Vec<Rectangle<i32, Logical>>,
    /// The focused text field and its caret, for `sol.text_input()`:
    /// `text_input::tests::an_enabled_field_has_its_caret_in_the_global_space`.
    pub(crate) text_input: Option<crate::text_input::Field>,
    /// What is broken in an effect, a rule or the configuration, for
    /// `sol.problems()`:
    /// `tests::a_broken_effect_reaches_sol_problems_with_its_file_and_line`.
    pub(crate) problems: Vec<ProblemRow>,
    /// Whether the session is locked. A binding run while it is -- one that
    /// asked to be, with `locked` -- may not act on windows; see
    /// [`crate::wm::convert::reaches_past_the_lock`].
    pub(crate) locked: bool,
    /// Notifications as the service holds them, for `sol.notifications()`
    /// (#346). Shared, not copied, from one snapshot to the next.
    pub(crate) notifications: std::rc::Rc<crate::services::notifications::View>,
    /// Audio as the service holds it, for `sol.audio()` (#347).
    pub(crate) audio: std::rc::Rc<crate::services::audio::View>,
    /// The tray as the service holds it, for `sol.tray()` (#363).
    pub(crate) tray: std::rc::Rc<crate::services::tray::View>,
    /// The media players as the service holds them, and the time, for
    /// `sol.media()` (#379).
    pub(crate) media: crate::services::media::Seen,
}

/// One problem, as `sol.problems()` hands it over: an effect, a rule or the
/// configuration, where, and what. Rust publishes; `lua/problems.lua` shows
/// (`tests::a_broken_effect_reaches_sol_problems_with_its_file_and_line`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ProblemRow {
    pub(crate) effect: String,
    pub(crate) file: String,
    pub(crate) line: Option<u32>,
    pub(crate) column: Option<u32>,
    pub(crate) message: String,
    pub(crate) severity: &'static str,
}

/// What `sol.press` delivers to a window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Press {
    /// A pointer button, by its evdev code: `BTN_LEFT` and the four after it.
    Button(u32),
    /// A key combination, as the keysyms pressed in order: the modifiers
    /// first, the key last. Let go of in reverse.
    Keys(Vec<smithay::input::keyboard::Keysym>),
}

/// A key binding as the window manager declares it, and as `solium --check`
/// reports it: the protocol's record. The note is `None` for the shipped
/// bindings, which is why `--check` stays a plain list of combinations until
/// a configuration has something to add.
pub(crate) use solium_wm_protocol::Binding;

/// What a binding asked for besides its handler: the table form of
/// `sol.bind`'s third argument (#224). Both are off unless asked for, which is
/// what every binding was before there was a table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct BindingOptions {
    /// While its key is held, the handler runs again at the keyboard's repeat
    /// delay and rate. `input::tests::held`.
    pub(crate) repeating: bool,
    /// It also fires while the session is locked, where nothing it does may
    /// reach a window. `state::tests::real_client::lock_focus::held`.
    pub(crate) locked: bool,
    /// It also fires while a window holds the compositor's shortcuts (#58),
    /// which every other binding gives way to.
    /// `state::tests::real_client::lock_focus::shortcuts_inhibit`.
    pub(crate) inhibited: bool,
}

/// A setting a configuration wrote that nothing reads, and the nearest
/// key that does exist when one is close enough to name: the protocol's
/// record.
pub(crate) use solium_wm_protocol::UnknownSetting;

#[cfg(test)]
impl WindowInfo {
    /// An ordinary window for a test: shown, on `DP-1`, 800 by 600 at
    /// (100, 100), named after its id, with nothing else set.
    pub(crate) fn test(id: u64) -> Self {
        Self {
            id,
            rect: Rect {
                x: 100.0,
                y: 100.0,
                w: 800.0,
                h: 600.0,
            },
            insets: crate::decoration::Insets::default(),
            drawn: Drawn::default(),
            title: format!("window {id}"),
            focused: false,
            monitor: "DP-1".to_owned(),
            modal: false,
            parent: Parentage::None,
            leaving: false,
            minimized: false,
            above: false,
            icon: None,
            app_id: String::new(),
            min: None,
            max: None,
            cramped: false,
            shown: true,
            fullscreen: false,
            maximized: false,
            style: String::new(),
            client_side: false,
            blur_region: false,
            x11_type: None,
            accepts_input: true,
            class: None,
            instance: None,
            content_type: "none",
            pid: None,
        }
    }
}
