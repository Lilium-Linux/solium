//! Compositor state and the Wayland protocol handlers.
//!
//! Smithay hands each protocol a state object and a handler trait; this module
//! owns both. Layout and presentation deliberately do not live here — see
//! `docs/architecture.md`.

use std::time::Duration;

use smithay::output::{Output, Scale};
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{IsAlive as _, Logical, Point, Rectangle, SERIAL_COUNTER, Serial, Size};
use smithay::wayland::cursor_shape::CursorShapeManagerState;
use smithay::wayland::fractional_scale::{
    FractionalScaleHandler, FractionalScaleManagerState, with_fractional_scale,
};
use smithay::wayland::pointer_constraints::{
    PointerConstraintsHandler, PointerConstraintsState, with_pointer_constraint,
};
use smithay::wayland::presentation::PresentationState;
use smithay::wayland::relative_pointer::RelativePointerManagerState;
use smithay::wayland::viewporter::ViewporterState;
use smithay::wayland::xdg_activation::{
    XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
};

use smithay::{
    backend::{allocator::dmabuf::Dmabuf, renderer::utils::on_commit_buffer_handler},
    delegate_data_device, delegate_dmabuf, delegate_layer_shell, delegate_output, delegate_seat,
    delegate_shm, delegate_xdg_decoration, delegate_xdg_shell,
    desktop::{
        LayerSurface, PopupGrab, PopupKeyboardGrab, PopupKind, PopupManager, PopupPointerGrab,
        PopupUngrabStrategy, Space, Window, WindowSurfaceType, find_popup_root_surface,
        get_popup_toplevel_coords, layer_map_for_output, utils::under_from_surface_tree,
    },
    input::{
        Seat, SeatHandler, SeatState,
        pointer::{CursorIcon, CursorImageStatus, Focus, GrabStartData},
    },
    reexports::{
        wayland_protocols::xdg::{
            decoration::zv1::server::zxdg_toplevel_decoration_v1,
            shell::server::{xdg_toplevel, xdg_toplevel::ResizeEdge},
        },
        wayland_server::{
            Client, DisplayHandle,
            protocol::{wl_data_source::WlDataSource, wl_seat::WlSeat, wl_surface::WlSurface},
        },
    },
    wayland::seat::WaylandFocus,
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState, get_parent,
            is_sync_subsurface, with_states,
        },
        dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
        output::{OutputHandler, OutputManagerState},
        selection::data_device::{
            ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
            clear_data_device_selection, request_data_device_client_selection,
            set_data_device_selection,
        },
        selection::primary_selection::{
            PrimarySelectionHandler, PrimarySelectionState, clear_primary_selection,
            request_primary_client_selection, set_primary_selection,
        },
        selection::{SelectionHandler, SelectionSource, SelectionTarget},
        session_lock::SessionLockManagerState,
        shell::{
            wlr_layer::{
                Layer, LayerSurface as WlrLayerSurface, LayerSurfaceConfigure, LayerSurfaceData,
                WlrLayerShellHandler, WlrLayerShellState,
            },
            xdg::{
                PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
                XdgToplevelSurfaceData,
                decoration::{XdgDecorationHandler, XdgDecorationState},
                dialog::{XdgDialogHandler, XdgDialogState},
            },
        },
        shm::{ShmHandler, ShmState},
    },
};
use zxdg_toplevel_decoration_v1::Mode;

use crate::{
    decoration::{Action, Decorations, Insets, TITLEBAR_HEIGHT},
    input::{grab::MoveGrab, profile::Profile, resize},
    layer, monitor,
    pane::Pane,
    present::{self, Clock, Frame},
    script::{
        AnimationSpec, Command, Drawn, Outcome, Parentage, Rect, Scripts, Snapshot, WindowInfo,
    },
};

mod handlers;
mod monitors;
mod snapshot;
mod x11;

#[cfg(test)]
use handlers::{may_grab, popup_target};
#[cfg(test)]
use monitors::anywhere_on;
#[cfg(test)]
use snapshot::to_rect;

/// How far past the present a question about where a window is *going* looks.
///
/// **Focus judges where a window is settling; the pointer judges where it is
/// drawn.** That is the rule, and this constant is the line between its two
/// halves. A click lands on what is on screen this frame, so the hit test —
/// [`Solium::window_under`], through `Frame::covers` — samples the present
/// and must go on doing so. Focus is a decision about what the user is about
/// to work with, so it belongs to the destination: [`Solium::settle_focus`]
/// and [`Solium::everything_is_off_stage`] both ask [`Solium::drawn_at`] this
/// far ahead, through [`Solium::settling`]. Where the two meet —
/// `settle_focus` asking which window is under the pointer — it is still a
/// focus decision, and it hit-tests the destination.
///
/// The cases, each pinned by the test named:
///
/// * a closing window is headed for `present::close`'s opacity zero, so it is
///   no candidate from the press onwards, even while it is still visibly
///   fading and still takes clicks —
///   `a_window_mid_close_under_the_pointer_is_not_handed_the_keyboard`;
/// * a window being given back is headed for full opacity, so it is a
///   candidate on the frame its restore starts, when it is still drawn at
///   nothing — `a_refused_close_gives_the_keyboard_back_to_the_window_it_brings_back`;
/// * a desk just switched to is headed on stage and a desk just left is headed
///   off, on the switch's first frame, by either arm —
///   `the_first_frame_of_a_workspace_switch_focuses_the_desk_switched_to` and
///   `a_pointer_over_the_desk_being_left_does_not_hand_it_the_keyboard`, which
///   also asserts the click on the same pixel on the same frame still goes to
///   what is drawn.
///
/// A reload *starts* the workspace slide; it does not finish it. Asking where
/// the windows are at that instant asks where they were before it, so the
/// question has to be put to a moment when the transforms have landed.
///
/// Only has to be longer than the longest animation a configuration can name,
/// and costs nothing for being longer than that: `Animation::progress` clamps
/// the elapsed time to the duration, so every extra second is the same
/// division. An hour is not a guess at how long an animation takes — it is far
/// enough that it cannot be one.
const SETTLED: Duration = Duration::from_secs(60 * 60);

/// Whether none of these windows is drawn on any screen: each a slot, where
/// the window lives, and the rectangle it is drawn at.
///
/// **A drawn rectangle counts only on a screen its slot is on**, because that
/// is the only kind of screen that draws it: [`crate::render::drawn_on`], the
/// renderer's own cull. Until #134's third review this asked whether the
/// drawn rectangle reached *any* screen, and with two monitors side by side
/// the left one's workspace next door is carried onto the right one -- which
/// never draws it -- so its windows came back on stage.
/// `a_window_is_on_stage_only_on_a_monitor_that_draws_it` pins the rule, and
/// `on_two_monitors_a_window_on_the_left_monitors_hidden_workspace_is_not_on_stage`
/// the shipped workspaces meeting it.
///
/// `None` when there is nothing to ask about — no screens, or no windows. The
/// free function over plain rectangles, for the same reason [`anywhere_on`] is
/// one: `Solium` needs a `Display` and cannot be built in a unit test, so the
/// half that decides is the half kept testable. The instant the rectangles were
/// measured at is the caller's, and is the other half — see
/// [`Solium::everything_is_off_stage`], which got it wrong.
fn nothing_on_stage(
    drawn: impl IntoIterator<Item = (Rectangle<i32, Logical>, Rectangle<f64, Logical>)>,
    screens: &[Rectangle<i32, Logical>],
) -> Option<bool> {
    if screens.is_empty() {
        return None;
    }
    let mut any = false;
    for (slot, rect) in drawn {
        any = true;
        if screens
            .iter()
            .any(|screen| crate::render::drawn_on(slot, *screen) && screen.to_f64().overlaps(rect))
        {
            return Some(false);
        }
    }
    any.then_some(true)
}

/// [`Solium::on_stage`]'s rule, for a window living at `slot` and drawn as
/// `frame`: it paints something, on a screen that draws it.
fn staged(
    slot: Rectangle<i32, Logical>,
    frame: Frame,
    screens: &[Rectangle<i32, Logical>],
) -> bool {
    frame.shows() && nothing_on_stage([(slot, frame.rect)], screens) != Some(true)
}

/// Whether `point` is somewhere a window living at `slot` could be drawn: on a
/// screen that draws it, by [`crate::render::drawn_on`].
///
/// **The hit tests' half of the rule [`nothing_on_stage`] is the focus half
/// of**, and asked by every walk that decides where a press goes --
/// `window_under`, `surface_under`, `pane_chrome` -- and by `decorated_under`,
/// which only looks. `Frame::covers` says whether the point is inside what a
/// pane draws; this says whether the screen under the point draws the pane at
/// all. With two monitors side by side the left one's hidden workspace is
/// carried over the right one, where its rectangle covers pixels the right
/// monitor never drew it on, and a press there went into a window nobody could
/// see (#134's third review).
/// `on_two_monitors_a_press_on_the_right_monitor_reaches_what_it_draws`.
///
/// **No screens is not "nowhere"**, for the reason [`Solium::on_stage`] gives:
/// a pointer with no monitor to be on is not a reason to decide that nothing
/// is under it. A point on no screen while there are some is on nothing,
/// because nothing is drawn there -- and `input::confine` keeps the pointer
/// from ever being there. `a_point_is_on_a_window_only_on_a_monitor_that_draws_it`.
///
/// **And never for `remains`: what is left of a window whose client has gone**
/// (#126), `Pane::ghost`. It is drawn, fading where the window stood, but it is
/// a picture of a window and not one: there is no client to hand a press, a
/// key or a hover to, and what it is drawn over is the window the layout has
/// grown into its place (`crate::pane::Left::over`). So every walk looks
/// through it, and it is asked here, once, rather than by each walk beside
/// this rule. `a_window_that_left_is_nobodys_to_find` asks the walks,
/// `a_point_is_on_a_window_only_on_a_monitor_that_draws_it` this.
fn shown_at(
    slot: Rectangle<i32, Logical>,
    remains: bool,
    point: Point<f64, Logical>,
    screens: &[Rectangle<i32, Logical>],
) -> bool {
    !remains
        && (screens.is_empty()
            || screens.iter().any(|screen| {
                screen.to_f64().contains(point) && crate::render::drawn_on(slot, *screen)
            }))
}

/// Whether a window living at `slot`, drawn as `frame`, owns the pixel at
/// `point`: on a screen that draws it, [`shown_at`], and inside what it
/// paints there, `Frame::covers`.
///
/// **One question for "is this window under this point", whether Rust or a
/// script is asking.** [`Solium::window_under`] asks it of every pane in its walk,
/// and `sol.window_at` of every window in the snapshot, which carries the
/// slot, the frame and the screens for the purpose (`script::Drawn`). Until
/// #134's fourth review the script's half asked only whether the drawn
/// rectangle held the point, and so found what the Rust half had already
/// learned to see past: a window on a hidden workspace carried over the next
/// monitor, and a window fading out at opacity zero over the neighbour in its
/// place (#135).
/// `on_two_monitors_a_press_on_the_right_monitor_reaches_what_it_draws` asks
/// it from Rust, `on_two_monitors_sol_window_at_answers_what_the_right_monitor_draws`
/// and `sol_window_at_over_a_window_fading_out_answers_the_neighbour_in_its_place`
/// from Lua.
///
/// **One question, asked over two lists, and the lists differ.** The Rust walk
/// is every pane; the snapshot leaves out a pane that is not managed, one
/// whose application has not arrived when `loading.reserves_a_slot` is off,
/// and one scripts have been told has gone. Where the Rust walk stops at one
/// of those, `sol.window_at` sees through it to the window behind. Older than
/// #134 and left as it stands:
/// `sol_window_at_sees_through_the_panes_the_snapshot_leaves_out`. What is
/// left of a window whose client has gone is seen through by both: the
/// snapshot leaves it out, and `remains` has the Rust walk look past it
/// (`a_window_that_left_is_nobodys_to_find`).
pub(crate) fn owns(
    slot: Rectangle<i32, Logical>,
    remains: bool,
    frame: Frame,
    point: Point<f64, Logical>,
    screens: &[Rectangle<i32, Logical>],
) -> bool {
    shown_at(slot, remains, point, screens) && frame.covers(point)
}

/// Whether a selection headed for `bound` puts away the desk it carries:
/// carries it anywhere at all, or fades it to nothing as [`Frame::shows`]
/// counts nothing.
///
/// **Measured against no screen.** `workspaces.lua` parks a desk by its
/// monitor's work area times `spread`, not by the screen, and until #134's
/// sixth review this asked whether the screen, carried as the desk is headed,
/// was still on itself: at `spread = 1.0` a panel across the slide left every
/// parked desk overlapping its screen by the panel's thickness, and on stage.
/// `with_a_panel_across_a_horizontal_slide_a_genuine_activation_on_a_hidden_workspace_leaves_the_keyboard_exactly_where_it_was`,
/// `with_a_bar_across_a_vertical_slide_a_genuine_activation_on_a_hidden_workspace_leaves_the_keyboard_exactly_where_it_was`.
/// A selection that nudges a desk and leaves it on screen puts it away too.
///
/// [`Solium::carried_by_a_selection`]'s rule, over plain values for the reason
/// [`nothing_on_stage`] is one: `Solium` cannot be built in a unit test.
/// `a_desk_is_put_away_when_its_selection_is_headed_anywhere_or_to_nothing`.
fn put_away(bound: crate::group::Shift) -> bool {
    let (dx, dy) = bound.offset();
    dx != 0.0 || dy != 0.0 || !bound.apply(Frame::real(Rectangle::default())).shows()
}

/// A rectangle grown outward by the frame drawn around it.
fn grown(real: Rectangle<i32, Logical>, insets: Insets) -> Rectangle<i32, Logical> {
    if !insets.any() {
        return real;
    }
    Rectangle::new(
        (real.loc.x - insets.left, real.loc.y - insets.top).into(),
        (
            real.size.w + insets.horizontal(),
            real.size.h + insets.vertical(),
        )
            .into(),
    )
}

/// How much room a frame takes around its client.
///
/// The whole of what a [`crate::pane::Frame`] means to a layout, in one place
/// so that every reader of it agrees. The `Pending` arm is the interesting
/// one: a frame that has not been built *yet* still reserves what one will
/// want, because a window that changes shape the moment its frame appears is
/// worse than one that was always the right size. It must not apply to a pane
/// that will never have a frame — a client drawing its own decorations, an
/// override-redirect menu, or `pane = "none"` — which got a titlebar's
/// worth of blank space above it with no titlebar in it. That is what an
/// Electron application looked like here.
const fn insets_for(frame: &crate::pane::Frame) -> Insets {
    match frame {
        crate::pane::Frame::Pending => Insets {
            top: TITLEBAR_HEIGHT,
            ..Insets::NONE
        },
        crate::pane::Frame::None => Insets::NONE,
        crate::pane::Frame::Styled(decoration) => decoration.insets(),
    }
}

/// Per-client state Smithay asks us to store.
#[derive(Default, Debug)]
pub(crate) struct ClientState {
    pub(crate) compositor_state: CompositorClientState,
}
impl smithay::reexports::wayland_server::backend::ClientData for ClientState {}

#[derive(Debug)]
pub(crate) struct Solium {
    // Held for the lifetime of the compositor: these register Wayland globals
    // and dropping them would remove those globals. #12 reads `seat`.
    pub(crate) display_handle: DisplayHandle,

    pub(crate) compositor_state: CompositorState,
    pub(crate) xdg_shell_state: XdgShellState,
    /// The `xdg_wm_dialog_v1` global, so a client can say a toplevel is a modal
    /// dialog.
    ///
    /// Nothing ever reads this field, and it is not dead: the global lives for
    /// exactly as long as the `XdgDialogState` that created it, so dropping it
    /// would take `xdg_dialog_v1` off the registry and a client that had
    /// already bound it would be talking to nothing. That is the same contract
    /// every other `*_state` field above is held under -- see the comment at
    /// the top of this struct -- which is why it sits with them rather than
    /// being constructed and discarded in `new`.
    #[allow(dead_code)]
    pub(crate) xdg_dialog_state: XdgDialogState,
    pub(crate) shm_state: ShmState,
    #[allow(dead_code)]
    pub(crate) output_manager_state: OutputManagerState,
    pub(crate) seat_state: SeatState<Self>,
    pub(crate) data_device_state: DataDeviceState,

    /// The surface a client attached to the drag it started, while that drag
    /// lasts.
    ///
    /// **Issue #57: without this the drag is invisible.** Smithay offers the
    /// icon exactly once, as an argument to [`ClientDndGrabHandler::started`],
    /// and then keeps its own copy only so that it can drop it — see
    /// `selection/data_device/dnd_grab.rs`, where `self.icon = None` is the
    /// last thing `DnDGrab::drop` does and no accessor ever exposes it. A
    /// compositor that does not take it here has no route back to it.
    ///
    /// Nothing else notices it is missing: the offers are negotiated and the
    /// drop lands, so dragging between two windows *works*, silently, with
    /// nothing under the cursor for the whole gesture. Which reads as a drag
    /// that failed, and gets let go over the wrong window.
    ///
    /// `None` between drags. Read through [`Self::dnd_icon`] rather than
    /// directly: a client can die mid-drag and leave a dead surface here.
    dnd_icon: Option<WlSurface>,

    #[expect(
        dead_code,
        reason = "registers the xdg-decoration global; dropping it would remove it"
    )]
    pub(crate) xdg_decoration_state: XdgDecorationState,
    /// The shell's way in: bars, docks, wallpapers and notification areas are
    /// ordinary clients that anchor to an output edge. See `layer.rs`.
    pub(crate) layer_shell_state: WlrLayerShellState,
    /// Registers `ext_idle_notifier_v1`, and `zwp_idle_inhibit_manager_v1`
    /// beside it. See `idle.rs` for why those two are one feature.
    #[expect(dead_code, reason = "holds the global; dropping it would remove it")]
    pub(crate) idle_state: crate::idle::IdleState,
    #[expect(dead_code, reason = "holds the global; dropping it would remove it")]
    pub(crate) idle_inhibit_state: smithay::wayland::idle_inhibit::IdleInhibitManagerState,
    /// Who is waiting to be told nobody is here, and who is stopping us
    /// deciding that.
    pub(crate) idle: crate::idle::Idle,

    /// Everything a script has asked the compositor to draw in QML.
    ///
    /// The wallpaper is one of these and there is nothing in here that knows
    /// that. See `scripted.rs`.
    pub(crate) surfaces: crate::scripted::Surfaces,

    /// Every selection a script has named, and where each is being carried.
    ///
    /// **Not a sixth table keyed by `PaneId`.** A group holds its own members
    /// and its own transform, so nothing here is keyed by anything and there is
    /// nothing to reconcile: a selection naming a window that has closed
    /// resolves to nothing and costs a `u64`. See `group.rs`.
    pub(crate) groups: crate::group::Groups,

    /// The keymap in force, kept so a reload that changes nothing does not
    /// re-send one. See `keymap.rs`.
    pub(crate) keymap: Option<crate::keymap::Keymap>,
    /// What the keyboard currently is, cached rather than read.
    ///
    /// Reading it means locking xkb and walking the keymap, and `snapshot` is
    /// built for every script event -- every window move, every focus change.
    /// Refreshed where it changes instead, which is one place.
    pub(crate) keyboard: crate::keymap::State,

    /// Registers `ext_session_lock_manager_v1`: the lock screen. See `lock.rs`.
    pub(crate) session_lock_state: SessionLockManagerState,
    /// The set of monitors the backend is driving may no longer be right.
    ///
    /// Set when a configuration reload changes which monitors are enabled, and
    /// read by the hardware backend, which is the only one with connectors to
    /// look at. A flag rather than a `Request` because it is not a thing the
    /// session does -- it is a thing the session notices.
    pub(crate) rescan_outputs: bool,

    /// Set while the session is locked, and the single thing every other part
    /// of the compositor checks before it draws or delivers anything.
    ///
    /// `Some` means locked, whether or not the client has managed to put
    /// anything on screen -- see `lock.rs` for why that asymmetry is the
    /// safe one.
    pub(crate) lock: Option<crate::lock::Lock>,

    pub(crate) space: Space<Window>,

    /// Registers `zwlr_screencopy_manager_v1`, which is screenshots, screen
    /// recording and screen sharing. See `screencopy.rs`.
    #[expect(dead_code, reason = "holds the global; dropping it would remove it")]
    pub(crate) screencopy_state: crate::screencopy::ScreencopyState,
    /// Captures a client has asked for and not yet been given.
    ///
    /// Drained by whichever backend is running, because reading pixels needs a
    /// renderer and that is the one place there is one.
    pub(crate) pending_captures: Vec<crate::screencopy::Capture>,

    /// Where the monitors are, relative to each other. See `monitor.rs`.
    ///
    /// Empty until a script says otherwise, which means "left to right in
    /// connector order" — right about half the time, and wrong in a way that
    /// is obvious and one line to fix.
    pub(crate) arrangement: monitor::Arrangement,

    /// What the compositor thinks its windows are. See `pane.rs`.
    ///
    /// `space` is still underneath and still the authority on stacking and
    /// damage for a mapped client. This is the view everything else asks:
    /// which window a script means, what is drawn, what the pointer is over.
    /// `sync_panes` is the one place the two are reconciled.
    pub(crate) panes: crate::pane::Panes,

    pub(crate) popups: PopupManager,
    /// The menu chain holding the seat's grabs, while one does. Kept so that
    /// locking can close it rather than leave it steering the keyboard: see
    /// `Solium::release_grabs` in `focus.rs`.
    pub(crate) popup_grab: Option<PopupGrab<Self>>,
    pub(crate) seat: Seat<Self>,

    /// The one animation clock. Ticked by the render loop, read by everything.
    pub(crate) clock: Clock,

    /// Per-form-factor input behaviour.
    pub(crate) profile: Profile,

    /// The Lua runtime. Modes live in here, not in the compositor.
    pub(crate) scripts: Option<Scripts>,

    /// The active mode's name, as a script reported it. The compositor does
    /// not know what modes exist — it only knows what to put in the bar.
    pub(crate) status: String,

    /// Whether a mode owns input. While it does, keys and clicks belong to the
    /// script rather than to clients.
    pub(crate) script_grab: bool,

    /// The keys whose press the keyboard filter forwarded to a client, and
    /// whose release has not come yet.
    ///
    /// What decides where a release goes. A release has to reach whoever was
    /// given the press, or that client holds the key down for ever and repeats
    /// it; and it must not reach anyone who was not, when a mode swallowed the
    /// press. Asking "is a mode active *now*" instead got both wrong across a
    /// change: a mode active when the session locked kept intercepting releases
    /// the lock screen had been given the presses of, so every key of the
    /// password auto-repeated. Smithay keeps the same set, privately.
    pub(crate) keys_forwarded: std::collections::HashSet<u32>,

    /// The session has just unlocked with a key still held, and the keyboard
    /// goes back to a window once it is let go. See `Solium::unlock`.
    pub(crate) refocus_on_release: bool,

    /// The socket clients connect on. Held so that a program started from a
    /// script finds *this* compositor rather than the session it is nested in.
    pub(crate) socket_name: String,
    /// What a window does between being asked for and its application
    /// arriving — what draws it, how long it waits, whether it takes a place
    /// in the layout. A script's, not the compositor's. See `Command::Loading`.
    pub(crate) loading: crate::script::Loading,
    /// Which frame the pointer was last over, so the one it leaves can be
    /// told. QML hover is positional: a frame never told the pointer left
    /// stays lit forever.
    pub(crate) hovered_frame: Option<crate::pane::PaneId>,
    // A window on its way out, and one that has been asked to close and not
    // gone, used to be two `HashMap<PaneId, Duration>` here. They are
    // `Pane::closing_at` and `Pane::asked_at` now: a timer about one window is
    // part of that window, and leaves with it rather than waiting to be swept
    // out of a table beside it. See `close_pane` and `settle_refused`.
    /// When the last memory report went out; see `memory_report`.
    pub(crate) reported_at: std::time::Duration,
    /// XWayland's window manager, once XWayland has started. `None` means no
    /// X11 support this session, which is a working session with fewer apps.
    pub(crate) xwm: Option<smithay::xwayland::X11Wm>,
    /// The X display number XWayland took, for `DISPLAY` in children.
    pub(crate) x11_display: Option<u32>,
    pub(crate) xwayland_shell_state: smithay::wayland::xwayland_shell::XWaylandShellState,
    /// Raw pointer motion, for anything that reads movement rather than
    /// position.
    ///
    /// A game reading the mouse to turn a camera cannot use `wl_pointer`: that
    /// reports where the pointer *is*, and the pointer stops at the edge of the
    /// screen. Without this a first-person game does not turn badly, it does
    /// not turn at all.
    #[expect(
        dead_code,
        reason = "registers zwp_relative_pointer_v1; dropping it would remove the global"
    )]
    pub(crate) relative_pointer_state: RelativePointerManagerState,
    /// Locking and confining the pointer to a surface.
    ///
    /// The other half of the same problem. Reading raw movement is no use while
    /// the pointer is still crossing the screen and leaving the window — a game
    /// wants it held still, a drawing application wants it kept inside a
    /// region.
    #[expect(
        dead_code,
        reason = "registers zwp_pointer_constraints_v1; dropping it would remove the global"
    )]
    pub(crate) pointer_constraints_state: PointerConstraintsState,
    /// Where a locked pointer's client would like the cursor left when the
    /// lock ends. Advice, taken at unlock. See `cursor_position_hint`.
    pub(crate) constraint_hint: Option<Point<f64, Logical>>,

    /// Letting a client *name* the cursor it wants instead of drawing one.
    ///
    /// Without it a toolkit has to rasterise every cursor itself and hand over
    /// pixels, and the ones that no longer do — which is the modern default,
    /// because naming a shape is how a client gets the compositor's theme
    /// rather than a guess at it — got no cursor change at all. Not a subtle
    /// failure: a text field showed the arrow, a resize edge showed the arrow,
    /// everything showed the arrow. The name is resolved through the same
    /// XCursor theme the rest of the pointer uses; see `cursor::shape`.
    #[expect(
        dead_code,
        reason = "registers wp_cursor_shape_manager_v1; dropping it would remove the global"
    )]
    pub(crate) cursor_shape_state: CursorShapeManagerState,

    /// Cropping and scaling a surface without the client redrawing it.
    ///
    /// How a video player presents a frame decoded at one size at another size
    /// — and how anything that scales does it without paying for a resize. The
    /// renderer already honours a viewport once the global exists; what was
    /// missing was the global, so every client fell back to redrawing at the
    /// size it wanted.
    #[expect(
        dead_code,
        reason = "registers wp_viewporter; dropping it would remove the global"
    )]
    pub(crate) viewporter_state: ViewporterState,
    /// Buffers that are one colour rather than a grid of pixels.
    ///
    /// A client that wants a solid rectangle names the colour and stretches
    /// the result with a viewport, instead of filling shared memory with one
    /// value. Smithay draws such a buffer without uploading it; see
    /// `single_pixel.rs` for what was checked and how.
    #[expect(
        dead_code,
        reason = "registers wp_single_pixel_buffer_manager_v1; dropping it would remove the global"
    )]
    pub(crate) single_pixel_buffer_state:
        smithay::wayland::single_pixel_buffer::SinglePixelBufferState,
    /// Telling a surface what scale it is really being drawn at.
    ///
    /// Without it a client has only the integer scale from `wl_output`, so on
    /// anything that is not a whole number it picks the next one up and is
    /// scaled back down — which is the blurry-on-a-150%-display problem. Solium
    /// is 1x everywhere today and this reports exactly that; the value is that
    /// it stops being a guess.
    #[expect(
        dead_code,
        reason = "registers wp_fractional_scale_manager_v1; dropping it would remove the global"
    )]
    pub(crate) fractional_scale_state: FractionalScaleManagerState,

    /// Handing focus from whoever launched an application to the application.
    ///
    /// A token is minted by the launcher, travels to the launched program in
    /// its environment, and comes back when that program has a window. Two
    /// things fall out of it: an application can raise itself without any
    /// window being able to steal focus by simply asking, and the compositor
    /// can recognise the window it opened for a program *whatever* the program
    /// did to its own processes on the way — which is the one thing walking up
    /// from a pid cannot do.
    pub(crate) activation_state: XdgActivationState,

    /// Telling a client when its frame actually reached the screen.
    ///
    /// A client that has to guess when its work was shown guesses wrong, and
    /// the way that looks is video that judders on a screen fast enough to have
    /// shown it smoothly. `wp_presentation` hands over the real timestamp, the
    /// refresh interval and a sequence number, so a player can pace itself
    /// against the display instead of against a timer.
    #[expect(
        dead_code,
        reason = "registers wp_presentation; dropping it would remove the global"
    )]
    pub(crate) presentation_state: PresentationState,

    /// A selection an X11 client owns that a Wayland client has asked to read.
    ///
    /// Recorded rather than served, for the same reason the resize and the drop
    /// are: pumping an X11 transfer needs the event loop, and only a backend
    /// has one — the two backends have loops over different state types, so
    /// there is no one handle this could hold. See `settle_selection`.
    pub(crate) pending_selection: Option<(SelectionTarget, String, std::os::fd::OwnedFd)>,

    /// The middle-click clipboard. A separate selection with its own protocol,
    /// and its absence is not subtle: a terminal that pastes on middle click
    /// pastes nothing at all.
    pub(crate) primary_selection_state: PrimarySelectionState,

    /// How windows are framed: which QML draws a frame, and the building of
    /// one. The frames themselves are on the panes they are drawn around.
    pub(crate) decorations: Decorations,

    /// What the pointer should look like right now.
    ///
    /// Nested, the host compositor drew the cursor and this could be ignored.
    /// On the hardware nothing else will draw it, so a compositor that does not
    /// track this has an invisible pointer — which is indistinguishable, to
    /// whoever is sitting there, from input being broken.
    pub(crate) pointer: crate::cursor::Pointer,

    /// Fragment programs, compiled on first use and kept for the life of the
    /// renderer.
    ///
    /// Not on the renderer, because that one is Smithay's; not on the pane,
    /// because a program belongs to a GL context and there is one of those.
    /// Beside `pointer` rather than among the protocol states for the same
    /// reason `pointer` is here: it is something the compositor draws *with*,
    /// not something a client binds.
    pub(crate) programs: crate::pass::Programs,

    /// Hardware buffer sharing: `zwp_linux_dmabuf_v1`.
    ///
    /// The global itself is created by whichever backend has a renderer, since
    /// only it knows which formats the GPU can actually scan out. Held here so
    /// dropping it — which would withdraw the global — happens with the rest.
    pub(crate) dmabuf_state: DmabufState,
    pub(crate) dmabuf_global: Option<DmabufGlobal>,

    /// The renderer context a departing window's picture is read under.
    ///
    /// `RendererSurfaceState` files each imported texture by the context it
    /// was imported under, and the backend owns the renderer, so the backend
    /// hands its context over when it has one. `None` until then, and in a
    /// test with no renderer, where a window that goes leaves a picture with
    /// no pixels in it. See `crate::remains`.
    pub(crate) textures: Option<crate::remains::Textures>,

    /// The last window list handed to the shell, so it is only sent again
    /// when it differs.
    published_windows: String,

    /// A shell surface, when one was asked for.
    ///
    /// The compositor ships no shell and invents none: `SOLIUM_SHELL_SCENE`
    /// names a QML file to host, and without it there is nothing here.

    /// Set while a focus change is being reported to scripts.
    ///
    /// A script handling `focus` will often ask for focus itself — a scroller
    /// focuses the window whose column it just brought into view — and without
    /// this that answer would be reported straight back to it, forever.
    focusing: bool,

    /// The pane a close is being reported for, while it is being reported.
    ///
    /// A closing window is still in `panes` for the length of that call, on
    /// purpose — a script is handed an id and has to be able to look it up. The
    /// one thing it must not still be is somebody's *parent*: see
    /// [`Self::parented`].
    closing: Option<crate::pane::PaneId>,

    /// A resize asked for by an edge drag, not yet applied.
    ///
    /// Offered to layouts first: in a tiled or scrolling arrangement a window
    /// does not have a size of its own to change — dragging its edge moves the
    /// seam it shares with its neighbour, or the width of its column. Only a
    /// floating window is resized directly.
    pub(crate) pending_resize: Option<ResizeRequest>,

    /// The window whose pane's slot is currently telling the truth about its
    /// size, because an edge drag is still going on. See [`crate::resizing`].
    ///
    /// **Not a fourth place a window's position lives** — issue #84 counts
    /// three already and adding one would be the wrong direction. A window's
    /// rectangle is the pane's slot, and this only says that the slot outranks
    /// the client for a while and when it stops doing so. The rectangle inside
    /// the hold is `resizing::Hold::asked`, which is a copy of the last
    /// configure rather than a window's geometry: nothing reads it to find out
    /// where a window is.
    pub(crate) resize_hold: Option<crate::resizing::Held>,

    /// The same, for the panes a *layout* moved because of an edge drag.
    ///
    /// [`Self::resize_hold`] is one slot because a floating drag resizes one
    /// window. A tiled drag resizes as many as the layout says: a seam is two
    /// panes, a corner drag is two seams, and `tiling.apply` is free to place
    /// every leaf on the monitor. Each of those panes has its own client with
    /// its own latency, its own refusal and its own moved edge — which is not
    /// the pointer's, because the neighbour across a seam has the opposite edge
    /// pulled; see [`crate::resizing::moved_edges`]. There is no way to express
    /// that in one hold, so this is one per pane.
    ///
    /// **Created only by a live pointer gesture** — see [`Self::resize_gesture`]
    /// — because `Self::release_resize` is the only thing that ever ends one and
    /// the pointer grab is the only thing that calls it.
    resize_bridge: Option<Bridged>,

    /// The edge drag a layout is being asked about right now.
    ///
    /// Set for the length of one `trigger_resize` and cleared after it, because
    /// that call runs `apply` itself: every `move_pane` a claiming layout causes
    /// happens *inside* it, which is the only window in which `move_pane` can
    /// tell "a layout is moving this pane because the user is dragging an edge"
    /// from the half-dozen other things that reach it — a keyboard nudge, a
    /// reload, a monitor change, a workspace switch, `rescue_offscreen`. None of
    /// those has a gesture to end a hold, so none of them may arm one.
    ///
    /// **It spans the whole of `trigger_resize`, including an `apply` whose
    /// outcome turns out to be unclaimed, and that is right rather than merely
    /// unavoidable.** It is unavoidable, because a layout's placements *are*
    /// commands and `apply` is what runs them: there is no seam inside the call
    /// at which "this handler's own layout" could be told from "a command this
    /// handler ran", so narrowing the window would mean arming nothing at all.
    /// It is also right, because a pane this dispatch moved is a pane this
    /// gesture moved whatever the handler said about claiming the *dragged*
    /// window: `handled` answers who decides that one window's rectangle, not
    /// whether the others need bridging, and they do — they were moved by the
    /// drag's own dispatch and `release_bridge` ends every one of them when the
    /// button comes up. An unclaimed frame hands the dragged pane back to the
    /// floating path and leaves the rest alone; see `settle_resize`.
    resize_gesture: Option<Gesture>,

    /// An edge drag whose button has come up, and when.
    ///
    /// **Recorded whether or not there is a hold to tell**, which is the point
    /// of it. A hold is not born when the gesture starts: `input::resize`'s
    /// `motion` records a request and `Self::hold_resize` turns it into a hold
    /// at the *frame*, so a press, a motion and a release inside one dispatch
    /// batch — a quick nudge of a border, which is well under sixteen
    /// milliseconds — all happen before any hold exists. `Self::release_resize`
    /// would then have nothing to write to, and the hold born a moment later
    /// would believe its gesture was still going, for ever.
    ///
    /// So the release is written down instead of dropped, and the hold born
    /// afterwards is born already released. Cleared at the end of any frame
    /// that leaves no hold behind: nothing else can consume it by then, and a
    /// `Window` kept here is a client kept alive.
    resize_ended: Option<(Window, Duration)>,

    /// What `config.lua` said about resizing. See [`crate::resizing::Fill`].
    pub(crate) resizing: crate::resizing::Settings,

    /// A drag that has finished and not yet been reported to scripts.
    ///
    /// Recorded inside the pointer grab and acted on after it, for exactly the
    /// reason the keyboard filter carries its bindings out rather than running
    /// them: a grab callback runs while the seat holds the pointer's lock, and
    /// anything that asks the seat where the pointer is — `snapshot` does —
    /// takes that lock again and never gets it. A deadlock here freezes a
    /// compositor holding DRM master, which from the other side of the screen
    /// is indistinguishable from the machine dying.
    pub(crate) pending_drop: Option<(Window, f64, f64)>,

    /// Set when anything on screen has changed and not yet been drawn.
    ///
    /// The render loop used to ask "is any window non-empty", which is true of
    /// every mapped window forever — so a still screen was redrawn sixty times
    /// a second for nothing. Damage is the honest question, and an idle
    /// compositor should cost nothing.
    pub(crate) redraw: bool,

    /// Whether anything is mid-animation and needs the next frame.
    ///
    /// Kept here rather than in a backend, because both backends need the same
    /// answer and the one that did not have it drew every loop iteration
    /// instead. Two backends with two drawing policies is two compositors: for
    /// months every animation was verified on the one that redrew
    /// unconditionally, which is precisely the one where a missing damage
    /// signal cannot be seen.
    pub(crate) animating: bool,

    /// Something only a backend can carry out: switching VT, or stopping.
    ///
    /// The input layer must not do either itself. It runs inside the keyboard
    /// filter, holding the seat's lock, and it is shared with the nested
    /// backend where neither action means anything.
    pub(crate) request: Option<Request>,
}

/// Every pane a layout has moved on behalf of one edge drag.
///
/// The tiled counterpart of [`Solium::resize_hold`]. One [`crate::resizing::Hold`]
/// per pane, because each has its own client to wait for and its own edge to
/// anchor against, and one `window` for the whole of it, because the gesture
/// that ends every one of them is the single pointer grab on that window —
/// `Solium::release_resize` is told which window let go, not which pane.
#[derive(Debug)]
struct Bridged {
    /// The window the pointer has hold of. Not necessarily a pane in `panes`
    /// below: a layout is free to move a window's neighbours and leave the
    /// dragged window exactly where it was, and `scrolling.lua` does.
    window: Window,
    /// One per pane the layout actually moved, in no particular order. Short —
    /// two for an ordinary seam, four for a corner — but not bounded: a layout
    /// that reflows a whole monitor gets a hold for every pane whose rectangle
    /// changed, which is what a window pushed aside by someone else's drag
    /// needs in order to be told its own new size.
    panes: Vec<crate::resizing::Held>,
}

/// The edge drag a layout is being asked about, while it is being asked.
///
/// See [`Solium::resize_gesture`]. Carries the release the same way
/// [`Solium::hold_resize`] reads it, so a press, a motion and a release inside
/// one dispatch batch build a tiled bridge that is born already released rather
/// than one that waits for a button that has already come up.
///
/// **Which window is being dragged is [`Bridged::window`]'s and not repeated
/// here.** `arm_resize_gesture` makes the two agree before the layout is asked
/// and is the only thing that sets either, so a second copy could only ever
/// disagree.
#[derive(Clone, Copy, Debug)]
struct Gesture {
    /// When the button came up, if it already has. `Hold::new`'s contract.
    released: Option<Duration>,
}

/// An edge drag in progress.
///
/// Carries *positions* rather than deltas, on both of its paths. A layout sets
/// its seam from a position directly, so dragging to the same place twice gives
/// the same result; feeding it deltas fed the layout's own response back in as
/// the next input.
///
/// The positions are relative to the grab all the same, and since #124 both of
/// them are: each is a rectangle frozen at the grab plus the drag's *total*
/// pointer movement. "Absolute or relative" and "a position or a delta" are two
/// different questions, and conflating them is how the tiled path came to send
/// the pointer's own coordinate for a year.
///
/// **The two are built from two different rectangles, and that is deliberate.**
/// [`Self::wanted`] starts from `Solium::pane_outer` — where the window is,
/// which is the client's own rectangle and the right one to resize a floating
/// window by. [`Self::edge_at`] starts from `Solium::pane_laid_out` — where the
/// layout put the pane, which is the only rectangle a layout will recognise
/// when it comes back. Deriving the second from the first is the #124 review's
/// finding; see [`crate::input::resize::dragged_edge`].
#[derive(Clone, Debug)]
pub(crate) struct ResizeRequest {
    pub(crate) window: Window,
    /// Where a floating window would be put, for when no layout claims it.
    pub(crate) wanted: Rectangle<i32, Logical>,
    /// Where the dragged edge should come to rest, per axis, in the layout's
    /// **outer** coordinate space.
    ///
    /// The layout's own edge for this pane, frozen at the grab, plus the total
    /// pointer movement — so it starts on a number the layout produced and
    /// moves with the pointer rather than jumping to it. Deliberately *not*
    /// read off [`Self::wanted`], which is in the client's space and already
    /// floored at a window's minimum size. See
    /// [`crate::input::resize::dragged_edge`] for the whole of the reasoning,
    /// including which space this is in and why that has to be the space
    /// `solium_layout::tree::Tiling::node_box` measures in.
    pub(crate) edge_at: (f64, f64),
    /// Which edges the pointer has hold of.
    ///
    /// The whole truth about the gesture, and since #120 the whole of it
    /// reaches the layout too. This used to be accompanied by a `horizontal`
    /// and a `vertical` boolean — what a *script* was handed — justified by
    /// the claim that an axis pair "is all a layout needs: a seam moves or it
    /// does not". That is false, and it was false in a way that was written
    /// down as a reason. **Which seam moves depends on which side was
    /// grabbed.** A window that is the right-hand child of a vertical split
    /// has that split's seam on its left; told only "horizontal", the layout
    /// moved that seam for a drag on the window's *right* edge, so the left
    /// edge jumped and the edge under the pointer did not move at all.
    ///
    /// The compositor needed the side already, for its own reason — a client
    /// that refuses the size it was offered has to give the pixels back on the
    /// edge being dragged, and "horizontal" cannot say which of the two that
    /// is; see `crate::resizing::Hold::anchored`. It simply threw the answer
    /// away on the way to the script. [`crate::input::resize::sides`] is the
    /// conversion that no longer does.
    pub(crate) edges: ResizeEdge,
}

/// A request from the input layer that only the backend can honour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Request {
    /// Switch to this virtual terminal.
    ///
    /// The kernel stops acting on Ctrl+Alt+F-keys once libseat puts the VT into
    /// graphics mode, so a compositor that does not do this itself is a trap:
    /// the only way out of it is the power button.
    Vt(i32),
    /// Stop the compositor and give the session back.
    Quit,
    /// Read the configuration again, without ending the session.
    ///
    /// Configuration you have to restart to try is configuration people stop
    /// changing, so this exists for the same reason the shell reloads on edit.
    Reload,
}

/// A process and the processes that started it, up to a few generations.
///
/// The program the compositor spawns is not always the one that connects: a
/// flatpak, a shell wrapper or a launcher forks and the client is a
/// grandchild. Walking up from the client finds the launch that started it
/// anyway. Bounded because this runs when a window appears and `/proc` is not
/// free, and because a chain longer than this is not a launch we started.
fn ancestry(pid: u32) -> Vec<u32> {
    const GENERATIONS: usize = 8;
    let mut family = Vec::with_capacity(GENERATIONS);
    let mut current = pid;
    for _ in 0..GENERATIONS {
        family.push(current);
        // Field 4 of /proc/<pid>/stat is the parent. The command name in
        // field 2 may contain spaces and parentheses, so the tail is taken
        // from the last ')' rather than by splitting the whole line.
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{current}/stat")) else {
            break;
        };
        let Some(tail) = stat.rsplit_once(')') else {
            break;
        };
        let Some(parent) = tail
            .1
            .split_whitespace()
            .nth(1)
            .and_then(|field| field.parse::<u32>().ok())
        else {
            break;
        };
        if parent <= 1 {
            break;
        }
        current = parent;
    }
    family
}

/// What the scripts did with a window opening, as [`Solium::trigger_open`]
/// reports it.
#[derive(Clone, Copy, Debug, Default)]
struct Opened {
    /// A script answered with commands of its own, so the built-in open
    /// animation is not wanted.
    handled: bool,
    /// A script asked for the keyboard to go somewhere -- a `sol.focus`
    /// among its commands. Where a new window's keyboard goes is then the
    /// script's decision and not [`Solium::offer_keyboard`]'s: see
    /// `a_script_that_moves_the_keyboard_at_open_has_the_last_word`.
    focused: bool,
}

/// What kind of client a window is. Named for [`first_focus`], which gives
/// both kinds the same answer, and on purpose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClientKind {
    /// An `xdg_toplevel`.
    Xdg,
    /// A managed XWayland window: Steam, a Wine game, anything X11.
    X11,
}

impl ClientKind {
    fn of(window: &Window) -> Option<Self> {
        Self::from_roles(window.toplevel().is_some(), window.x11_surface().is_some())
    }

    /// The kind a window with these roles is, apart from [`Self::of`] so it
    /// can be asked about an X11 window, which a test cannot make. `None` for a
    /// window with neither role, which smithay's `Window` cannot be today.
    /// `a_managed_x11_window_is_offered_the_keyboard_as_an_xdg_one_is`.
    const fn from_roles(xdg: bool, x11: bool) -> Option<Self> {
        if xdg {
            Some(Self::Xdg)
        } else if x11 {
            Some(Self::X11)
        } else {
            None
        }
    }
}

/// How a window is given the keyboard as it is first shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FirstFocus {
    /// Through [`Solium::focus_window`], as a click or `sol.focus` would be:
    /// raised, handed the keyboard through the gate, activated in X11's own
    /// terms, and announced to the scripts as `focus`.
    Focus,
    /// Not at all: the keyboard stays where it was.
    Stay,
}

/// The rule [`Solium::offer_keyboard`] applies, apart so that it can be asked
/// about an X11 window, which a test cannot make: an `X11Surface` needs a live
/// XWayland.
///
/// **Both kinds, and the same answer for both.** Until #134's review an X11
/// window got the keyboard as it opened only because `scrolling.lua` focused
/// every window but a dialog that opened, whether or not it was in charge.
/// Nobody had decided that; `new_toplevel`, which gave an xdg window the
/// keyboard, never sees an X11 one. When that stray call was stopped, Steam and
/// every Wine game opened with the keyboard left on the window before. Both kinds are named in the match so
/// that leaving one out again is an edit to this function and to
/// `a_managed_x11_window_is_offered_the_keyboard_as_an_xdg_one_is`, rather than
/// a line somewhere that forgets.
///
/// Only a managed window: an unmanaged one is a menu, a tooltip or a splash,
/// placed by its client (`xwayland.rs`'s `places_itself`). And only one headed
/// somewhere the user can see, which is [`Solium::on_stage`].
///
/// **The kind is an `Option`, so that the whole decision is here.**
/// `offer_keyboard` used to return early when [`ClientKind::of`] answered
/// nothing, and in #134's first round that early return was the line --
/// `toplevel().is_none()` -- that left every X11 window without the keyboard.
/// A kind nobody knows is `Stay` here instead, and `offer_keyboard` asks this
/// once with no return of its own:
/// `a_managed_x11_window_is_offered_the_keyboard_as_an_xdg_one_is` pins the
/// rule, [`ClientKind::from_roles`] and that shape.
const fn first_focus(kind: Option<ClientKind>, managed: bool, on_stage: bool) -> FirstFocus {
    match kind {
        Some(ClientKind::Xdg | ClientKind::X11) => {
            if managed && on_stage {
                FirstFocus::Focus
            } else {
                FirstFocus::Stay
            }
        }
        None => FirstFocus::Stay,
    }
}

/// Which region of the compositor's own chrome a point is in.
///
/// Carries no window and no pane on purpose: this is the part that is a
/// *rule* rather than a lookup, and [`Under`] is what carries the rest. See
/// [`Solium::chrome_under`] for what a press and the pointer each do with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Chrome {
    /// The frame's band — the titlebar, its buttons, and any border the
    /// decoration reserved. A press here is the frame's: a button, or a drag
    /// that moves the window.
    Frame,
    /// A resize border, and which edges a drag from it would pull.
    Resize(ResizeEdge),
}

impl Chrome {
    /// The cursor the compositor asserts over this region.
    ///
    /// Over a frame this is the plain arrow rather than a move cursor, and
    /// deliberately: a titlebar is not only a drag handle — it carries buttons
    /// that are pressed, not dragged — and every desktop shows the ordinary
    /// pointer over one. What matters for #108 is that it is the
    /// *compositor's* arrow and overrides whatever the client last set,
    /// because a client drawing its own resize affordance in the shadow margin
    /// under our titlebar is exactly the case that went wrong.
    pub(crate) fn cursor(self) -> CursorIcon {
        match self {
            Self::Frame => CursorIcon::Default,
            Self::Resize(edges) => resize::cursor(edges),
        }
    }
}

/// The compositor's own chrome under a point: which region, which pane, and
/// the two things a press there needs.
#[derive(Clone, Debug)]
pub(crate) struct Under {
    /// What is under the pointer, and therefore both what a press does and
    /// what the pointer is drawn as.
    pub(crate) chrome: Chrome,
    pub(crate) pane: crate::pane::PaneId,
    /// The pane's window, when its application has arrived. `None` is a frame
    /// around a window that is still loading, whose buttons work anyway.
    /// [`Solium::pane_chrome`] never reports a resize border without one.
    pub(crate) window: Option<Window>,
    /// The point in the pane's own coordinates, which is what a decoration
    /// hit-tests its buttons against.
    pub(crate) local: Point<f64, Logical>,
    /// The pane's outer rectangle, which a resize drag measures from.
    pub(crate) outer: Rectangle<i32, Logical>,
}

/// Which region a point belongs to, given what each of the two tests said
/// about it — and the only place the overlap between them is resolved.
///
/// **The overlap is real, not theoretical.** A window with a titlebar has a
/// band below its top edge that is inside the frame *and* within
/// [`resize::RESIZE_BORDER`] of the top edge, so both tests answer yes for the
/// same pixel. The frame takes it, for the plain reason that the frame is what
/// a press there does: `pointer_button` has always checked the frame before
/// the resize border, and nothing about the pointer's shape is allowed to
/// disagree with that. The top edge is still draggable from the outside half
/// of its border, which is over the desktop rather than over the titlebar, and
/// now says so.
///
/// Written as a function taking two booleans-worth of answer rather than as a
/// `match` inside [`Solium::pane_chrome`] so that the rule can be pinned: a
/// `Solium` needs a `Display` and cannot be built in a unit test, and a rule
/// that can only be exercised by running the compositor is a rule that goes
/// untested until somebody notices it on hardware. Which is how #108 was
/// found.
pub(crate) fn chrome_of(on_frame: bool, edges: ResizeEdge) -> Option<Chrome> {
    if on_frame {
        return Some(Chrome::Frame);
    }
    match edges {
        ResizeEdge::None => None,
        edges => Some(Chrome::Resize(edges)),
    }
}

/// What one pane makes of a point — which is a wider question than whether
/// that pane's chrome is under it.
///
/// **[`Self::Client`] is the answer that was missing, and it is the whole of
/// issue #111.** A hit test that only ever says "my chrome, or nothing"
/// cannot express *occlusion*: a pane whose client covers the point answers
/// the same "nothing" as a pane the point falls nowhere near, so a walk down
/// the stack carries on past a window that is plainly on top and hands the
/// point to whatever is underneath. What that looked like in use: two
/// overlapping windows, a press on the upper one's client where the lower
/// one's titlebar happened to lie beneath, and the *lower* window raised and
/// took focus. A titlebar took clicks through the window covering it.
///
/// **[`Self::Halo`] is the answer the first fix for #111 was missing.** A
/// pane's chrome and the pixels it paints are not the same region: a resize
/// border reaches [`resize::RESIZE_BORDER`] pixels *outside* the window, over
/// whatever is drawn behind it. A claim made out there is not backed by
/// anything the pane draws, so it cannot be settled by stacking order — see
/// [`topmost_chrome`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PaneHit<T> {
    /// This pane's own chrome is under the point *and* this pane draws there.
    /// The walk has its answer.
    Chrome(T),
    /// This pane's chrome is under the point but the pane draws nothing there:
    /// the outside half of a resize border, hanging over whatever is behind.
    /// A claim, but the weakest one — see [`topmost_chrome`].
    Halo(T),
    /// The point is inside what this pane draws, but on its client rather than
    /// on its chrome. The client owns it and the walk stops here with nothing:
    /// everything below this pane is covered at that point.
    Client,
    /// The point is not this pane's at all. Keep descending.
    Miss,
}

impl<T> PaneHit<T> {
    /// Carry a chrome answer into whatever the caller wanted to say about it,
    /// leaving the two stopping answers alone.
    fn map<U>(self, chrome: impl FnOnce(T) -> U) -> PaneHit<U> {
        match self {
            Self::Chrome(found) => PaneHit::Chrome(chrome(found)),
            Self::Halo(found) => PaneHit::Halo(chrome(found)),
            Self::Client => PaneHit::Client,
            Self::Miss => PaneHit::Miss,
        }
    }
}

/// One pane's complete answer about a point, from the two things that decide
/// it.
///
/// `covers` is whether the point is inside the pane's *drawn* rectangle, and
/// it never suppresses a chrome claim — the frame band and the resize border
/// are both settled by [`chrome_of`] first, and `covers` only grades the
/// claim that came out. That is what keeps the inside half of a resize border
/// working: it lies within the drawn rect, so a rule that answered
/// [`PaneHit::Client`] wherever `covers` held would swallow it and leave every
/// window resizable only from outside.
///
/// **The four answers are the two questions crossed, and the cross is the
/// point.** A pane can claim chrome while `covers` is false — a border reaches
/// [`resize::RESIZE_BORDER`] pixels outside the window — and that claim is a
/// [`PaneHit::Halo`] rather than a [`PaneHit::Chrome`] precisely because the
/// pane paints nothing there to back it up. Collapsing the two, which is what
/// taking `(Some(chrome), _)` did, hands a window's empty margin authority over
/// pixels another window is visibly drawing.
pub(crate) fn pane_hit_of(chrome: Option<Chrome>, covers: bool) -> PaneHit<Chrome> {
    match (chrome, covers) {
        (Some(chrome), true) => PaneHit::Chrome(chrome),
        (Some(chrome), false) => PaneHit::Halo(chrome),
        (None, true) => PaneHit::Client,
        (None, false) => PaneHit::Miss,
    }
}

/// Which of its chrome a pane is allowed to offer at all, before any point is
/// considered.
///
/// Three gates, and all of them are about what a press there could actually
/// *do*:
///
/// - **`shows`.** A pane drawn at opacity zero has no titlebar to press and no
///   edge to drag, because it has nothing on screen at all. Unlike the two
///   below, this one is *also* a statement about covering — see
///   [`Solium::pane_chrome`], which asks it in both places — because an
///   invisible pane is the one kind that offers nothing and occludes nothing
///   either. Issue #127's review finding 1.
/// - **`managed`.** An unmanaged pane is one its client placed and owns: an
///   X11 menu, a tooltip, a dropdown. Nothing here ever sizes it or moves it —
///   `show_if_new` and `snapshot` both ask [`crate::pane::Pane::managed`] and
///   nothing else — and `size_window` refuses an override-redirect surface
///   outright. So a resize border eight pixels outside a Steam menu is a
///   cursor promising a drag that cannot happen, and a press there starts a
///   `ResizeGrab` that does nothing instead of dismissing the menu. Such a
///   pane still *occludes*, which is [`pane_hit_of`]'s business and not this
///   one's: covering is a fact about pixels, offering chrome is a claim about
///   what a press means.
/// - **`window`.** A resize needs a window to resize. A frame does not: a
///   frame around a window whose application has not arrived still has working
///   buttons, which is the point of giving it one.
pub(crate) fn chrome_offered(
    shows: bool,
    managed: bool,
    window: bool,
    framed: bool,
    edges: ResizeEdge,
) -> Option<Chrome> {
    if !shows || !managed {
        return None;
    }
    chrome_of(framed, edges).filter(|chrome| window || !matches!(chrome, Chrome::Resize(_)))
}

/// The chrome under a point, given what every pane makes of it, topmost first.
///
/// **One pass, and the first pane with anything to say ends it.** This is the
/// fix for issue #111 stated as a rule: a pane that covers the point answers
/// [`PaneHit::Client`], which stops the walk with `None`, and the panes below
/// it are never asked. Before this the walk could only be stopped by a *match*,
/// so a covering window was indistinguishable from an absent one and the point
/// fell through to a lower window's titlebar.
///
/// **A halo is the weakest claim there is, and that is the correction to the
/// first fix.** [`PaneHit::Halo`] — chrome outside the pane's own drawn rect —
/// is remembered and the walk carries on, so it is used only if nothing below
/// paints that pixel. It beats bare desktop, which is what makes an edge
/// grabbable from outside at all, and it loses to any lower pane that actually
/// draws there.
///
/// The rule that stood here briefly was that a higher pane's border simply wins,
/// "because the higher window is on top". That is unanswerable at a pixel the
/// higher pane does not occupy: an upper window's edge floating four pixels
/// above a lower window's close button drew `NsResize` over a visibly drawn,
/// clickable control, and a press there started a resize grab instead of
/// closing the window. Stacking order decides who owns a pixel among the panes
/// that *draw* it; a pane drawing nothing there is not in that contest. Which
/// is also why the two-pass shape this replaces was not wrong for the reason
/// #111's fix first gave: running every frame before any border did give a
/// lower titlebar the point, and at a point outside the upper window that
/// happens to be the right answer. It was wrong because it reached it without
/// consulting stacking order at all, so it got the covered-titlebar case
/// (#111) and the *inside* half of a higher border wrong by the same omission.
///
/// Generic over what a pane answers with, and taking the answers rather than
/// the panes, for the reason [`chrome_of`] and [`claim_of`] are written the
/// same way: a `Solium` needs a `Display` and cannot be stood up in a unit
/// test, and a stacking rule that can only be exercised by running the
/// compositor is a rule that goes untested until somebody notices it on
/// hardware. Which is how #111 was found.
///
/// The iterator is walked lazily and abandoned at the first pane that claims
/// the point with something it draws, so a pane under a covering window is
/// never hit-tested at all. A halo alone does not abandon it: what is under
/// the halo is exactly the question.
pub(crate) fn topmost_chrome<T>(stack: impl IntoIterator<Item = PaneHit<T>>) -> Option<T> {
    // The topmost halo, kept because the walk cannot yet tell whether anything
    // below draws where it hangs. Later halos are lower and never displace it:
    // among panes that all merely hover over a point, the top one still wins.
    let mut halo = None;
    for hit in stack {
        match hit {
            PaneHit::Chrome(chrome) => return Some(chrome),
            PaneHit::Client => return None,
            PaneHit::Halo(chrome) => halo = halo.or(Some(chrome)),
            PaneHit::Miss => {}
        }
    }
    halo
}

/// Who a press at a point belongs to, before any client sees it.
///
/// [`Chrome`] is one link of this and not the whole of it, which is what the
/// first pass at #108 got wrong: the cursor was read off `chrome_under` alone
/// while [`crate::input`]'s `pointer_button` consults two other things first,
/// so in a mode — or over a scripted bar — the pointer went on describing a
/// resize that the press was never going to perform. Same bug, one altitude up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Claim {
    /// A scripted surface above the windows takes it: a bar, a panel, an
    /// overlay. What it does with the press is the script's business and the
    /// compositor has nothing to say about it.
    Surface,
    /// A mode owns input — `sol.grab(true)`, which is overview. Every press is
    /// the mode's, whatever is drawn under the pointer.
    Mode,
    /// The compositor's own chrome: a frame's band, or a resize border.
    Chrome(Chrome),
    /// None of the compositor's: the press reaches a window or a client.
    Nothing,
}

impl Claim {
    /// The cursor the compositor asserts, or `None` to say nothing and leave
    /// the pointer to whoever owns the point.
    ///
    /// **Only `Chrome` names a shape.** The other three are the compositor
    /// declining to describe the press, which is the whole content of this
    /// finding: a thumbnail's corner in overview is within eight pixels of a
    /// window edge and `chrome_under` will happily call it `BottomRight`, but
    /// the press there focuses the window and leaves the mode. A pointer that
    /// promised a resize would be #108's second symptom again, on a key
    /// combination used every day.
    pub(crate) fn cursor(self) -> Option<CursorIcon> {
        match self {
            Self::Chrome(chrome) => Some(chrome.cursor()),
            Self::Surface | Self::Mode | Self::Nothing => None,
        }
    }
}

/// `pointer_button`'s precedence chain, as a rule rather than as a sequence of
/// early returns.
///
/// The three links are asked in the order the press asks them, and the order is
/// the point: a scripted overlay above the windows is offered the press before
/// a mode is consulted, and a mode before any chrome. `pointer_button` is where
/// each answer is *acted* on — it has a surface to deliver to, a click to
/// trigger and an [`Under`] to start a grab from, none of which fit in a value
/// — but which one wins is decided here, and the pointer's shape is read off
/// the result rather than off the last link alone.
///
/// Pure, and taking the links already answered, for the same reason
/// [`chrome_of`] is: a `Solium` needs a `Display` and cannot be stood up in a
/// unit test, so a rule that lives inside one is a rule that is checked by
/// running the compositor and noticing. Which is how both halves of #108 were
/// found.
pub(crate) fn claim_of(surface: bool, mode: bool, chrome: Option<Chrome>) -> Claim {
    if surface {
        return Claim::Surface;
    }
    if mode {
        return Claim::Mode;
    }
    match chrome {
        Some(chrome) => Claim::Chrome(chrome),
        None => Claim::Nothing,
    }
}

/// Whether a point in a pane's own coordinates lands on its frame rather than
/// on its client.
///
/// The band is everything inside the pane's outer rectangle that the insets
/// reserve — a titlebar across the top, a bar down a side, a border all round,
/// whatever the decoration asked for — and the complement is the client's,
/// wherever the frame chose to draw itself inside it. A decoration that
/// reserves nothing owns no band at all, and its clicks belong to the window
/// under it.
///
/// The outer bound is part of the predicate and not a caller's business: the
/// insets say how far in the client starts, so without it every point above a
/// window would be "not the client" and therefore the titlebar.
pub(crate) fn on_frame(
    size: Size<i32, Logical>,
    insets: Insets,
    local: Point<f64, Logical>,
) -> bool {
    let pane = Rectangle::new(Point::from((0.0, 0.0)), size.to_f64());
    let client = Rectangle::new(
        Point::from((f64::from(insets.left), f64::from(insets.top))),
        Size::from((
            f64::from(size.w - insets.horizontal()),
            f64::from(size.h - insets.vertical()),
        )),
    );
    pane.contains(local) && !client.contains(local)
}

/// The client's rect inside an outer one, once the frame has taken its share.
fn inner(outer: Rectangle<i32, Logical>, insets: Insets) -> Rectangle<i32, Logical> {
    Rectangle::new(
        (outer.loc.x + insets.left, outer.loc.y + insets.top).into(),
        (
            (outer.size.w - insets.horizontal()).max(1),
            (outer.size.h - insets.vertical()).max(1),
        )
            .into(),
    )
}

/// What a placement says about the tile a pane is held in.
///
/// Since #133 `Pane::placed` is two things: the layout's own rectangle for a
/// pane, which a tiled edge drag starts from (#124), and the tile a client is
/// held inside, which `Solium::pane_geometry` caps a client at and
/// `render::elements` cuts it to. The second is only right for a *tile*, and
/// `Solium::move_pane` is reached by more than tiles, so every caller says
/// which it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Standing {
    /// A layout's tile. The client is held inside this rectangle from now on.
    Tile,
    /// A layout placing a window it does not tile: `dialogs.lua` centring a
    /// modal over its parent, through `sol.place` with `tile = false`. The pane
    /// is taken out of any tile it was in, because a dialog is a floating
    /// window whichever mode is running -- at once, or, for a window being
    /// closed, once it is back: see `Pane::let_go`.
    Free,
    /// Not a layout at all -- `Solium::rescue_offscreen` dragging a window
    /// back onto a screen. A tiled pane is still tiled, at the rectangle it was
    /// brought back to; a floating one is still floating.
    Kept,
}

/// Tell a window how big it is, in whichever protocol it speaks.
///
/// An xdg toplevel is asked and answers on its own schedule; an X11 window is
/// simply told, position included, because X11 has no separate notion of the
/// manager's opinion.
///
/// Refuses outright for an override-redirect X11 window, before ever calling
/// `configure`. Smithay already rejects that call on its own account
/// (`X11SurfaceError::UnsupportedForOverrideRedirect`) — OR means the client
/// picked its own geometry and no manager may second-guess it — so this
/// early return changes no behaviour by itself; smithay was already refusing
/// it, which is the `could not size an X11 window` warning this removes.
/// What the early return buys is a single place that knows the rule: every
/// caller of `size_window` funnels through here, so "never size an OR
/// window" is enforced once, for all four call sites and whatever is added
/// later, instead of each of them having to remember to ask first.
fn size_window(window: &Window, client: Rectangle<i32, Logical>) {
    if let Some(toplevel) = window.toplevel() {
        toplevel.with_pending_state(|state| state.size = Some(client.size));
        toplevel.send_pending_configure();
        return;
    }
    let Some(x11) = window.x11_surface() else {
        return;
    };
    if x11.is_override_redirect() {
        return;
    }
    if let Err(err) = x11.configure(Some(client)) {
        tracing::warn!(?err, "could not size an X11 window");
    }
}

impl Solium {
    pub(crate) fn new(display_handle: DisplayHandle) -> Self {
        let mut seat_state = SeatState::new();
        // "solium", not "winit". The name reaches clients through `wl_seat`,
        // and the nested backend's name was hardcoded here, so a session on
        // real hardware announced a seat called after a windowing library it
        // was not using.
        let mut seat = seat_state.new_wl_seat(&display_handle, "solium");

        // Every capability is advertised on every form factor. Which of them a
        // machine actually has is a hardware question; how it behaves is the
        // profile's, and advertising a capability that never sends events is
        // cheaper than a client that cannot discover a device that appears.
        // `XkbConfig::default()` is empty strings, and that is the useful
        // default rather than a lazy one: xkbcommon reads `XKB_DEFAULT_LAYOUT`
        // and its siblings when the names are empty, so a session that sets
        // them in the usual place is honoured before any script has run. A
        // script can then say otherwise -- see `keymap.rs`.
        let _ = seat.add_keyboard(
            Default::default(),
            crate::keymap::REPEAT_DELAY,
            crate::keymap::REPEAT_RATE,
        );
        let _ = seat.add_pointer();
        let _ = seat.add_touch();

        Self {
            compositor_state: CompositorState::new::<Self>(&display_handle),
            xdg_shell_state: XdgShellState::new::<Self>(&display_handle),
            xdg_dialog_state: XdgDialogState::new::<Self>(&display_handle),
            shm_state: ShmState::new::<Self>(&display_handle, Vec::new()),
            output_manager_state: OutputManagerState::new_with_xdg_output::<Self>(&display_handle),
            data_device_state: DataDeviceState::new::<Self>(&display_handle),
            dnd_icon: None,
            loading: crate::script::Loading::default(),
            hovered_frame: None,
            reported_at: std::time::Duration::ZERO,
            xwm: None,
            x11_display: None,
            xwayland_shell_state: smithay::wayland::xwayland_shell::XWaylandShellState::new::<Self>(
                &display_handle,
            ),
            primary_selection_state: PrimarySelectionState::new::<Self>(&display_handle),
            relative_pointer_state: RelativePointerManagerState::new::<Self>(&display_handle),
            pointer_constraints_state: PointerConstraintsState::new::<Self>(&display_handle),
            constraint_hint: None,
            // Version 2, which smithay's `new` asks for unconditionally: it
            // adds `dnd-ask` and `all-resize` to the shape enum, and a client
            // bound at version 1 simply never sends them. `cursor::shape` maps
            // both, so there is nothing to gate.
            cursor_shape_state: CursorShapeManagerState::new::<Self>(&display_handle),
            pending_selection: None,
            activation_state: XdgActivationState::new::<Self>(&display_handle),
            viewporter_state: ViewporterState::new::<Self>(&display_handle),
            single_pixel_buffer_state: crate::single_pixel::state(&display_handle),
            // 1 is CLOCK_MONOTONIC, which is the clock every timestamp in this
            // compositor comes from -- both the DRM page-flip time and our own
            // animation clock. Telling a client a different clock id than the
            // one the numbers are on is worse than not telling it at all.
            presentation_state: PresentationState::new::<Self>(&display_handle, 1),
            fractional_scale_state: FractionalScaleManagerState::new::<Self>(&display_handle),
            xdg_decoration_state: XdgDecorationState::new::<Self>(&display_handle),
            layer_shell_state: WlrLayerShellState::new::<Self>(&display_handle),
            idle_state: crate::idle::IdleState::new::<Self>(&display_handle),
            idle_inhibit_state: crate::idle::inhibit_state(&display_handle),
            idle: crate::idle::Idle::default(),
            surfaces: crate::scripted::Surfaces::default(),
            groups: crate::group::Groups::default(),
            keymap: None,
            keyboard: crate::keymap::State::initial(),
            session_lock_state: crate::lock::state(&display_handle),
            lock: None,
            rescan_outputs: false,
            seat_state,
            screencopy_state: crate::screencopy::ScreencopyState::new::<Self>(&display_handle),
            pending_captures: Vec::new(),
            space: Space::default(),
            arrangement: monitor::Arrangement::default(),
            panes: crate::pane::Panes::default(),
            popups: PopupManager::default(),
            popup_grab: None,
            seat,
            clock: Clock::new(),
            profile: Profile::from_env(),
            scripts: None,
            status: String::new(),
            script_grab: false,
            keys_forwarded: std::collections::HashSet::new(),
            refocus_on_release: false,
            socket_name: String::new(),
            decorations: Decorations::default(),
            pointer: crate::cursor::Pointer::default(),
            programs: crate::pass::Programs::default(),
            textures: None,
            published_windows: String::new(),
            focusing: false,
            closing: None,
            pending_drop: None,
            pending_resize: None,
            resize_hold: None,
            resize_bridge: None,
            resize_gesture: None,
            resize_ended: None,
            resizing: crate::resizing::Settings::default(),
            redraw: true,
            animating: false,
            dmabuf_state: DmabufState::new(),
            dmabuf_global: None,
            request: None,
            display_handle,
        }
    }
}

impl Solium {
    /// Where a window actually lives, as opposed to where it is drawn.
    ///
    /// The layout's answer. Transforms are expressed relative to it and never
    /// write back to it, which is what makes leaving a mode exact.
    pub(crate) fn real_geometry(&self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        let location = self.space.element_location(window)?;
        Some(Rectangle::new(location, window.geometry().size))
    }

    /// A window as drawn, frame included.
    ///
    /// The client rect grown upward by the titlebar, when the window has one.
    /// **Every presentation transform is expressed against this**, which is
    /// what makes a frame move, scale and animate with its window instead of
    /// beside it — in overview a thumbnail carries its own titlebar.
    pub(crate) fn outer_geometry(&self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        Some(grown(
            self.real_geometry(window)?,
            self.frame_insets(window),
        ))
    }

    /// Where a pane lives, as opposed to where it is drawn.
    ///
    /// A pane with a client asks the space, which is the authority for a
    /// mapped window. A pane without one answers from its own slot — the
    /// layout's answer for it, and the only one there is.
    ///
    /// **Total, and it always was.** Every arm below already ended in `Some`,
    /// because every pane has a slot and a slot is always an answer. It
    /// returned `Option` anyway, and twenty-odd callers wrote a bail for a
    /// `None` that cannot happen — one of which, in `close_pane`, read as a
    /// policy: a window the compositor cannot locate and therefore declines to
    /// close. Dead code that describes a real-sounding case is worse than no
    /// code, so the type says what the function does. `pane_outer_of` still
    /// answers `Option`, because *that* question — is there a pane with this id
    /// — really can be no.
    pub(crate) fn pane_geometry(&self, pane: &Pane) -> Rectangle<i32, Logical> {
        // A pane whose client has gone answers where the client was, as this
        // function answered it while there was one: the slot is the last thing
        // `sync_panes` or a layout wrote, which is not where it was drawn.
        if let Some(left) = pane.left() {
            return left.geometry;
        }
        let Some(window) = pane.client() else {
            return pane.slot();
        };
        // **While an edge is being dragged the slot outranks the client.**
        //
        // This is the inversion issue #113 needed. Everywhere else the space is
        // the authority for a mapped window, and the space reports a window's
        // size as whatever the *client* last committed -- so a drag that moved
        // the origin now and asked for the size later drew a rectangle with a
        // new origin and an old size, which moves the edge nobody is dragging
        // and snaps it back when the client answers. Here the rectangle the
        // user is dragging is the truth, immediately, and the client's last
        // buffer is bridged into it by `resizing::factor` until it catches up.
        //
        // Bounded, and that matters more than the inversion: `resizing::Hold`
        // ends this the moment the client answers -- with what it was asked for
        // or with anything else -- and on a deadline if it answers nothing.
        // See `crate::resizing`.
        if self.holding_resize(pane.id()) {
            return pane.slot();
        }
        // A client that has mapped and not yet answered the size it was asked
        // for has a window of no size at all. Taking the space's word for that
        // collapses the pane to nothing for as long as it lasts -- which is
        // most of the moment an application is starting, and is exactly the
        // blank gap between the scene and the client. The slot is what the
        // layout said, and it is still the truth. Same rule as `Panes::sync`.
        //
        // **And a tiled client is held inside its tile (#133).** The space's
        // size is whatever the client committed, and a client that will not
        // shrink as far as its tile -- kitty on its cell grid, Firefox at its
        // minimum width -- committed more than the tile has. Taken as it is,
        // that rectangle is the frame canvas, the titlebar's width and the hit
        // test, all reaching over the neighbouring window; and it is also what
        // every sibling looks like for the moment between a new window opening
        // and each shrinking client answering. So the size is cut, per axis,
        // to the client's share of the tile -- the tile is the answer, and the
        // client catches up inside it. Cut and not replaced: a client *smaller*
        // than its tile keeps its own size, which is the cell-grid residue
        // `pane_laid_out` describes and must go on seeing. At rest,
        // `render::elements` cuts the surfaces to the same rectangle, because
        // `render::fit` cuts a tiled client to whatever its frame pictures and
        // a frame at rest pictures this. See `Self::shown_size`.
        match self.real_geometry(window) {
            Some(real) if real.size.w > 0 && real.size.h > 0 => {
                Rectangle::new(real.loc, self.shown_size(pane, real.size))
            }
            _ => pane.slot(),
        }
    }

    /// The client's share of the tile a layout holds this pane in, while one
    /// does and nothing outranks it.
    ///
    /// `None` for a pane in no tile -- see `Pane::placed` for every way out of
    /// one -- and `None` while a resize hold is live. The hold is the other
    /// authority over a tiled pane's size, and a stronger one: the dragged
    /// rectangle is the truth for as long as it lasts, and the client's last
    /// buffer is bridged into it by `resizing::factor`. Capping that as well
    /// would cut the bridge down to a tile the drag is in the middle of
    /// moving. `a_held_pane_is_bridged_and_not_cut_to_its_tile` pins it.
    ///
    /// The share is taken with the insets as they are now, which is the same
    /// arithmetic `move_pane` configured the client with.
    pub(crate) fn tile_of(&self, pane: &Pane) -> Option<Size<i32, Logical>> {
        if self.holding_resize(pane.id()) {
            return None;
        }
        Some(inner(pane.placed()?, self.insets_of(pane.id())).size)
    }

    /// A committed size, as this pane shows it at rest: cut per axis to its
    /// tile.
    ///
    /// The committed size itself for a pane in no tile, and on any axis the
    /// client is already inside. What `pane_geometry` answers, which is what
    /// the frame, the hit test and every transform read. The two places that
    /// draw a client's pixels do not ask this: they ask `render::place_client`
    /// of the frame being drawn, which on a frame at rest cuts to exactly this
    /// (`a_titled_tiled_client_is_held_inside_its_tile_frame_and_all`) and on
    /// a frame of a glide cuts to the rectangle the glide has reached instead,
    /// which this cannot know
    /// (`a_glide_that_narrows_a_tiled_window_is_drawn_1_to_1_on_its_first_frame`).
    pub(crate) fn shown_size(
        &self,
        pane: &Pane,
        committed: Size<i32, Logical>,
    ) -> Size<i32, Logical> {
        self.tile_of(pane).map_or(committed, |tile| {
            (committed.w.min(tile.w), committed.h.min(tile.h)).into()
        })
    }

    /// Whether a client has anything worth showing yet.
    ///
    /// Both halves matter. A client with no buffer has painted nothing; a
    /// client with a buffer and no size has a window of no size, and drawing
    /// it draws nothing. Until both are true the compositor is still the one
    /// with something to show.
    pub(crate) fn client_ready(&self, window: &Window) -> bool {
        let size = window.geometry().size;
        size.w > 0 && size.h > 0 && self.has_content(window)
    }

    /// Where a pane *lives*, frame included. Every presentation transform is
    /// expressed against this.
    ///
    /// **Not "as drawn", which is what this used to say and has not been true
    /// since #113.** Two things are wrong with that reading. Where a pane is
    /// drawn is [`Self::drawn_at`] — this rectangle put through whatever
    /// transform the pane is carrying — and the two differ by the whole of
    /// every animation, every mode and every group shift; a closing window is
    /// drawn shrunk and transparent at a rectangle this function knows nothing
    /// about. And even as a statement about geometry it is wrong in the one
    /// case it was written for: [`Self::pane_geometry`] returns the *dragged
    /// slot* while a resize hold is live, which is deliberately a rectangle the
    /// client has not agreed to and is not yet painting.
    ///
    /// What it is, is the rectangle transforms are expressed against and hit
    /// tests are resolved in — the pane's real geometry grown by its frame.
    pub(crate) fn pane_outer(&self, pane: &Pane) -> Rectangle<i32, Logical> {
        // Kept whole rather than grown again from the kept geometry, so a
        // style change during the fade does not move the rectangle its
        // transform is expressed against.
        if let Some(left) = pane.left() {
            return left.outer;
        }
        grown(self.pane_geometry(pane), self.insets_of(pane.id()))
    }

    /// The same, for a caller that holds only the pane's id.
    ///
    /// `Option` here is the *lookup* failing, not the geometry: an id whose
    /// pane has been retired has no rectangle because it has no pane.
    pub(crate) fn pane_outer_of(&self, id: crate::pane::PaneId) -> Option<Rectangle<i32, Logical>> {
        Some(self.pane_outer(self.panes.get(id)?))
    }

    /// Where the *layout* has this pane, in the layout's outer space.
    ///
    /// **Not [`Self::pane_outer`], and the two differ by exactly the amount a
    /// client has disagreed with what it was asked for.** `pane_outer` goes
    /// through [`Self::pane_geometry`], which answers `real_geometry` for a
    /// mapped client with no hold live — the space's location paired with the
    /// size the *client* committed. `Pane::placed` is the rectangle the layout
    /// asked for, written by [`Self::move_pane`] and by nothing that hears from
    /// a client.
    ///
    /// The difference is small, silent and constant: a terminal quantises to
    /// its cell grid, so it answers a few pixels short on every configure it is
    /// ever sent, and `settle_resize_hold` then *adopts* that answer into the
    /// slot rather than fighting it — see `crate::resizing`, which puts the
    /// tolerance for calling such an answer a rounding at `max(asked / 20,
    /// CELL)`. Measured on this compositor's own fixture: a pane placed 500
    /// wide whose client committed 492 has a `slot` and a `pane_outer` of 492
    /// from the next frame onward, and a `placed` of 500.
    ///
    /// **Which is why an edge drag starts here.** A tiled drag hands the layout
    /// a position and `tree:drag_seam` puts the seam exactly there, so a first
    /// frame that has not moved must hand back a number the layout itself
    /// produced or the seam shifts by the client's residue before the pointer
    /// has travelled a pixel. `pane_outer` cannot do that for a right or bottom
    /// edge: `real.loc` is compositor-set so the left and top edges are exact,
    /// and the far edges carry the whole of the client's disagreement. See
    /// `crate::input::resize::dragged_edge`.
    ///
    /// **A maximised or fullscreen window answers the tile it left**, from
    /// `Pane::left_tile`. It is out of its tile for the cap's sake -- a
    /// maximised window is not cut to the corner it was tiled in -- but it is
    /// still a leaf of its layout's tree, and nothing on the drag path asks
    /// about maximised: `settle_resize` hands a drag on it to
    /// `trigger_resize`, and `tiling.lua`'s `resize` handler asks only whether
    /// tiling is on. Answered with the work area instead, `tree:drag_seam` was handed
    /// the screen's edge plus the pointer's travel and threw the seam to its
    /// clamp on the first frame.
    /// `a_drag_on_a_maximised_tiled_window_starts_from_its_tile` pins it.
    ///
    /// Falls back to `pane_outer` for a pane in no tile and waiting for none:
    /// a floating window -- where nothing reads this, because no layout claims
    /// the drag -- a dialog a layout centred without tiling it, or the frames
    /// between a window mapping and the first sweep, where the pane's own
    /// rectangle is the only answer there is.
    ///
    /// **A pane that was laid out and is not any more answers `pane_outer`
    /// too, and until #133 it did not.** It answered where the last layout
    /// left it, deliberately: the only reader was this edge, and clearing the
    /// field meant teaching every mode that stops placing a window to say so.
    /// #133 made `Pane::placed` the tile a client is held inside as well, and
    /// a stale one of those is a window cut down to where it used to be — so
    /// every way out of a tile clears it now, and `modes.use` is taught to say
    /// so with `sol.unplace`.
    ///
    /// **Every way out but a close, and that one is deliberate (#128).** A
    /// layout's `closing` handler takes the window out of its tree and nothing
    /// clears the field, because the tile is what cuts the window for the
    /// whole of its fade; and a `sol.unplace` that arrives during the fade
    /// waits until the window is back. `Pane::placed` has the reasons and
    /// `Pane::let_go` the waiting, and
    /// `a_closed_window_is_cut_to_the_tile_it_left_for_the_whole_fade` fails
    /// if the field is cleared at `closing`.
    ///
    /// What this function answers for a pane that *is* tiled is unchanged:
    /// `a_client_that_rounds_its_size_does_not_move_the_seam` still drives it,
    /// and `a_restored_window_goes_back_into_its_tile` covers the one way back
    /// that has to put the layout's rectangle back as well.
    pub(crate) fn pane_laid_out(&self, window: &Window) -> Option<crate::input::resize::LaidOut> {
        let pane = self.panes.get(self.panes.id_of(window)?)?;
        Some(crate::input::resize::LaidOut(
            pane.placed()
                .or_else(|| pane.left_tile())
                .unwrap_or_else(|| self.pane_outer(pane)),
        ))
    }

    /// How a pane is being drawn right now. Real geometry unless something is
    /// animating it, and real geometry for a pane that has gone.
    ///
    /// **Samples the clock itself, so a caller asking about more than one pane
    /// wants [`Self::drawn_id_at`] instead.** `present::Clock` reads through
    /// rather than caching — for reasons its own documentation gives — so N
    /// calls here are N instants, and a frame's worth of panes would each be
    /// drawn at a slightly different moment of the same animation.
    pub(crate) fn drawn(&self, id: crate::pane::PaneId, real: Rectangle<i32, Logical>) -> Frame {
        self.drawn_id_at(id, real, self.clock.now())
    }

    /// The same, for a caller that holds the pane's id and already has the
    /// instant.
    ///
    /// The third of the three ways to ask this question, and the one a whole
    /// frame wants: [`Self::drawn`] holds neither the pane nor the instant,
    /// [`Self::drawn_at`] holds both, and this holds only the instant. All
    /// three end in `drawn_at`, so the rule about a pane's own transform and
    /// its groups' being put together in exactly one place is unaffected.
    pub(crate) fn drawn_id_at(
        &self,
        id: crate::pane::PaneId,
        real: Rectangle<i32, Logical>,
        now: std::time::Duration,
    ) -> Frame {
        self.panes
            .get(id)
            .map_or_else(|| Frame::real(real), |pane| self.drawn_at(pane, real, now))
    }

    /// The same, for a caller that already holds the pane and the instant.
    ///
    /// **The one place a pane's own transform and its groups' are put
    /// together**, so no caller can ask for one and forget the other. Every
    /// reader of a drawn rectangle goes through here — the renderer, the hit
    /// test, the resize edges, the window list a script sees — which is what
    /// keeps "a workspace you cannot see is one you cannot click into by
    /// accident" true now that the workspace is a selection rather than a
    /// transform per window.
    ///
    /// Hit-testing follows the *rectangle* and not the matrix, which is
    /// unchanged: a window drawn in perspective is still clicked where the
    /// layout put it, and a mode that wants otherwise inverts its own transform
    /// through `present::to_window_space`.
    ///
    /// **The instant is the caller's, and which one is a rule rather than a
    /// habit.** The renderer and a press pass the present; the focus questions
    /// pass [`Self::settling`]. [`SETTLED`] states it and names the tests on
    /// both sides of it.
    pub(crate) fn drawn_at(
        &self,
        pane: &Pane,
        real: Rectangle<i32, Logical>,
        now: std::time::Duration,
    ) -> Frame {
        self.carried_at(pane, real, now)
            .apply(present::frame(pane, real, now))
    }

    /// How the selections a pane is in carry it at `now`: the half of
    /// [`Self::drawn_at`] that is the groups', before it is composed onto the
    /// pane's own frame -- which `Shift::apply` returns untouched when there
    /// is nothing to carry.
    ///
    /// [`Self::carried_by_a_selection`] asks the same selections where they
    /// are *headed*, `Groups::bound_for_window`, with the same monitor; see it
    /// for why that is not this asked late.
    fn carried_at(
        &self,
        pane: &Pane,
        real: Rectangle<i32, Logical>,
        now: std::time::Duration,
    ) -> crate::group::Shift {
        // Carried by the selections it was in when its client went, as they
        // move now, and whoever they hold. See `crate::pane::Left::groups`.
        if let Some(left) = pane.left() {
            return self.groups.on_named(&left.groups, now);
        }
        if self.groups.is_empty() {
            return crate::group::Shift::NONE;
        }
        let monitor = self.named_monitor_of(real);
        self.groups
            .on_window(pane.id().get(), monitor.as_deref(), now)
    }

    /// The connector a pane is on, when a selection names a monitor.
    ///
    /// Only worked out when a selection has actually named a screen: this is
    /// a geometric search over the outputs, per pane, per frame.
    fn named_monitor_of(&self, real: Rectangle<i32, Logical>) -> Option<String> {
        self.groups
            .names_monitors()
            .then(|| self.output_of(real).map(|output| output.name()))
            .flatten()
    }

    /// The panes stacked under this one this instant, bottom to top: what a
    /// pane whose client is going stays over while it fades. See
    /// `crate::pane::Left::over`.
    ///
    /// From the space for a window in it, which is the authority on how
    /// clients are stacked, and from the panes for one that is not -- a window
    /// whose application never arrived, which the space has never held.
    fn stacked_under(&self, pane: &Pane) -> Vec<crate::pane::PaneId> {
        let in_space = pane.client().and_then(|window| {
            self.space
                .elements()
                .position(|each| each == window)
                .map(|at| {
                    self.space
                        .elements()
                        .take(at)
                        .filter_map(|each| self.panes.id_of(each))
                        .collect()
                })
        });
        in_space.unwrap_or_else(|| {
            self.panes
                .iter()
                .take_while(|each| each.id() != pane.id())
                .map(Pane::id)
                .collect()
        })
    }

    /// Where a layout has a pane: the tile it asked for, or where the pane is
    /// when it is in none. What `Self::depart` compares across a `close` to
    /// find the windows the layout grew into the space of one that went.
    fn laid_out_at(&self, pane: &Pane) -> Rectangle<i32, Logical> {
        pane.placed().unwrap_or_else(|| self.pane_outer(pane))
    }

    /// Where one monitor's instance of a scripted surface is actually drawn.
    ///
    /// The surface half of [`Self::drawn_at`], and deliberately a rectangle
    /// rather than a `Frame`: a selection reaches a surface as a displacement
    /// and an opacity, and no further. A matrix or a deformation on a group
    /// reaches its *panes* — bending a surface means capturing it into a texture
    /// first, and a scripted surface is a memory buffer on the software path,
    /// where there is no texture to bend. That is `offscreen::capture` for
    /// surfaces, which is a change of its own and not a line of this one.
    pub(crate) fn carried(
        &self,
        id: crate::scripted::SurfaceId,
        output: &Output,
        area: Rectangle<i32, Logical>,
    ) -> Rectangle<i32, Logical> {
        if self.groups.is_empty() {
            return area;
        }
        let (dx, dy) = self
            .groups
            .on_surface(id, &output.name(), self.clock.now())
            .offset();
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a displacement on this desktop, in logical pixels"
        )]
        let moved = Rectangle::new(
            (
                area.loc.x + dx.round() as i32,
                area.loc.y + dy.round() as i32,
            )
                .into(),
            area.size,
        );
        moved
    }

    /// How much of a scripted surface a selection is showing.
    pub(crate) fn carried_alpha(&self, id: crate::scripted::SurfaceId, output: &Output) -> f32 {
        if self.groups.is_empty() {
            return 1.0;
        }
        self.groups
            .on_surface(id, &output.name(), self.clock.now())
            .opacity
    }

    /// Turn what a script aimed at into what the compositor holds.
    ///
    /// The one place a surface's *name* becomes a [`crate::scripted::SurfaceId`]
    /// — which is what makes an anchor `Copy` and a `Frame` still cheap to
    /// blend. A name nobody has declared loses the effect and not the window,
    /// the same failure an anchor that stops resolving already has, and it says
    /// so once rather than every frame.
    fn aimed(&self, deform: &crate::script::Deform) -> Option<present::Deform> {
        let anchor = match &deform.aim {
            crate::script::Aim::Rect(rect) => {
                present::Anchor::Rect(present::logical((rect.x, rect.y), (rect.w, rect.h)))
            }
            crate::script::Aim::Window(id) => present::Anchor::Pane(*id),
            crate::script::Aim::Surface(name) => match self.surfaces.named(name) {
                Some(id) => present::Anchor::Surface(id),
                None => {
                    tracing::warn!(
                        surface = name,
                        "no surface by that name to aim at, drawing the window undeformed"
                    );
                    return None;
                }
            },
        };
        Some(present::Deform {
            effect: deform.effect,
            anchor,
        })
    }

    /// Put back the windows a membership change has just moved.
    ///
    /// **What happens when membership changes while things are animating**, and
    /// the reason it is not a jump. A window sent to another workspace leaves
    /// one selection for another, and the difference between the two shifts
    /// lands on it between one frame and the next; this displaces its own
    /// transform by exactly that much, so the frame after the change draws it
    /// where the frame before did, and animates it home.
    ///
    /// Only windows. A surface joining a selection has no transform of its own
    /// to displace — there is nowhere to put one, and the case it would cover
    /// (a wallpaper changing desk) is not a thing a desk does. A selection that
    /// names a monitor is not rebased either: what is on a screen changes
    /// because the *user* dragged a window across a bezel, which no declaration
    /// observes.
    fn keep_displaced(
        &mut self,
        displaced: &crate::group::Displaced,
        now: std::time::Duration,
        animation: crate::script::AnimationSpec,
    ) {
        if displaced.is_empty() {
            return;
        }
        for (id, by) in displaced {
            let Some(pane) = self.panes.by_script_id(*id) else {
                continue;
            };
            // Not a window whose client has gone: it is drawn under the
            // selections it was in when it went, by name, which no membership
            // change touches, so nothing moved it and there is nothing to put
            // back. Rebased, it was displaced twice, and glided out from under
            // its desk while it faded.
            // `a_window_that_left_keeps_the_shift_its_desk_had`.
            if pane.ghost() {
                continue;
            }
            let outer = self.pane_outer(pane);
            present::rebase(pane, outer, *by, now, animation.duration, animation.easing);
        }
        self.redraw = true;
    }

    /// Turn a deform's anchor into the rectangle it is aimed at *this frame*.
    ///
    /// The compositor half of `crates/effects`. The crate is handed two
    /// rectangles and knows nothing about panes — that is what keeps it
    /// testable without a session — so the identity a script named is resolved
    /// here, on the frame it is drawn on, and never snapshotted when the
    /// script ran. A dock icon the user is still opening windows next to is
    /// somewhere else 500 ms later.
    ///
    /// A pane is resolved through [`Self::drawn`] rather than to its layout
    /// slot, so a genie aimed at a window that is itself animating follows the
    /// window and not the hole it is leaving. One level deep and no deeper:
    /// `drawn` reads a pane's own transform and resolves no anchors of its
    /// own, so two panes aimed at each other cannot recurse.
    ///
    /// `None` when the anchor names nothing: the pane has closed, or never
    /// existed. The caller draws the window flat, which is the failure that
    /// loses an effect rather than the frame.
    pub(crate) fn aimed_at(&self, deform: Option<present::Deform>) -> Option<present::Aimed> {
        let deform = deform?;
        let to = match deform.anchor {
            present::Anchor::Rect(rect) => rect,
            present::Anchor::Pane(id) => {
                let pane = self.panes.by_script_id(id)?;
                self.drawn(pane.id(), self.pane_outer(pane)).rect
            }
            // The monitor in front of the user, and the primary one when there
            // is no pointer yet. Not "the first output that answers": that is
            // stable only until somebody plugs a screen in on the other side.
            present::Anchor::Surface(id) => {
                let surface = self.surfaces.get(id)?;
                let output = self.active_output().or_else(|| self.primary_output())?;
                let geometry = self.space.output_geometry(&output)?;
                let primary = self.primary_output();
                let area = surface.area_on(&output, geometry, primary.as_ref())?;
                self.carried(id, &output, area).to_f64()
            }
        };
        Some(present::Aimed {
            effect: deform.effect,
            to,
        })
    }

    /// Gather the presentation-feedback callbacks committed for this frame.
    ///
    /// Taken *before* the frame is sent, and reported when it has actually been
    /// shown — which on the hardware is the page flip and nowhere earlier. A
    /// compositor that answers at render time is answering a different question
    /// than the one asked, and answering it with a number that is always early.
    pub(crate) fn presentation_feedback(
        &self,
        output: &smithay::output::Output,
    ) -> smithay::desktop::utils::OutputPresentationFeedback {
        let mut feedback = smithay::desktop::utils::OutputPresentationFeedback::new(output);
        for window in self.space.elements() {
            window.take_presentation_feedback(
                &mut feedback,
                // One output, so everything on screen scanned out on it. This
                // is the closure that would have to learn about several.
                |_, _| Some(output.clone()),
                |_, _| smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind::empty(),
            );
        }
        feedback
    }

    /// Every pane on screen, topmost first, with its client if it has one.
    ///
    /// Collected rather than borrowed because drawing needs `&mut` state;
    /// `Window` is a handle, so this is a few pointer copies. The pane's id
    /// comes with it because that is the one thing both kinds of pane share —
    /// a window still waiting for its application is in this list too, and
    /// draws its own scene instead of a client's surface.
    pub(crate) fn on_screen(&self) -> Vec<(crate::pane::PaneId, Option<Window>)> {
        self.panes
            .iter()
            .rev()
            .map(|pane| (pane.id(), pane.client().cloned()))
            .collect()
    }

    /// Whether the compositor is drawing this pane itself.
    pub(crate) fn pane_has_scene(&self, id: crate::pane::PaneId) -> bool {
        self.panes.get(id).is_some_and(Pane::has_scene)
    }

    /// What to write on a pane's frame.
    pub(crate) fn pane_title(&self, id: crate::pane::PaneId) -> String {
        self.panes.get(id).map_or_else(String::new, |pane| {
            if let Some(left) = pane.left() {
                return left.title.clone();
            }
            pane.client().map_or_else(
                || pane.program().unwrap_or_default().to_owned(),
                |window| self.window_title(window),
            )
        })
    }

    /// Whether a pane's frame is drawn focused.
    ///
    /// The keyboard's answer for a window, and the answer a window had when
    /// its client went for one fading out: the keyboard has moved on by then,
    /// and a titlebar that greyed out while it faded would be drawing the
    /// focus change on a window nobody can focus.
    pub(crate) fn looks_focused(&self, id: crate::pane::PaneId) -> bool {
        if let Some(left) = self.panes.get(id).and_then(Pane::left) {
            return left.focused;
        }
        self.focused_window()
            .is_some_and(|window| self.panes.id_of(&window) == Some(id))
    }

    /// Whether the pointer is over a pane, frame included.
    ///
    /// **A question about a rectangle, and deliberately not a hit test.** It is
    /// asked once per frame by `render::chrome`, for
    /// `decoration::Look::pointer_inside` — the flag a titlebar reads to light
    /// a close button up as the cursor crosses it. Nothing routes an event by
    /// it: presses go through `chrome_under` and `window_under`, motion and
    /// buttons through `surface_under`, and all three walk `drawn_at` and
    /// `Frame::covers`.
    ///
    /// That is why it takes `pane_outer_of` rather than the drawn frame, and
    /// why it is right that it does. The alternative was raised by #127's
    /// review and is worth answering once so it is not raised again: were this
    /// gated on `covers` like the walks are, it would still decide nothing
    /// about where input goes, and a pane it answers `true` for while invisible
    /// draws no decoration to light up — `render::chrome` is reached through
    /// the same transform, and a frame at opacity zero paints nothing. Asking a
    /// cheaper question here and the exact one there is the split, not an
    /// oversight in this line.
    pub(crate) fn pointer_over(&self, id: crate::pane::PaneId) -> bool {
        // Never over a window that has gone: a close button lighting up as
        // the cursor crosses a pane fading out offers a press that nothing
        // will take (`a_window_that_left_is_nobodys_to_find`).
        if self.panes.get(id).is_some_and(Pane::ghost) {
            return false;
        }
        let Some(outer) = self.pane_outer_of(id) else {
            return false;
        };
        let Some(pointer) = self.seat.get_pointer() else {
            return false;
        };
        outer.to_f64().contains(pointer.current_location())
    }

    /// Whether the compositor draws this window's frame.
    ///
    /// `Styled` and nothing else: a pane whose frame has not been built is not
    /// decorated *yet*, and one that will never have a frame is not decorated
    /// at all. Both were "not in `frames`" before and are one arm apart now.
    pub(crate) fn is_decorated(&self, window: &Window) -> bool {
        self.panes
            .of(window)
            .is_some_and(|pane| pane.decoration().is_some())
    }

    /// The instant a question about where windows are *going* is put to.
    ///
    /// Far enough ahead that every transform running now has landed. One
    /// function, so the two focus readers cannot drift apart on it again: see
    /// [`SETTLED`] for the rule and the tests that pin it.
    fn settling(&self) -> Duration {
        self.clock.now().saturating_add(SETTLED)
    }

    /// Whether a pane is headed somewhere the user can see it.
    ///
    /// **The two halves `Frame::covers` asks of a point — `shows()` and the
    /// rectangle — asked of the screens instead, and of the destination rather
    /// than the frame being drawn.** `landed` is [`Self::settling`]; [`SETTLED`]
    /// says why and names the tests. It was the present until #127's fourth
    /// review, and that put the question to a restore at its progress zero —
    /// which answers `present::close`'s opacity-zero end, so the window being
    /// given back declined itself — and to a workspace switch before it had
    /// moved anything.
    ///
    /// Through [`Self::drawn_at`] and not `pane_outer`, for one reason: a
    /// hidden workspace is **parked a screen away, not unmapped**. Its windows
    /// keep the rectangle their layout gave them and a selection carries them
    /// off-stage, so the real rectangle says they are on screen and only the
    /// drawn one knows better. See `workspaces.lua`, and
    /// `a_close_does_not_hand_the_keyboard_to_a_workspace_nobody_can_see`.
    ///
    /// **On a screen that draws it, and not merely on a screen.** A pane is
    /// drawn only on the monitors its slot is on -- the renderer's own cull,
    /// [`crate::render::drawn_on`] -- so its drawn rectangle is measured
    /// against those. Measured against every screen, as it was until #134's
    /// third review, the left monitor's hidden workspace -- carried a screen
    /// and a bit to the right by the shipped `spread` -- was on stage on the
    /// right monitor, and `offer_keyboard`, `settle_focus` and an activation
    /// all handed it the keyboard. `nothing_on_stage` has the rule and the
    /// `on_two_monitors_` tests in `keyboard_at_open` have each caller.
    ///
    /// **No screens is not "invisible".** `nothing_on_stage` answers `None`
    /// when there is nothing to measure against, and a compositor with no
    /// output bound yet must not decide that every window is unreachable — the
    /// caller would then refuse to focus anything at all. "Not known to be off
    /// stage" is the honest reading and the safe one.
    fn on_stage(&self, pane: &Pane, screens: &[Rectangle<i32, Logical>], landed: Duration) -> bool {
        let slot = self.pane_outer(pane);
        staged(slot, self.drawn_at(pane, slot, landed), screens)
    }

    /// Whether the selections this pane is in are putting its desk away: on a
    /// workspace other than the one its monitor is showing.
    ///
    /// **Asked of the selections directly**, because that is what a hidden
    /// workspace is. `workspaces.lua` parks a desk with `sol.present_group`
    /// and clears the shift of the desk in view, so the compositor -- which
    /// knows no workspaces -- can still tell a window on a desk nobody is
    /// looking at from one on the desk in front of them, wherever either one's
    /// own frame is. A window on the desk in view is carried by nothing, and a
    /// column scrolled off it is still focused and brought back by the strip.
    ///
    /// **Where the selections are headed, and not where they have got to.**
    /// Until #134's fifth review this was the shift at [`Self::settling`]. A
    /// spring never lands exactly on nothing before the travel is retired
    /// (`Groups::bound_for_window` says why), so with
    /// `workspaces.motion.easing = "spring"` every window on the desk being
    /// switched to was refused for the whole slide
    /// (`on_a_spring_a_genuine_activation_mid_slide_on_the_desk_being_switched_to_takes_the_keyboard`).
    ///
    /// **Put away is carried anywhere, or faded to nothing**, and measured
    /// against no screen: [`put_away`] is the rule, and says why. A selection
    /// that dims a desk or turns it leaves it where the user can see it. One
    /// that fades it to nothing is refused here, before it is focused, where
    /// until #134's fifth review it was focused and then handed off to
    /// whatever `settle_focus` picked
    /// (`a_genuine_activation_of_a_window_a_selection_fades_to_nothing_leaves_the_keyboard_exactly_where_it_was`).
    ///
    /// Until #134's fourth review this was inferred from where the frame
    /// lands -- the pane's own frame on stage and its carried one off -- which
    /// missed every window on a hidden desk whose own frame was already off
    /// stage: a column scrolled off it, and a window being closed there.
    /// `a_genuine_activation_of_a_column_scrolled_off_a_hidden_workspace_leaves_the_keyboard_exactly_where_it_was`,
    /// `a_genuine_activation_of_a_window_being_closed_on_a_hidden_workspace_leaves_the_keyboard_exactly_where_it_was`,
    /// and on the desk in view
    /// `a_genuine_activation_of_a_column_scrolled_off_screen_brings_it_back_with_the_keyboard`.
    fn carried_by_a_selection(&self, pane: crate::pane::PaneId) -> bool {
        if self.groups.is_empty() {
            return false;
        }
        let Some(pane) = self.panes.get(pane) else {
            return false;
        };
        let Some(output) = self.output_of(self.pane_outer(pane)) else {
            return false;
        };
        // The monitor `carried_at` would name, so a selection of a monitor
        // holds the pane here exactly when it carries it there.
        let monitor = self.groups.names_monitors().then(|| output.name());
        let bound = self
            .groups
            .bound_for_window(pane.id().get(), monitor.as_deref());
        put_away(bound)
    }

    /// [`Self::on_stage`] for one pane, asked by id at [`Self::settling`]: for
    /// a caller with a single question rather than a walk to hoist the
    /// screens out of. A pane that is not there is not on stage.
    fn pane_on_stage(&self, pane: crate::pane::PaneId) -> bool {
        let landed = self.settling();
        let screens = self.screens();
        self.panes
            .get(pane)
            .is_some_and(|held| self.on_stage(held, &screens, landed))
    }

    /// Drop a queued capture whose frame has gone away.
    pub(crate) fn forget_capture(
        &mut self,
        frame: &smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
    ) {
        self.pending_captures
            .retain(|capture| &capture.frame != frame);
    }

    /// The window with keyboard focus, if any.
    ///
    /// A focused *popup* answers with the window it belongs to, because that is
    /// what everything asking this question means. A popup grab moves keyboard
    /// focus onto the menu's own surface -- that is how a menu receives Escape
    /// and arrow keys -- but `window_for` matches toplevels only, so without
    /// this the answer would be `None` for as long as a menu is open.
    ///
    /// Three things read it and all three were wrong that way: a titlebar drew
    /// unfocused on every right-click, `snapshot`'s `focused` went null so a
    /// Lua layout saw no focused window, and `settle_focus` treated the gap as
    /// "nothing is focused" and moved the selection. The menu is a part of its
    /// window, not a rival to it.
    pub(crate) fn focused_window(&self) -> Option<Window> {
        let surface = self.seat.get_keyboard()?.current_focus()?;
        if let Some(window) = self.window_for(&surface) {
            return Some(window);
        }
        // Only a popup has a root to find; for anything else this is the same
        // `None` the line above already produced.
        let popup = self.popups.find_popup(&surface)?;
        let root = find_popup_root_surface(&popup).ok()?;
        self.window_for(&root)
    }

    pub(crate) fn is_focused(&self, window: &Window) -> bool {
        self.focused_window().as_ref() == Some(window)
    }

    /// Give a newly mapped client a pane, and answer with its id.
    ///
    /// Where a window enters the compositor. `sync_panes` would notice it at
    /// the next refresh anyway; doing it here is what makes the id exist for
    /// the script that is about to be told the window opened.
    pub(crate) fn take_pane(&mut self, window: Window) -> u64 {
        let slot = self.real_geometry(&window).unwrap_or_default();
        self.panes.mapped(window, slot, self.clock.now()).get()
    }

    /// A pane for a surface that places itself: a menu, a tooltip, a drag icon.
    ///
    /// Unmanaged, so no layout ever sees it, and bare, so it never grows a
    /// titlebar. Both matter: without the first, dragging a text selection out
    /// of an application reflows the whole desktop to make room for the drag
    /// icon; without the second, a tooltip gets a title bar.
    pub(crate) fn take_unmanaged_pane(&mut self, window: Window) {
        let slot = self.real_geometry(&window).unwrap_or_default();
        let id = self.panes.mapped(window, slot, self.clock.now());
        if let Some(pane) = self.panes.get_mut(id) {
            pane.unmanage();
        }
        self.decorations.set_bare(&mut self.panes, id);
    }

    /// Which process a client belongs to, as the kernel reports it.
    ///
    /// The compositor's own view of who is on the other end of the socket, not
    /// anything the client said about itself.
    fn client_pid(&self, window: &Window) -> Option<u32> {
        let surface = window.wl_surface()?;
        let client = surface.client()?;
        let credentials = client.get_credentials(&self.display_handle).ok()?;
        u32::try_from(credentials.pid).ok()
    }

    /// Move a client into the window that was opened for its launch.
    ///
    /// The late half of adoption. `new_toplevel` matches on the process and
    /// gets it right for anything that stays as the process we spawned; a
    /// program whose launcher forks and exits breaks that chain and opens a
    /// window of its own. When it then activates with the token we gave it,
    /// this puts it where it belonged: the window it was already in is retired
    /// and its content moves to the one that has been waiting.
    ///
    /// Returns whether the token was this window's own -- it is in the window
    /// the token was minted for, now or already -- so that only a token that
    /// is not falls through to being an ordinary request for focus.
    ///
    /// **Already** is the ordinary case, and it is asked first. An application
    /// that is itself the process `sol.spawn` started was adopted by its pid
    /// in `new_toplevel`, so by the time it activates with the token it was
    /// handed -- GTK, Qt and winit all do, alacritty among them -- its window
    /// is no longer loading. Asked after the loading test, as it was, this
    /// answer was never reached for any such window: the token fell through,
    /// `request_activation` focused the window wherever it was, and with
    /// `follow_overflow = false` that was a workspace nobody is looking at
    /// (#134 review). The keyboard for a launched window is
    /// [`Self::offer_keyboard`]'s to decide, at its first frame, and a token
    /// that only says "this is the window you opened for me" is not a second
    /// opinion. Nor a brief one: `request_activation` now takes the keyboard
    /// back off a window nobody can see, but a window focused on the way has
    /// still been told it had the keyboard and the clipboard, and the scripts
    /// that it was focused. `a_launched_window_parked_on_a_hidden_workspace_does_not_take_the_keyboard_by_its_own_token`
    /// sends the token before the first frame, as winit does, and after, and
    /// asserts both; `a_launched_window_on_screen_takes_the_keyboard_when_it_activates_with_its_own_token`
    /// is the other half.
    fn claim_into(&mut self, pane: crate::pane::PaneId, surface: &WlSurface) -> bool {
        let Some(window) = self.window_for(surface) else {
            return false;
        };
        let Some(wrong) = self.panes.id_of(&window) else {
            return false;
        };
        if wrong == pane {
            return true;
        }
        // Still waiting, or already given up on.
        if !self.panes.get(pane).is_some_and(Pane::is_loading) {
            return false;
        }

        if let Some(held) = self.panes.get_mut(pane) {
            held.adopt(window.clone());
        }
        // The pane it opened in goes, and with it the frame and the id nothing
        // should have learned. Retired rather than left empty: `sync_panes`
        // would drop it anyway, and the layout is told now rather than a frame
        // late.
        //
        // **At once, with no fade, and removed before it is told** -- the one
        // way a window leaves that does not go through `Self::depart` (#126).
        // It is a merge, not a close: the client it held is still on screen,
        // in the pane above, and a fade here would draw a second copy of it
        // leaving. Removed first because for this one line both panes hold
        // the same `Window`, and `close`'s snapshot would list it twice.
        self.panes.remove(wrong);
        self.trigger_close(wrong);
        tracing::debug!(
            pane = pane.get(),
            was = wrong.get(),
            "an application arrived in its window, by token"
        );
        // **The window it arrived in may be one nobody can see**, and the
        // window that moved into it may already have the keyboard: it opened on
        // screen, as a window of its own, and was offered it there. The
        // keyboard does not follow it onto a hidden workspace; `hand_off_keyboard`
        // does nothing for a window that does not have it.
        // `a_focused_window_claimed_into_a_pane_on_a_hidden_workspace_gives_the_keyboard_up`.
        if !self.pane_on_stage(pane) {
            self.hand_off_keyboard(&window);
        }
        self.redraw = true;
        true
    }

    /// Give a mapped client to the window that was opened for it, or open a
    /// new one.
    ///
    /// The whole point of the refactor arrives here. A client whose process is
    /// the one a window has been waiting for becomes that window's content:
    /// same id, same slot, same frame with the same animation still running in
    /// it. Nothing is created and nothing is replaced, so nothing downstream
    /// ever learns that the window used to be empty.
    ///
    /// Everything that can go wrong ends in an ordinary window. A client that
    /// re-execs or forks past the ancestor walk, one whose window was closed
    /// while it was still starting, one nobody asked for — each of them opens
    /// the old way. A missed adoption is a window that appears normally; it is
    /// never a window that is lost.
    ///
    /// Must happen where the window is mapped rather than later: `sync_panes`
    /// gives any client it finds without a pane one of its own, and by then
    /// there would be two windows for one application.
    fn adopt_or_open(&mut self, window: Window) -> u64 {
        let waiting = match self.client_pid(&window) {
            Some(pid) => self.panes.awaiting(&ancestry(pid)),
            None => None,
        };
        let Some(id) = waiting else {
            return self.take_pane(window);
        };
        let Some(pane) = self.panes.get_mut(id) else {
            return self.take_pane(window);
        };
        pane.adopt(window);
        let slot = pane.slot();
        tracing::debug!(pane = id.get(), "an application arrived in its window");

        // Told its size straight away, rather than on its first commit. A
        // client that learns its size only after it has drawn paints one frame
        // at a size it chose for itself, and that frame is visible -- so the
        // window that has been standing there at the right size all along
        // flickers to the wrong one and back at the exact moment it fills.
        if let Some(window) = self.panes.get(id).and_then(Pane::client).cloned() {
            size_window(&window, slot);
            self.map_stacked(window, slot.loc, false);
        }
        id.get()
    }

    /// Bring the compositor's own view of its windows back in line with the
    /// space, and say whether the set of windows changed.
    ///
    /// Called where the space is refreshed, which is once per frame. That is
    /// also the moment Smithay drops elements whose client has died — the only
    /// notice we get for a window that went away without telling anyone.
    pub(crate) fn sync_panes(&mut self) -> bool {
        // A scene whose application has painted has been replaced, and can go.
        //
        // Here rather than where a client is first shown, because that is not
        // the only way to arrive: an X11 window takes a different path, and a
        // client adopted after its first commit takes none at all. Asking the
        // question once a frame, about every pane, cannot miss one -- and a
        // scene kept past its moment is a window that never shows its
        // application.
        let now = self.clock.now();
        let fade = self.loading.fade;
        let ready: Vec<crate::pane::PaneId> = self
            .panes
            .iter()
            .filter(|pane| pane.has_scene())
            .filter(|pane| {
                pane.client()
                    .is_some_and(|window| self.client_ready(window))
            })
            .map(Pane::id)
            .collect();
        for id in ready {
            // The application has painted. The scene stays on screen over it,
            // fading, so what appears underneath is already the application
            // rather than a hole it then fills.
            if self.panes.get_mut(id).is_some_and(|pane| pane.fade(now)) {
                tracing::debug!(pane = id.get(), "an application filled its window");
                self.redraw = true;
            }
            if self.panes.get(id).is_some_and(|pane| pane.faded(now, fade))
                && let Some(pane) = self.panes.get_mut(id)
            {
                pane.filled();
            }
        }

        // The space's answer per window, except for one being dragged: while an
        // edge drag is live the slot is the authority and the space's size is
        // whatever the client last committed, so copying it back would undo the
        // drag between one frame and the next. Its own slot is substituted
        // rather than the pane being skipped, so `Panes::sync` still sees every
        // window in the space and in the space's order — the ordering is the
        // other half of what this call is for.
        let stack: Vec<(Window, Rectangle<i32, Logical>)> = self
            .space
            .elements()
            .filter_map(|window| {
                let rect = match self.held_slot(window) {
                    Some(slot) => slot,
                    None => self.real_geometry(window)?,
                };
                Some((window.clone(), rect))
            })
            .collect();
        if !self.panes.sync(&stack, self.clock.now()) {
            return false;
        }

        // Nothing to reconcile. A pane's frame and its two timers are fields
        // of the pane, so `Panes::sync` above took them with the panes it
        // dropped -- which is the whole point of moving them in.
        //
        // What stood here was a `retain` over the decoration tables against
        // the set of live panes, and before that the same sweep was done where
        // a window was seen leaving *tidily*: nothing there knew the set of
        // live windows, so a client that crashed left its frame behind for
        // ever. A frame that is part of its window cannot be left behind by
        // either route.

        // A window appearing or going is exactly when the keyboard can be left
        // with nowhere to be, and the only moment worth checking.
        self.settle_focus();
        true
    }

    /// Put a window in the stack, and keep whatever is waiting on it above it.
    ///
    /// **The one way a managed window is mapped or raised.** Use this rather
    /// than `self.space.map_element`, which cannot know the one thing that has
    /// to be true afterwards.
    ///
    /// A modal dialog is the window that is holding its parent up: "Discard
    /// changes?" is the only thing on screen you are allowed to answer, and the
    /// document behind it is not going to accept a keystroke until you have. So
    /// a modal has to be *above* the window it belongs to, and nothing in the
    /// stack says so on its own. Focusing the parent raised it — one click on a
    /// strip of it left showing, or one pointer crossing with
    /// `focus_follows_mouse` — and the prompt went behind the window that was
    /// waiting on the answer, where it cannot be found and cannot be dismissed.
    ///
    /// **Here rather than in `focus_window`**, which is where the symptom was
    /// seen and not where the cause is. Every raise has the same effect, and
    /// there are a dozen: a click, a fullscreen, a maximise, a client mapping,
    /// a layout pass. `Space::map_element` takes the element out of the stack
    /// and pushes it back on top whatever `activate` says — that flag only
    /// decides who is told they are focused — so *every* call is a restack, and
    /// a rule kept at one caller is a rule broken at eleven.
    ///
    /// This is also why it is not a z-index: a modal belongs above its own
    /// parent, not above everybody, and a second window's prompt has no claim
    /// over the first window's.
    pub(crate) fn map_stacked(
        &mut self,
        window: Window,
        location: impl Into<Point<i32, Logical>>,
        activate: bool,
    ) {
        self.space.map_element(window.clone(), location, activate);
        self.lift_modals_over(&window);
    }

    /// Map a window a layout has placed, raised as [`Self::map_stacked`]
    /// raises it -- but never past a window that is leaving, or drawn at less
    /// than full opacity.
    ///
    /// **Issue #128's review, findings 2 and 6.** Every placement is a raise,
    /// and a layout places the windows it moves one after another, so the
    /// stack after a sweep was the sweep's order. Since #128 a close reflows at
    /// once: the neighbour growing into the space of a window that is fading
    /// there was placed and the fading window was not, so the neighbour was
    /// stacked over the fade and covered most of it -- and the refused window
    /// fading back in was covered the same way by the neighbour giving its
    /// space back. See
    /// `a_closing_window_fades_in_front_of_the_neighbour_moving_into_its_space`
    /// and `a_refused_window_fades_back_in_front_of_the_neighbour_making_room`.
    ///
    /// **"Not past" rather than "raise those again afterwards".** What goes
    /// back on top is the lowest such window above this one *and everything
    /// that was above it*, in the order they were in, so the placed window
    /// ends just below it and the windows above it keep their order. Raising
    /// only the fading window would lift it over windows it had been under.
    ///
    /// Only for layout placements. A click, a new window and a fullscreen raise
    /// through `map_stacked` as they always have: those mean "this one, in
    /// front", and a sweep means nothing about the stack at all.
    fn map_laid_out(&mut self, window: Window, location: Point<i32, Logical>, now: Duration) {
        // Collected before anything moves, bottom to top: everything from the
        // lowest window above this one that must stay above it, upward.
        let kept_over: Vec<Window> = self
            .space
            .elements()
            .skip_while(|element| **element != window)
            .skip(1)
            .skip_while(|element| !self.stays_over_a_layout(element, now))
            .cloned()
            .collect();
        self.map_stacked(window, location, false);
        for element in &kept_over {
            self.space.raise_element(element, false);
        }
    }

    /// Whether a layout placing another window must leave this one above it:
    /// a window being closed, from the press until it is given back or gone,
    /// and one drawn translucent -- which is where a refused window is from
    /// the moment it is given back until its return lands.
    fn stays_over_a_layout(&self, window: &Window, now: Duration) -> bool {
        self.panes.of(window).is_some_and(|pane| {
            pane.leaving() || present::frame(pane, self.pane_outer(pane), now).opacity < 1.0
        })
    }

    /// Put every modal waiting on this window back above it, and their own
    /// modals above them.
    ///
    /// The chain is not hypothetical: a file chooser is modal for the document
    /// and its "Replace?" prompt is modal for the chooser, so raising the
    /// document has to lift two windows and in that order.
    ///
    /// `lifted` is what stops a client that names a cycle of parents — which
    /// neither `xdg_toplevel.set_parent` nor `WM_TRANSIENT_FOR` forbids — from
    /// walking for ever: each window is raised at most once.
    fn lift_modals_over(&mut self, window: &Window) {
        let Some(pane) = self.panes.id_of(window) else {
            return;
        };
        let mut over = vec![pane];
        let mut lifted: Vec<crate::pane::PaneId> = Vec::new();
        while let Some(parent) = over.pop() {
            // Collected before anything moves: raising borrows the space, and
            // restacking the list being walked is how one gets skipped.
            let children: Vec<Window> = self
                .space
                .elements()
                .filter(|element| self.is_modal(element))
                .filter(|element| self.parent_of(element) == Parentage::Window(parent.get()))
                .cloned()
                .collect();
            for child in children {
                let Some(id) = self.panes.id_of(&child) else {
                    continue;
                };
                if lifted.contains(&id) {
                    continue;
                }
                lifted.push(id);
                // `false`: this is about the stack, not about focus. A dialog
                // lifted because its parent was clicked has not been clicked.
                self.space.raise_element(&child, false);
                over.push(id);
            }
        }
    }

    /// Run whatever a key combination is bound to, and apply what it asked for.
    pub(crate) fn trigger(&mut self, combo: &str) -> bool {
        let snapshot = self.snapshot();
        // Taken out for the call so no part of the compositor is borrowed while
        // Lua runs, and a script cannot re-enter the seat mid-dispatch.
        let Some(mut scripts) = self.scripts.take() else {
            return false;
        };
        let outcome = scripts.key(combo, snapshot);
        self.scripts = Some(scripts);

        let handled = outcome.handled;
        self.apply(outcome);
        handled
    }

    /// Give a pointer press to the mode that owns input.
    pub(crate) fn trigger_click(&mut self, x: f64, y: f64) -> bool {
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return false;
        };
        let outcome = scripts.click(x, y, snapshot);
        self.scripts = Some(scripts);

        let handled = outcome.handled;
        self.apply(outcome);
        handled
    }

    /// Apply what a script asked for.
    fn apply(&mut self, outcome: Outcome) {
        // Anything a script asked for changes what is on screen, and almost
        // all of it starts an animation. Damage-driven rendering only draws
        // when something says it must, and a transform created here says
        // nothing on its own — so without this the animation does not advance
        // until some *unrelated* damage happens to wake the loop, at which
        // point it jumps straight to wherever the clock says it should be.
        //
        // That is the whole of "sometimes the animation is instant, sometimes
        // too fast, sometimes right": it was running at the mercy of whatever
        // else happened to be redrawing.
        if !outcome.commands.is_empty() {
            self.redraw = true;
        }

        if let Some(grab) = outcome.grab
            && grab != self.script_grab
        {
            self.script_grab = grab;
            tracing::debug!(grab, "script input grab changed");
        }
        if let Some(status) = outcome.status
            && status != self.status
        {
            tracing::debug!(status, "mode changed");
            self.status = status;
        }

        let now = self.clock.now();
        for command in outcome.commands {
            match command {
                Command::Present {
                    id,
                    rect,
                    opacity,
                    matrix,
                    deform,
                    z,
                    pivot,
                    animation,
                } => {
                    let Some(pane) = self.panes.by_script_id(id) else {
                        continue;
                    };
                    let outer = self.pane_outer(pane);
                    let rect = rect.map_or_else(
                        || outer.to_f64(),
                        |rect| present::logical((rect.x, rect.y), (rect.w, rect.h)),
                    );
                    let target = Frame {
                        matrix: matrix.unwrap_or(crate::mat4::Mat4::IDENTITY),
                        rect,
                        // A script's rectangle is a picture of the window it
                        // moves there -- a thumbnail is the window made small
                        // -- so it is zoomed by what it is over the window's
                        // own rectangle. See `Frame::zoom`.
                        zoom: Frame::zoom_of(rect, outer),
                        opacity: opacity.unwrap_or(1.0),
                        deform: deform.and_then(|deform| self.aimed(&deform)),
                        // Both arrive resolved: `script::depth_from` and
                        // `script::pivot_from` hold the defaults, so a table
                        // mentioning neither key produces them there rather
                        // than here. Still spelled out rather than
                        // `..Frame::real(outer)`, because a struct update
                        // would take whatever field is added next without
                        // anyone looking at this line again.
                        z,
                        pivot,
                    };
                    present::present(
                        pane,
                        outer,
                        target,
                        now,
                        animation.duration,
                        animation.easing,
                    );
                }
                Command::PresentFrom {
                    id,
                    rect,
                    opacity,
                    animation,
                } => {
                    let Some(pane) = self.panes.by_script_id(id) else {
                        continue;
                    };
                    let outer = self.pane_outer(pane);
                    let rect = present::logical((rect.x, rect.y), (rect.w, rect.h));
                    let start = Frame {
                        matrix: crate::mat4::Mat4::IDENTITY,
                        rect,
                        // The window it grows into, drawn smaller: `open.lua`
                        // shrinks the window's own rectangle. See `Frame::zoom`.
                        zoom: Frame::zoom_of(rect, outer),
                        opacity: opacity.unwrap_or(1.0),
                        deform: None,
                        // Explicit so the next field added breaks this line
                        // instead of being defaulted past it -- and these two
                        // stay the defaults because `sol.present_from` reads
                        // no keys for them. It could not honour them if it
                        // did: this is the frame a window animates *from*, and
                        // both fields select the destination's value at the
                        // first blended frame, so a depth or a pivot here
                        // would never be on screen. A script wanting either
                        // says so with `sol.present` once it has landed.
                        z: 0.0,
                        pivot: (0.5, 0.5),
                    };
                    present::from(
                        pane,
                        outer,
                        start,
                        now,
                        animation.duration,
                        animation.easing,
                    );
                }
                Command::Clear { id, animation } => {
                    let Some(pane) = self.panes.by_script_id(id) else {
                        continue;
                    };
                    let outer = self.pane_outer(pane);
                    // Deliberately discarded, unlike `give_back`'s. A script
                    // clearing a transform is restated by the next call through
                    // this queue, and nothing here retires a piece of state that
                    // the clear is the only way out of -- which is the whole of
                    // why `clear` reports at all.
                    let _ = present::clear(pane, outer, now, animation.duration, animation.easing);
                }
                Command::Focus { id } => {
                    if let Some(window) = self.window_by_id(id) {
                        tracing::debug!(id, title = self.window_title(&window), "script focused");
                        self.focus_window(&window, SERIAL_COUNTER.next_serial());
                    }
                }
                Command::Place {
                    id,
                    rect,
                    animation,
                    tile,
                } => {
                    let standing = if tile { Standing::Tile } else { Standing::Free };
                    self.place(id, rect, animation, now, standing);
                }
                Command::Unplace { id } => {
                    if let Some(pane) = self.panes.by_script_id(id).map(Pane::id)
                        && let Some(pane) = self.panes.get_mut(pane)
                    {
                        pane.untile();
                    }
                }
                Command::Close { id } => {
                    if let Some(pane) = self.panes.by_script_id(id).map(Pane::id) {
                        self.close_pane(pane);
                    }
                }
                Command::Loading(loading) => {
                    if self.loading != loading {
                        tracing::debug!(?loading, "loading behaviour set");
                        self.loading = loading;
                    }
                }
                Command::Resize(resizing) => {
                    if self.resizing != resizing {
                        tracing::debug!(?resizing, "resize behaviour set");
                        self.resizing = resizing;
                    }
                }
                Command::Cursor(configured) => {
                    // The environment is re-read here rather than cached at
                    // startup, because this also runs on `super+shift+r` and a
                    // reload is the one moment a session can pick up an
                    // `XCURSOR_THEME` that was exported after the compositor
                    // started. Two `env::var` calls per reload.
                    //
                    // And a reload that really did change the pointer damages
                    // the screen. The pointer is rebuilt for every output on
                    // every *frame* (see `render::cursor`), which is not the
                    // same as there being a frame: both backends draw on
                    // damage, and a pointer sitting still produces none. So a
                    // `super+shift+r` that changed only the cursor theme or
                    // size would otherwise show the new pointer whenever
                    // something unrelated next happened to redraw — which,
                    // while trying a theme out, is when the mouse is jiggled.
                    //
                    // `configure` answers `false` when nothing changed, which
                    // on a reload that changed a keybinding is every time, so
                    // the ordinary reload still schedules nothing.
                    if self
                        .pointer
                        .configure(&configured, &crate::cursor::theme::Environment::read())
                    {
                        self.redraw = true;
                    }
                }
                Command::Decoration { name } => {
                    // The slots windows occupy are kept; what changes is how
                    // much of each slot the frame takes, so every client is
                    // resized to whatever the new decoration left it.
                    let slots: Vec<(Window, Rectangle<i32, Logical>)> = self
                        .space
                        .elements()
                        .cloned()
                        .collect::<Vec<_>>()
                        .into_iter()
                        .filter_map(|window| {
                            self.outer_geometry(&window).map(|outer| (window, outer))
                        })
                        .collect();
                    if self.decorations.set_style(&mut self.panes, name) {
                        for (window, outer) in slots {
                            self.resize_to(&window, outer);
                        }
                        self.redraw = true;
                        // Resizing each window in place keeps a floating
                        // arrangement looking right, but a tiled one is the
                        // layout's arithmetic and only the layout can redo it.
                        self.trigger_relayout();
                    }
                }
                Command::Spawn { program, args } => self.spawn(&program, &args),
                Command::Reload => self.request = Some(Request::Reload),
                Command::Keyboard(request) => {
                    if crate::keymap::apply(self, &request) {
                        let now = crate::keymap::describe(self);
                        tracing::info!(
                            layouts = ?now.layouts,
                            active = now.active,
                            repeat = format!("{}/s after {}ms", now.repeat_rate, now.repeat_delay),
                            "keyboard"
                        );
                    }
                }
                Command::Surface(surface) => self.declare_surface(*surface),
                Command::SurfaceGone(name) => self.remove_surface(&name),
                Command::Group {
                    name,
                    selection,
                    animation,
                } => {
                    let displaced = match selection {
                        Some(selection) => {
                            let selection = crate::group::selection_of(&selection, &self.surfaces);
                            self.groups.declare(&name, selection, now)
                        }
                        None => self.groups.forget(&name, now),
                    };
                    self.keep_displaced(&displaced, now, animation);
                }
                Command::PresentGroup {
                    name,
                    to,
                    animation,
                } => {
                    if !self
                        .groups
                        .present(&name, to, now, animation.duration, animation.easing)
                    {
                        // Named rather than ignored, for the reason
                        // `sol.surface` names a scene it cannot find: a
                        // transform on a selection nobody declared is a typo,
                        // and a mode that silently does nothing is the hardest
                        // kind of configuration mistake to find.
                        tracing::warn!(group = name, "no selection by that name to carry");
                    }
                }
                Command::ClearGroup { name, animation } => {
                    self.groups
                        .clear(&name, now, animation.duration, animation.easing);
                }
                Command::Monitors(arrangement) => {
                    let was = std::mem::replace(&mut self.arrangement, arrangement);
                    // `enabled = false` on a monitor is an unplug as far as
                    // everything downstream is concerned, and `enabled = true`
                    // is a plug -- so the backend is asked to look again
                    // rather than this growing its own way to drop a screen.
                    // Nothing to do nested: there are no connectors there.
                    if was.enablement() != self.arrangement.enablement() {
                        self.rescan_outputs = true;
                    }
                    // Applied immediately, and applied again on reload, so
                    // moving a monitor is `super+shift+r` rather than logging
                    // out. Anything already placed is now measured against a
                    // different work area, which is why the layout is asked to
                    // run again.
                    self.place_outputs();
                    self.trigger_relayout();
                    self.redraw = true;
                }
                Command::Quit => {
                    tracing::info!("a script asked to stop");
                    self.request = Some(Request::Quit);
                }
            }
        }
    }

    /// Whether a selection is carrying every window off every screen.
    ///
    /// **The recovery path is part of issue #116.** The session that found it
    /// had every window drawn two screen-widths to the left, nothing on any
    /// monitor but a wallpaper, and no key that brought it back — `super+1`
    /// early-returned because the scripts believed workspace 1 was already in
    /// view. What would have saved it was one line saying where everything had
    /// gone. The compositor knew; nothing asked it.
    ///
    /// **The compositor cannot tell a lost desktop from an empty workspace,
    /// and does not pretend to.** They are the same picture: in both, every
    /// window is in a selection carried off screen and the desk in view has
    /// none. The difference is intent, and intent lives in the scripts. So
    /// this is asked at the one moment where the answer is worth having
    /// either way — the end of [`Self::reload`], which is both the keypress
    /// that lost the desktop and the keypress anyone reaches for when
    /// something on screen has gone wrong. Asking it on every script event
    /// instead would warn about every empty workspace anybody switched to,
    /// and a warning that is usually wrong is one nobody reads.
    ///
    /// `None` when nothing is grouped, when there are no screens, or when
    /// there are no windows — none of which is a question with an answer.
    ///
    /// Measured through [`Self::drawn_at`], which is the same function the
    /// renderer uses to place a pane: asking `Groups` for the offset
    /// separately would be a second answer to "where is this window", and two
    /// answers drift.
    ///
    /// **Sampled [`SETTLED`] ahead, and the first version was not.** It asked
    /// `self.clock.now()` the instant after the three dispatches, which is the
    /// instant every group transform they started is at *progress zero* — so it
    /// measured where the previous session had left the desks rather than what
    /// this reload was doing with them. Wrong both ways round: a reload that
    /// rescued an off-stage desktop printed the warning anyway, and one that
    /// carried the desktop off went quiet. Only a displacement from a
    /// membership change, which is instantaneous, came out right, and that is
    /// the case the fault hid behind (#116 review).
    fn everything_is_off_stage(&self) -> Option<bool> {
        if self.groups.is_empty() {
            return None;
        }
        let screens: Vec<Rectangle<i32, Logical>> = self
            .space
            .outputs()
            .filter_map(|output| self.space.output_geometry(output))
            .collect();
        // Having no screens is `nothing_on_stage`'s answer to give, and it does
        // -- a second check here would be a second place that decides what an
        // unanswerable question comes back as.
        let landed = self.settling();
        nothing_on_stage(
            self.panes.iter().filter(|pane| pane.managed()).map(|pane| {
                let slot = self.pane_outer(pane);
                (slot, self.drawn_at(pane, slot, landed).rect)
            }),
            &screens,
        )
    }

    /// The compositor's own chrome under `location`, if any.
    ///
    /// **The single hit test behind both what a press does and what the pointer
    /// looks like, and that is the whole of issue #108.** The two regions
    /// overlap — the outer eight pixels of a titlebar are inside the top resize
    /// border — and before this there was nothing that resolved the overlap
    /// once. A press resolved it by asking `frame_under` first and
    /// `resize_target` second, so the band below a window's top edge moved the
    /// window. The pointer resolved it not at all: the compositor asked for no
    /// cursor but the default, so whatever a client had last set stayed on
    /// screen, and a CSD toolkit that names a resize shape for its own shadow
    /// margin left a resize arrow sitting over a band that moves. The pointer
    /// was not merely missing a shape; it was confidently describing a
    /// different action from the one a press would take.
    ///
    /// Callers get the answer and never the ingredients, so a second, parallel
    /// hit test for the cursor cannot be written by accident — which is the
    /// failure mode this shape is chosen against, because two hit tests drift
    /// and the bug comes back wearing a different face.
    ///
    /// **One walk, topmost first, and the first pane that claims the point with
    /// something it *draws* ends it — including when what it claims is "my
    /// client owns this".** That last case is issue #111 and is what
    /// [`topmost_chrome`] exists to state. This walk used to ask every pane for
    /// a [`Chrome::Frame`] and then every pane again for a [`Chrome::Resize`],
    /// and in neither pass could a pane stop the descent by *covering* the
    /// point: [`Self::pane_chrome`] returned the same `None` for "the point is
    /// on my client" as for "the point is nowhere near me". So a press on the
    /// top window, at a spot where a lower window's titlebar lay underneath,
    /// raised and focused the lower window — a titlebar taking clicks through
    /// whatever covered it.
    ///
    /// "Something it draws" is the qualification the first fix was missing: a
    /// resize border hanging in the empty margin outside its own window claims
    /// the point only against bare desktop, and yields to whatever a lower pane
    /// paints there. [`topmost_chrome`] has the argument.
    pub(crate) fn chrome_under(&self, location: Point<f64, Logical>) -> Option<Under> {
        // A titlebar is the compositor's own surface, so it would otherwise
        // still take clicks with the session locked -- close and maximise
        // included. The resize border used to sit outside this guard, since
        // `resize_target` walked the panes itself and asked nothing: a press
        // near where a window's edge used to be started a resize grab on a
        // locked screen, and the window was still that size when the session
        // unlocked. There is one guard now because there is one hit test.
        if self.lock.is_some() {
            return None;
        }
        let now = self.clock.now();
        let screens = self.screens();

        // `rev` because `panes` is in stacking order, bottom-first, and the
        // rule is topmost-first -- which is now load-bearing in a way it was
        // not before, since the first pane to cover the point ends the walk,
        // and a halo is only kept until a lower pane is found drawing under it.
        topmost_chrome(
            self.panes
                .iter()
                .rev()
                .map(|pane| self.pane_chrome(pane, location, now, &screens)),
        )
    }

    /// What one pane's chrome makes of a point.
    ///
    /// **Two regions in two coordinate spaces, and both of those spaces are
    /// deliberate.** The frame's band is hit-tested in the pane's *own*
    /// coordinates, because the frame is rasterised at its unscaled size: a
    /// titlebar drawn at two-thirds size in overview must still be measured
    /// against the QML that was drawn at full size, or its buttons move out
    /// from under the cursor. The resize border is hit-tested where the window
    /// is *drawn*, because a window in a mode should be resized by its
    /// thumbnail's edge or not at all, never by an edge that is somewhere else
    /// on screen. Both were already true separately; what was missing is that
    /// they meet, and [`chrome_of`] is where they are reconciled.
    ///
    /// `pane_outer` rather than `outer_geometry`, which is what the resize half
    /// used to reach for. They agree for a mapped, sized window and differ for
    /// one that has mapped and not yet answered a size, where the space reports
    /// a rectangle of nothing and the pane's slot is still the truth. Every
    /// other hit test in this file already went through `pane_outer`; this is
    /// the one that did not.
    ///
    /// **Four answers rather than two, which is issue #111 and its
    /// correction.** A pane covering the point with its client says so
    /// ([`PaneHit::Client`]) instead of declining, because declining is what a
    /// pane the point misses entirely does and [`Solium::chrome_under`]'s walk
    /// has to tell those apart. A pane with no geometry yet is a
    /// [`PaneHit::Miss`]: it draws nothing, so there is nothing for it to cover
    /// the point with. And chrome the pane claims *outside* what it draws is a
    /// [`PaneHit::Halo`], which is a claim the walk may yet overrule — the one
    /// question `covers` answers that the chrome tests cannot.
    ///
    /// `drawn.rect.contains(location)` is what `covers` is, in both places it
    /// is asked: the frame band already required it, and [`pane_hit_of`] grades
    /// the resize border by the same rectangle. Not `outer`, and not the
    /// client rect — where a pane is *drawn* is where it paints, which in a
    /// mode is its thumbnail and nowhere near where the window lives.
    ///
    /// What a pane may offer at all is [`chrome_offered`]'s, including the
    /// `managed` gate: an unmanaged pane occludes like any other and offers no
    /// chrome whatsoever.
    fn pane_chrome(
        &self,
        pane: &Pane,
        location: Point<f64, Logical>,
        now: std::time::Duration,
        screens: &[Rectangle<i32, Logical>],
    ) -> PaneHit<Under> {
        let outer = self.pane_outer(pane);
        // On a screen that does not draw the pane, it has nothing there to
        // press and nothing to occlude with -- a `Miss`, as an invisible pane
        // is below, and not a `Halo`, which would still win over bare desktop.
        // See [`shown_at`];
        // `on_two_monitors_a_press_on_the_right_monitor_reaches_what_it_draws`
        // asks both. Nor has what is left of a window whose client has gone:
        // `a_window_that_left_is_nobodys_to_find`.
        if !shown_at(outer, pane.ghost(), location, screens) {
            return PaneHit::Miss;
        }
        let drawn = self.drawn_at(pane, outer, now);
        let in_outer = present::to_window_space(drawn, outer, location) - outer.loc.to_f64();

        // Only a *built* frame has a band to press. A pane reserving room for
        // one that has not arrived reports insets -- `insets_of` answers for
        // `Frame::Pending` on purpose, so the window does not change shape the
        // moment its frame appears -- but there is no titlebar there yet for a
        // click to land on, and there never was: this is the `decoration()?`
        // that gated `frame_under`.
        let framed = pane.decoration().is_some()
            && drawn.covers(location)
            && on_frame(outer.size, self.insets_of(pane.id()), in_outer);

        #[expect(
            clippy::cast_possible_truncation,
            reason = "a drawn rect is screen-sized"
        )]
        let drawn_rect = Rectangle::new(
            (
                drawn.rect.loc.x.round() as i32,
                drawn.rect.loc.y.round() as i32,
            )
                .into(),
            (
                drawn.rect.size.w.round() as i32,
                drawn.rect.size.h.round() as i32,
            )
                .into(),
        );
        // What this pane is allowed to offer -- the `shows`, `managed` and
        // `window` gates -- is `chrome_offered`'s, and all of them decline by
        // answering `None` here rather than by returning out of the function.
        // That is the #111-shaped difference: a loading window, and a
        // client-placed menu, both still cover what is behind them, and a press
        // on either is its own and nobody else's. Occluding is a fact about
        // pixels; offering chrome is a claim about what a press would do.
        //
        // **An invisible pane is the one case that fails both questions**, and
        // it has to fail them together. Gating only `covers` below would turn
        // its `Some(chrome)` into a `PaneHit::Halo` -- a claim that survives
        // the walk and wins wherever nothing lower paints -- so a closed
        // window's resize border would go on being draggable, invisibly, over
        // bare desktop for the whole grace period. `shows` is therefore asked
        // here as well, and the pair answers `pane_hit_of(None, false)`:
        // `PaneHit::Miss`, the walk descends, and the pane is gone from the hit
        // test exactly as it is gone from the screen.
        let window = pane.client().cloned();
        let chrome = chrome_offered(
            drawn.shows(),
            pane.managed(),
            window.is_some(),
            framed,
            resize::border_edges(drawn_rect, location),
        );

        pane_hit_of(chrome, drawn.covers(location)).map(|chrome| Under {
            chrome,
            pane: pane.id(),
            window,
            local: in_outer,
            outer,
        })
    }

    /// Put a window at an exact rectangle, without animating.
    ///
    /// What a resize drag calls every frame: the window must be under the
    /// pointer's corner *now*, so this deliberately does not go through the
    /// transform the way `place` does.
    pub(crate) fn resize_to(&mut self, window: &Window, outer: Rectangle<i32, Logical>) {
        let client = inner(outer, self.frame_insets(window));

        size_window(window, client);
        self.map_stacked(window.clone(), client.loc, false);
    }

    /// Move and resize a window for real, gliding it there from where it was.
    ///
    /// This is the layout's authority: it changes the geometry everything else
    /// reads. The animation is a *transform* on top — the window is drawn from
    /// its old rectangle and lands on the new one — so a layout change and a
    /// mode use the same machinery and cannot disagree about where a window is
    /// going.
    fn place(
        &mut self,
        id: u64,
        rect: Rect,
        animation: AnimationSpec,
        now: Duration,
        standing: Standing,
    ) {
        let Some(pane) = self.panes.by_script_id(id).map(Pane::id) else {
            return;
        };
        // Captured before anything moves: this is where the animation starts.
        let Some(was) = self.pane_outer_of(pane) else {
            return;
        };

        #[expect(
            clippy::cast_possible_truncation,
            reason = "a rect from a script is screen-sized"
        )]
        let outer = Rectangle::new(
            (rect.x.round() as i32, rect.y.round() as i32).into(),
            (
                (rect.w.round() as i32).max(1),
                (rect.h.round() as i32).max(1),
            )
                .into(),
        );

        self.move_pane(pane, outer, was, animation, now, standing);
    }

    /// Put a pane's outer rectangle somewhere, and make every copy of that
    /// fact agree.
    ///
    /// There are three, and missing any one of them is a window that does not
    /// move -- or worse, moves and comes back. The space is the authority for
    /// a mapped window, so `sync_panes` writes it into the pane's slot every
    /// frame: setting the slot without telling the space is undone before the
    /// next frame is drawn, silently. That is not hypothetical, it is what the
    /// first attempt at `rescue_offscreen` did.
    ///
    /// **This is where a tiled resize meets its client, and it used to send a
    /// configure every time it ran.** A layout's sweep places every leaf on
    /// every visible monitor whether that leaf moved or not, and a drag runs the
    /// sweep once a frame, so an unchanged window was configured sixty times a
    /// second for the length of a gesture — deduplicated on the wire for xdg by
    /// smithay's `has_pending_changes`, and not deduplicated at all for X11,
    /// where `size_window` sends a real `ConfigureWindow` each time. The two
    /// windows that *did* move were configured sixty times a second for real,
    /// which is the rate `crate::resizing` exists to say no client can answer.
    /// [`Self::offers_size`] is the one gate both of those now go through.
    fn move_pane(
        &mut self,
        pane: crate::pane::PaneId,
        outer: Rectangle<i32, Logical>,
        was: Rectangle<i32, Logical>,
        animation: AnimationSpec,
        now: Duration,
        standing: Standing,
    ) {
        // **Nothing moves a window that has gone.** Scripts have been told
        // `close`, or its client has left it fading where it stood: a
        // stateless layout placing the rows of `close`'s own snapshot -- which
        // lists the window closing -- would otherwise move the tile a fading
        // picture is cut to, and a script holding the id can `sol.place` it.
        // `a_window_that_left_is_nobodys_to_find` places one.
        if self
            .panes
            .get(pane)
            .is_some_and(|held| held.gone() || held.ghost())
        {
            return;
        }
        // The frame's share comes off whichever sides it reserved; what is
        // left is the client's.
        let client = inner(outer, self.insets_of(pane));

        // **The configure, and only the configure.**
        //
        // The guard that shipped with #127 covered the transform at the bottom
        // of this function and nothing else, which left the *client-facing*
        // write running on a dying window -- issue #127's review finding 2.
        // `size_window` resizes `real_geometry` while `present::close` holds
        // `frame.rect` pinned at the rectangle the window was closed at. Those
        // two rectangles are exactly the pair `resizing::factor` divides -- the
        // client's committed buffer against the size the pane is drawn at -- so
        // the moment the layout hands the dying client a different size and the
        // client answers it, the leaving animation stretches the last buffer to
        // fill a rectangle it was never painted for. The window squashes as it
        // fades, which reads as the close going wrong rather than as a reflow
        // happening behind it.
        //
        // The configure is waste even when the client never answers: it asks
        // something in the middle of tearing itself down to re-lay-out at a
        // size that will never be drawn, on the one code path where the answer
        // cannot arrive in time to matter. Electron and the JVM are the clients
        // slow enough to still be running their quit handlers when it lands.
        //
        // **`map_stacked` is none of that, and suppressing it too was the
        // second review's finding 2.** It carries a *location* and no size, so
        // it is not half of any pair `resizing::factor` divides and cannot
        // stretch anything; and it tells no client anything -- it writes into
        // `self.space`, a window's position is not on the xdg wire at all, and
        // for X11 it is `size_window` that sends the `ConfigureWindow`. What it
        // is, is the third copy of the fact this function exists to keep in
        // agreement, and this function's own opening paragraph already says
        // what dropping it costs: *setting the slot without telling the space
        // is undone before the next frame is drawn, silently*. `pane_geometry`
        // answers `real_geometry` for a mapped client, so `sync_panes` copies
        // the space's stale rectangle back over the `set_slot` below on the
        // very next frame -- which made the slot, `Pane::placed` and the space
        // three different answers instead of one.
        //
        // The consequence was not cosmetic. A sweep that moved a pane *during*
        // a close -- a workspace switch, `rescue_offscreen`, a config reload --
        // followed by a refusal handed the window back at its **pre-close**
        // rectangle while `Pane::placed` said the layout had moved it. Those
        // two are exactly what `pane_laid_out` pairs, so #124's edge drag began
        // from a rectangle the window was not at. That path rests on `real.loc`
        // being compositor-set and therefore exact; this line is what keeps it
        // so.
        //
        // **What is still suppressed, said plainly rather than left to be
        // inferred.** The client's *size* stays one configure behind for as
        // long as the pane is leaving, because that configure was never sent.
        // It corrects itself on the first sweep after the window comes back --
        // the configure was suppressed before `offers_size` could record it as
        // told, so re-placing at the same rectangle is a change and goes out --
        // and `a_window_the_layout_moved_mid_close_comes_back_where_it_was_put`
        // asserts that rather than this paragraph asserting it.
        let leaving = self.panes.get(pane).is_some_and(Pane::leaving);

        // A client is moved and resized for real, and the space is told,
        // because the space is the authority for a mapped window.
        if let Some(window) = self.panes.get(pane).and_then(Pane::client).cloned() {
            if !leaving && self.offers_size(pane, &window, client, now) {
                size_window(&window, client);
            }
            // Placing is not focusing. It is still a raise -- see
            // `map_stacked` -- and `map_laid_out` is what keeps that raise from
            // burying a window that is leaving or fading; see the note on the
            // transform below.
            self.map_laid_out(window, client.loc, now);
        }
        // And the pane is told either way. For a mapped window this is what
        // `sync_panes` would write next frame anyway; for a pane whose
        // application has not arrived it is the whole of the move, because
        // there is nothing else holding its geometry.
        if let Some(held) = self.panes.get_mut(pane) {
            held.set_slot(client);
            // **And the layout's own answer, kept where no client can reach
            // it.** The line above is exactly the one `sync_panes` overwrites:
            // it writes the space's rectangle into the slot on every frame this
            // pane is not held, and the space reports a mapped window's size as
            // whatever the client last committed. So `slot` is the rectangle
            // asked for only until the client answers, and a client answering
            // with a size of its own is the ordinary case rather than the
            // exception. `Pane::placed` records the rectangle that was *asked
            // for*, which is the only copy of the layout's opinion the
            // compositor keeps; see `Self::pane_laid_out`.
            //
            // **Only for a tile**, because since #133 that field is also the
            // rectangle the client is held inside, and a placement that is not
            // a tile must not hold anything. See `Standing`.
            match standing {
                Standing::Tile => held.set_placed(outer),
                Standing::Free => held.untile(),
                // Moved and not set, so a let-go a leaving pane is waiting on
                // is still owed after a rescue. See `Pane::move_tile`, and
                // `a_rescue_during_a_fade_keeps_the_let_go_it_is_waiting_on`.
                Standing::Kept => held.move_tile(outer),
            }
        }

        // **And the transform, unless this pane is leaving.**
        //
        // `present::from` is released on arrival and aimed at `Frame::real` —
        // full size, full opacity — which is right for every pane that is
        // staying and is the whole of issue #127's first fault for one that is
        // not. `Pane::closing_at` had three readers and this was not one of
        // them, so any layout sweep inside the 190ms `CLOSING` window put the
        // dying window back at full opacity, released the transform, and left
        // it to vanish with no animation at all. A sweep inside that window is
        // not exotic: another window opening, a layer surface's first
        // configure, a GTK4 `set_parent` or `set_modal` each cause one.
        //
        // **What a closing pane does when the layout moves it: it animates out
        // from where it was.** The slot moves under it and the transform does
        // not follow, so the window shrinks and fades at the rectangle it was
        // closed at while its neighbours reflow around the space it is about to
        // give up. The alternative — sliding to the new slot while fading —
        // was rejected on three counts.
        //
        // * It animates towards a place the window will never occupy. The pane
        //   is retired within `CLOSING` plus whatever the client takes, so the
        //   destination is a fiction, and it pulls the eye away from the window
        //   the user just acted on.
        // * `present::close` computes its target *once*, from the frame at the
        //   instant of the press. Following the layout means recomputing that
        //   target on every sweep, and a sweep can run once a frame —
        //   `tiling.lua` runs `tiling.apply` per frame for the whole of a seam
        //   drag. Restarting a 190ms easing at 60Hz is an animation that never
        //   finishes, which is the shape of the defect being fixed here.
        // * The close transform already owns this pane's presentation for the
        //   rest of its life, deliberately: `present::close` is the one
        //   transform written with `release: false`, so that the window stays
        //   invisible between the animation landing and the client acting. The
        //   layout is not the authority over where a leaving pane is *drawn*,
        //   and this line was the only place that said otherwise.
        //
        // The cost is that a closing window overlaps the one moving into its
        // space for up to 190ms -- on every close since #128, because a layout
        // closes up the moment the close is asked for. It is shrinking and
        // fading throughout, and what that reads as depends on which of the two
        // is in front.
        //
        // **The window being closed is, and that is decided rather than left
        // to the sweep** (#128's review, findings 2 and 6). `Space::map_element`
        // puts every window a layout places on top, whatever `activate` says,
        // and the `closing` sweep places every survivor and not the
        // window being closed -- so the neighbour growing into the space was
        // stacked over it on every close and covered most of the fade, which
        // is the reverse of the look this note was written to buy. So
        // `close_pane` raises the window as its close begins, and
        // `map_laid_out` never raises a placed window past one that is leaving
        // or drawn translucent, so no later sweep in the fade buries it either.
        // A refused window fading back in is the same case in reverse. See
        // `a_closing_window_fades_in_front_of_the_neighbour_moving_into_its_space`
        // and `a_refused_window_fades_back_in_front_of_the_neighbour_making_room`.
        //
        // In front costs the window underneath no input once the fade is over:
        // the dying pane is at opacity zero from the end of `CLOSING`, and
        // `Frame::covers` gates every hit test on `shows()`, so a press there
        // reaches what is drawn there. See
        // `a_press_where_a_closed_window_used_to_be_reaches_what_is_drawn_there`.
        //
        // **The bookkeeping is what survives, not the presentation.** All three
        // copies of where this pane lives — the space, the slot and
        // `Pane::placed` — are written for a leaving pane exactly as for a
        // staying one. Exactly two things are suppressed, and they are the two
        // a *client* can observe: the configure at the top of the function, and
        // this transform. The first draft of this guard covered only this line,
        // which left the configure running; the second suppressed the space as
        // well, which left the three copies disagreeing. See the note above
        // them for both.
        //
        // **With one exception, and it is about the presentation too:** a
        // `Standing::Free` placement does not clear `Pane::placed` on a leaving
        // pane. That field is the tile the fading window is cut to, so
        // `Pane::untile` keeps it and owes the let-go until the window is back.
        // See `Pane::let_go`, and
        // `a_mode_switched_during_a_fade_does_not_squash_the_window_leaving`,
        // which places a leaving window with `tile = false`.
        //
        // Note what needs no guard. `present::rebase` — the group path — is
        // safe for a leaving pane by construction: it preserves both the
        // destination and the release flag, so a closing transform rebased by a
        // workspace slide is still a closing transform. It is this function's
        // unconditional `from` that was the exception.
        //
        // **From what is on screen, not from where the books say the pane was**
        // (#128's review, finding 1). `was` is `pane_outer`: the space's
        // location and the size the client last committed. Between two sweeps
        // in one dispatch those are the first sweep's answer and not anything
        // that has been drawn, and a dialog answering a close, sent in one
        // flush, is several sweeps in one dispatch -- `give_back`'s `refused`,
        // then `parent_changed`, then `modal_changed`. Starting from
        // `Frame::real(was)` put the refused window back at full opacity before
        // its fade had drawn a frame, and in tiling threw its neighbour to its
        // new position at its old size. `present::frame` is what the next
        // frame would draw, and for a pane with no transform it is
        // `Frame::real(was)`, so a placement from rest starts where it always
        // did. See
        // `a_refusal_by_dialog_fades_the_window_back_and_moves_its_neighbour_once`.
        if let Some(held) = self.panes.get(pane).filter(|_| !leaving) {
            let start = present::frame(held, was, now);
            present::from(
                held,
                outer,
                start,
                now,
                animation.duration,
                animation.easing,
            );
        }
    }

    /// Whether the client hears about this rectangle on this frame, and the
    /// bookkeeping that decides it.
    ///
    /// Three answers, in order, and the order is the design:
    ///
    /// 1. **A pane already under a bridge answers from its own hold.** The
    ///    throttle is per pane, not per gesture, which is what stops a window
    ///    merely pushed aside by someone else's drag from having its one
    ///    configure swallowed by an interval the dragged window opened. Each
    ///    hold's clock starts when that pane first moved.
    /// 2. **A client already at this exact rectangle is told nothing.** This is
    ///    most of a layout's sweep: `tiling.apply` re-places every leaf on every
    ///    visible monitor, and in a dwindle tree two of them changed. The whole
    ///    rectangle is compared and not just the size, because for X11
    ///    `size_window` is the only thing that carries a *position* — `map_stacked`
    ///    moves the window in the space and tells the client nothing — so
    ///    deduplicating on size alone would leave an X11 window told to stay
    ///    where it no longer is.
    /// 3. **Anything else is a real change, and is sent.** If a gesture is live
    ///    this is also the moment a pane joins the bridge, and the immediacy is
    ///    deliberate: the first offer of a new size goes out on the frame it is
    ///    decided, and only the ones after it are throttled.
    ///
    /// **Case (1) is only the drag's throttle when the drag is what is placing
    /// this pane.** A bridge outlives its gesture by up to `PATIENCE`, and
    /// `move_pane` is reached by a config reload, a `modes.use` from a
    /// keybinding, a workspace switch and `rescue_offscreen` as well — none of
    /// which has a next frame to resend anything. Routing one of those through
    /// the throttle dropped its one configure while `move_pane` went on writing
    /// the slot, which leaves the pane drawn, stretched, at a rectangle its
    /// client was never told about for the rest of the gesture.
    /// [`Self::resize_gesture`] is exactly "the layout sweep running right now
    /// belongs to a live drag", so it is the question, and anything else offers
    /// the same rectangle unthrottled through `Hold::placed` — through the hold
    /// rather than around it, so `asked` still names what the client last heard.
    ///
    /// **A toplevel carrying a pending change is sent whatever the throttle
    /// says**, for the same reason and one more: `size_window` is a pending size
    /// *and* a `send_pending_configure`, so a throttled frame skips the flush
    /// too, and a maximise or a decoration mode agreed by somebody else is then
    /// blocked behind an interval for exactly the windows a drag is touching.
    ///
    /// Note what is *not* here: nothing arms a hold without
    /// [`Self::resize_gesture`]. A keyboard nudge, a reload, a monitor change
    /// and a workspace switch all reach `move_pane`, and a hold armed by one of
    /// them could never be released — `Self::release_resize` has one caller and
    /// it is the pointer grab — so it would answer `Settle::Waiting` for ever
    /// and hold the slot and the space apart for ever. They fall to (2) and (3),
    /// which is what they had before minus the configures for panes that did not
    /// move.
    fn offers_size(
        &mut self,
        pane: crate::pane::PaneId,
        window: &Window,
        client: Rectangle<i32, Logical>,
        now: Duration,
    ) -> bool {
        let committed = window.geometry().size;
        // **And anything else the toplevel is carrying.** A maximise, a
        // fullscreen or a decoration mode agreed before the initial configure
        // went out is a pending change somebody else wrote and is waiting on.
        // Every one of those sends its own configure today, so this is belt and
        // braces rather than a known hole; it is here because the alternative
        // failure is a window that never hears an answer it is blocked on, and
        // there is no cheaper way to be sure than asking.
        let pending = window
            .toplevel()
            .is_some_and(smithay::wayland::shell::xdg::ToplevelSurface::has_pending_changes);
        // Whether the sweep reaching this pane is the live drag's own. Read
        // before the borrow below, which needs `self` mutably.
        let dragging = self.resize_gesture.is_some();
        // Case (1). The borrow ends on this line, so the arm below can reach
        // `self` again.
        let bridged = self
            .resize_bridge
            .as_mut()
            .and_then(|bridge| bridge.panes.iter_mut().find(|held| held.pane == pane))
            .map(|held| {
                if dragging && !pending {
                    held.hold.dragged(client, committed, now)
                } else {
                    held.hold.placed(client, committed, now)
                }
                .is_some()
            });
        let (told, held) = match bridged {
            Some(told) => (told, true),
            None => self.offers_first_size(pane, window, client, committed, now),
        };
        // A pending flush is worth a configure even when the rectangle is one
        // the client has already been told: the size is not what it is waiting
        // for.
        let told = told || pending;
        if crate::resizing::trace::on() {
            let hold = self.held_hold(pane);
            let asked = hold.map_or(client, crate::resizing::Hold::asked);
            crate::resizing::trace::line(
                "layout",
                format_args!(
                    "pane={} slot={},{} {}x{} committed={}x{} asked={},{} {}x{} told={} \
                     held={} refused={} unanswered={}",
                    pane.get(),
                    client.loc.x,
                    client.loc.y,
                    client.size.w,
                    client.size.h,
                    committed.w,
                    committed.h,
                    asked.loc.x,
                    asked.loc.y,
                    asked.size.w,
                    asked.size.h,
                    u8::from(told),
                    u8::from(held),
                    u8::from(hold.is_some_and(crate::resizing::Hold::refused)),
                    // Which side of `SILENCE` the verdict beside it was taken
                    // on. The two answers fail in opposite directions, so a log
                    // without this cannot say which one it caught.
                    hold.map_or(0, crate::resizing::Hold::unanswered),
                ),
            );
        }
        told
    }

    /// Cases (2) and (3) of [`Self::offers_size`]: a pane with no bridge entry.
    ///
    /// Returns whether the client is told and whether a hold was armed, which
    /// are different questions. A pane is told without being bridged by every
    /// caller that is not a drag.
    fn offers_first_size(
        &mut self,
        pane: crate::pane::PaneId,
        window: &Window,
        client: Rectangle<i32, Logical>,
        committed: Size<i32, Logical>,
        now: Duration,
    ) -> (bool, bool) {
        // The client's own rectangle: where the space has it, at the size it
        // last committed. Read before `map_stacked` moves it, which is why this
        // is answered here rather than after the move.
        //
        // **This is the right question for case (2) and the wrong one for the
        // edges below**, and the two used to share it. "Has the client already
        // been put here" is about where the *client* is, so it asks the space.
        // "Which of this pane's edges did this placement move" is about the
        // *pane*, and the pane's previous rectangle is its slot: during a drag
        // the client's committed size is frames behind the slot, so deriving an
        // edge from it mixes the client's latency into the answer and can name
        // an edge the placement never touched — `moved_edges` would see both
        // sides of an axis move where only one did, or a far edge move where
        // only the near one did, and `Hold::pins` and `anchored` would then
        // hang a held picture against the wrong side of a window.
        let before = self.real_geometry(window);
        let changed = before != Some(client);
        // A hold is armed by a live gesture and by nothing else, and only for a
        // pane whose rectangle actually changed: a layout re-placing a leaf
        // exactly where it already is has moved nothing, and a hold for it
        // would pin a slot that needs no pinning until the gesture ended.
        let released = self.resize_gesture.as_ref().map(|gesture| gesture.released);
        let Some(released) = released.filter(|_| changed) else {
            return (changed, false);
        };
        // **The pane's own moved edge, not the pointer's.** See
        // `crate::resizing::moved_edges`: the neighbour across a seam has the
        // opposite edge pulled, and a pane shoved sideways by someone else's
        // drag has neither. Against the pane's previous slot for the reason
        // `before` gives; a pane with no slot yet cannot have moved an edge, so
        // `client` against itself answers `ResizeEdge::None`, which is the
        // honest "nothing to anchor against".
        let previous = self.panes.get(pane).map_or(client, Pane::slot);
        let edges = crate::resizing::moved_edges(previous, client);
        // **The same drag's hold if this pane already had one, rather than a
        // fresh one.** `settle_resize` forks per frame, so a layout that claims
        // a drag on one frame and not the next hands this pane back and forth
        // between the floating path and the bridge. Building a new hold each
        // way reset `Hold::told`, so every flip bought an unthrottled configure
        // and an alternating handler restored the sixty a second this fix
        // removes. Same client, same gesture, same throttle — only the edges
        // are this path's to name. See `Hold::retargeted`.
        //
        // `released` is reasserted because `arm_resize_gesture` rearms the
        // bridge's holds before the sweep and a floating hold arriving during it
        // missed that: its deadline must stop for the same reason theirs did.
        let carried = self
            .resize_hold
            .take_if(|held| held.pane == pane && &held.window == window)
            .map(|held| held.hold);
        let (hold, told) = match carried {
            Some(mut hold) => {
                hold.retargeted(edges);
                hold.rearm(released);
                let told = hold.dragged(client, committed, now).is_some();
                (hold, told)
            }
            // A pane joining the bridge hears immediately, and only the offers
            // after it are throttled: `Hold::new` records this frame as the one
            // the client was spoken to on, and the caller sends.
            //
            // `committed` and not `before.size`, which is the same number —
            // `real_geometry` builds its size from `window.geometry()` — read
            // from the one source that answers for a window the space has let
            // go of as well.
            None => (
                crate::resizing::Hold::new(edges, committed, client, now, released),
                true,
            ),
        };
        let held = crate::resizing::Held {
            window: window.clone(),
            pane,
            hold,
        };
        match self.resize_bridge.as_mut() {
            Some(bridge) => {
                bridge.panes.push(held);
                (told, true)
            }
            // `arm_resize_gesture` creates the bridge with the gesture, so this
            // is unreachable rather than a case: answered instead of asserted
            // because a compositor may not panic. The client is still told,
            // because losing a hold is not a reason to lose a configure.
            None => (true, false),
        }
    }

    /// Start a program as a client of this compositor.
    fn spawn(&mut self, program: &str, args: &[String]) {
        use std::process::{Command as Process, Stdio};

        let mut process = Process::new(program);
        process
            .args(args)
            // Without this the child inherits the *host* display and opens its
            // window next to the compositor rather than inside it.
            .env("WAYLAND_DISPLAY", &self.socket_name)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            // Errors are inherited, not discarded. A program that refuses to
            // start says why on stderr, and swallowing that leaves "the
            // binding does nothing" as the only symptom of a dozen different
            // causes.
            .stderr(Stdio::inherit());

        // Our own X server if we have one, and emphatically not the host's if
        // we do not. Inheriting DISPLAY is worse than it sounds: a Qt or GTK
        // program prefers X11 when it is set, connects to the *host's*
        // XWayland, and opens its window on the host desktop. The spawn logs
        // success, nothing errors, and no window ever appears here.
        match self.x11_display {
            Some(number) => process.env("DISPLAY", format!(":{number}")),
            None => process.env_remove("DISPLAY"),
        };

        // Before the fork, so the window is on screen and the other windows
        // have moved aside by the time the program has been asked to start.
        let pane = self.begin_loading(program, None);

        // And a token in the child's environment, naming the window we just
        // opened for it.
        //
        // This is what makes the window find its application whatever the
        // application does to its own processes. Matching on the pid works
        // right up until a launcher forks and exits -- Firefox does -- and then
        // the chain from the client runs into init and stops. A token does not
        // care: we made it, we handed it over, and whatever comes back holding
        // it is the thing we launched.
        let token = self.launch_token(pane);
        process.env("XDG_ACTIVATION_TOKEN", &token);
        // The older spelling, for programs that only look for that one.
        process.env("DESKTOP_STARTUP_ID", &token);

        match process.spawn() {
            Ok(mut child) => {
                tracing::info!(program, socket = self.socket_name, "spawned");
                // And it learns whose process to wait for here, because there
                // was no process to name a moment ago.
                if let Some(pane) = self.panes.get_mut(pane) {
                    pane.expect(child.id());
                }
                // Waited on so the child is reaped — a compositor that leaves
                // zombies eventually cannot fork at all — and so that an early
                // exit is *reported*. A program that starts and immediately
                // quits is indistinguishable from one that never drew, and
                // that is the hard version of this to debug.
                let name = program.to_owned();
                std::thread::spawn(move || match child.wait() {
                    Ok(status) if status.success() => {
                        tracing::debug!(program = name, "a spawned program exited cleanly");
                    }
                    Ok(status) => {
                        tracing::warn!(program = name, %status, "a spawned program exited");
                    }
                    Err(err) => tracing::warn!(program = name, ?err, "could not wait for a child"),
                });
            }
            Err(err) => {
                // Nothing is coming, so the window goes now rather than
                // sitting there for the whole of `patience` promising
                // otherwise. The layout is told, and closes the gap, while the
                // window fades out of it. See `Self::depart`.
                self.depart(pane);
                self.redraw = true;
                tracing::warn!(?err, program, "could not spawn");
            }
        }
    }

    /// An activation token naming the window opened for a launch, for the
    /// launched program's environment. Apart from [`Self::spawn`] so that a
    /// test can hand its own client the token a launch would have.
    fn launch_token(&mut self, pane: crate::pane::PaneId) -> String {
        let data = XdgActivationTokenData::default();
        data.user_data.insert_if_missing(|| LaunchedFor(pane));
        let (token, _) = self.activation_state.create_external_token(data);
        token.as_str().to_owned()
    }

    /// The window a script means by an id.
    ///
    /// Ids that no longer exist are simply not found — a window closing while a
    /// mode holds its id is ordinary, not an error.
    fn window_by_id(&self, id: u64) -> Option<Window> {
        self.panes.by_script_id(id).and_then(Pane::client).cloned()
    }

    /// The window drawn at a point, topmost first, with its real geometry.
    ///
    /// Hit-testing follows the transform: in overview a window is clickable
    /// where the thumbnail is, not where the window lives.
    ///
    /// **The walk stops at the first pane that covers the point, whether or not
    /// that pane has a window to hand back.** This is issue #111 in the third
    /// of the three walks: `client()?` used to sit inside a `find_map`, where
    /// `None` means "keep looking" rather than "stop", so a window still
    /// loading — drawn, on screen, under the cursor and with no client yet —
    /// was descended straight past. `chrome_under` now correctly answers
    /// nothing over its body, `pointer_button` falls through to click-to-focus,
    /// and this raised and focused the window *behind* it. Same reasoning and
    /// the same line as [`Self::surface_under`], which has always got this
    /// right by holding its `?` in a `for` loop instead.
    ///
    /// `pane_outer` rather than `outer_geometry` for the same reason, and it is
    /// what makes stopping possible at all: `outer_geometry` needs the window,
    /// so the old shape could not ask whether a client-less pane covered the
    /// point even in principle.
    pub(crate) fn window_under(
        &self,
        location: Point<f64, Logical>,
    ) -> Option<(Window, Rectangle<i32, Logical>)> {
        self.window_under_at(location, self.clock.now())
    }

    /// The same walk, at an instant the caller names.
    ///
    /// [`Self::window_under`] is this at the present, which is what a press is
    /// answered from and must stay. The one other caller is
    /// [`Self::settle_focus`]'s pointer arm, which is a focus decision and so
    /// asks at [`Self::settling`] — see [`SETTLED`] for the rule and
    /// `a_pointer_over_the_desk_being_left_does_not_hand_it_the_keyboard` for
    /// the two asked of one pixel on one frame.
    fn window_under_at(
        &self,
        location: Point<f64, Logical>,
        now: Duration,
    ) -> Option<(Window, Rectangle<i32, Logical>)> {
        // Locked, so there is no window under the pointer however many are
        // still mapped. Everything built on this -- click to focus, focus
        // follows mouse, drag, resize -- stops at once, in one place.
        if self.lock.is_some() {
            return None;
        }
        let screens = self.screens();
        for pane in self.panes.iter().rev() {
            let outer = self.pane_outer(pane);
            // `covers`, not `rect.contains`: a pane drawn at opacity zero is
            // not on screen and owns no pixel, however solid the rectangle it
            // would be drawn at. See [`present::Frame::covers`], and #127's
            // review finding 1 -- this walk ends in `focus_window` through
            // click-to-focus, so an invisible pane winning it took the
            // keyboard as well as the click.
            //
            // And only on a screen that draws the pane, which is [`shown_at`]:
            // a hidden workspace carried over the next monitor covers pixels
            // that monitor never drew it on (#134's third review).
            //
            // Both through [`owns`], which is also what `sol.window_at` asks,
            // so a script and this walk put the same question to each window --
            // `on_two_monitors_sol_window_at_answers_what_the_right_monitor_draws`.
            // And never of what is left of a window whose client has gone.
            if !owns(
                outer,
                pane.ghost(),
                self.drawn_at(pane, outer, now),
                location,
                &screens,
            ) {
                continue;
            }
            // Covered. A pane whose application has not arrived has no window
            // to focus -- but it is on screen and it is under the cursor, so
            // nothing behind it may be focused or raised by this press either.
            let window = pane.client()?;
            return Some((window.clone(), self.real_geometry(window)?));
        }
        None
    }

    /// The surface at a point, and the origin to measure it from.
    ///
    /// The origin is chosen so that `location - origin` is the point in
    /// surface-local coordinates. That is what keeps a scaled window honest:
    /// the client is told where in *itself* the pointer is, and never learns
    /// that it is being drawn at half size.
    pub(crate) fn surface_under(
        &self,
        location: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        // Anchored surfaces above windows are hit first: a click on a panel is
        // the panel's, and it reserved that strip precisely so nothing of the
        // client's would be under the cursor there.
        //
        // The monitor under the point, not the first one: a layer map's
        // geometry is in its own output's coordinates, so asking the wrong
        // output hit-tests the right strip on the wrong screen.
        // Locked: the only surface anyone may point at is the lock screen's,
        // and on a monitor it has not covered, none at all. Returning early
        // rather than filtering afterwards is deliberate -- a later `return`
        // that forgets the check is a click landing in the session.
        if let Some(lock) = self.lock.as_ref() {
            let output = monitor::at(&self.space, location)?;
            let geometry = self.space.output_geometry(&output)?;
            let surface = lock.surface_for(&output)?;
            return under_from_surface_tree(
                surface.wl_surface(),
                location - geometry.loc.to_f64(),
                (0, 0),
                WindowSurfaceType::ALL,
            )
            .map(|(surface, offset)| (surface, (geometry.loc + offset).to_f64()));
        }

        if let Some(output) = monitor::at(&self.space, location)
            && let Some(geometry) = self.space.output_geometry(&output)
            && let Some((surface, origin)) =
                layer::surface_under(&output, location - geometry.loc.to_f64())
        {
            // `origin` came back in the output's coordinates; the pointer is
            // measured in the compositor's.
            return Some((surface, origin + geometry.loc.to_f64()));
        }

        let now = self.clock.now();
        let screens = self.screens();

        for pane in self.panes.iter().rev() {
            let outer = self.pane_outer(pane);
            // Nothing of a pane -- its surface or its popups, which the
            // renderer draws inside the same cull -- is at a point on a screen
            // that does not draw it. [`shown_at`] says why, and
            // `on_two_monitors_a_press_on_the_right_monitor_reaches_what_it_draws`
            // is this walk. Nor anything of a window whose client has gone:
            // `a_window_that_left_is_nobodys_to_find`.
            if !shown_at(outer, pane.ghost(), location, &screens) {
                continue;
            }
            let frame = self.drawn_at(pane, outer, now);
            // Invisible is not covered. Same rule and same reason as
            // [`Self::window_under`]: this walk is what delivers motion,
            // buttons and — through the focus a press sets — keystrokes, so a
            // pane held at opacity zero across a close winning it is where the
            // typing went. [`present::Frame::covers`] argues the predicate.
            //
            // **Except that a window's popups are not inside its rectangle.**
            // `render::elements` draws them uncut, reaching past the parent's
            // tile, so a point outside the parent's frame can still be on one
            // of its menus -- and since #133 that includes the whole strip
            // between a tiled window's tile and the edge its client committed,
            // where a menu opened from an oversized Firefox lands. The point
            // went to the neighbour instead, and a press on it -- when the
            // neighbour is another application -- had smithay's popup grab
            // dismiss the menu rather than choose the item under it
            // (`PopupPointerGrab::button`). So a visible pane that does not
            // cover the point is still asked about its popups, and only about
            // those. `a_menu_past_its_parents_tile_takes_the_press` pins it.
            let covered = frame.covers(location);
            let (window, kind) = match pane.client() {
                Some(window) if covered => (window, WindowSurfaceType::ALL),
                // A popup and its own subsurfaces, and not the toplevel's tree.
                Some(window) if frame.shows() => (
                    window,
                    WindowSurfaceType::POPUP | WindowSurfaceType::SUBSURFACE,
                ),
                // A window whose application has not arrived has no surface to
                // give the pointer -- but it is on screen and it is under the
                // cursor, so nothing behind it may have the click either.
                // Falling through would type into whatever the window is
                // covering.
                None if covered => return None,
                _ => continue,
            };

            // Into the client's own space through the very fit its picture is
            // drawn with (#133): off the buffer's drawn corner, and divided by
            // what the buffer was scaled by. A point in the titlebar lands
            // above the client and finds no surface, which is what should
            // happen: the frame is the compositor's, not the client's.
            //
            // It used to go through `to_window_space`, which reads the drawn
            // rectangle as a scale of the pane's own, and take its inset from
            // `frame_insets`. That is the picture's arithmetic for a decorated
            // window at rest or in a thumbnail and for nothing else:
            //
            // * a tiled window on a frame of a glide is drawn 1:1 and cut, and
            //   a press there landed as far from the pixel under it as the
            //   glide was from its destination --
            //   `a_press_on_a_gliding_window_lands_on_the_pixel_under_it`;
            // * a window under a resize hold has its last buffer stretched
            //   into the dragged rectangle, and a press was mapped 1:1 against
            //   the rectangle instead --
            //   `a_press_on_a_held_window_lands_on_the_pixel_its_picture_has`;
            // * a pane still reserving a titlebar is drawn below it, and
            //   `frame_insets` answers nothing for a pane that is not yet
            //   decorated --
            //   `a_press_on_a_window_reserving_a_titlebar_lands_on_the_pixel_under_it`.
            //
            // It is still the picture's arithmetic for the *frame*, which is
            // stretched from the pane's outer size to the drawn rect, so
            // `pane_chrome` keeps it.
            let placed =
                crate::render::place_client(self, pane, &frame, outer.size, window.geometry().size);
            let undo = |drawn: f64, factor: f64| {
                if factor.abs() > f64::EPSILON {
                    drawn / factor
                } else {
                    drawn
                }
            };
            let in_window: Point<f64, Logical> = (
                undo(location.x - placed.origin.x, placed.fit.factor.x),
                undo(location.y - placed.origin.y, placed.fit.factor.y),
            )
                .into();

            // Into the *buffer's* coordinates, which is what `surface_under`
            // wants and is not the same point.
            //
            // A client that draws its own decorations commits a surface bigger
            // than its window: the invisible resize shadow is part of the
            // buffer, and `xdg_surface.set_window_geometry` is how it says
            // which sub-rectangle is the real window. `geometry().loc` is that
            // offset -- around (26, 26) for a GTK application.
            //
            // `in_window` above is relative to the window the user can see.
            // Smithay's `Window::surface_under` ends in
            // `under_from_surface_tree(&surface, point, (0, 0), ..)` -- offset
            // zero -- so the point it expects is relative to the surface tree
            // root, the buffer origin. Its own `SpaceElement` wrapper puts that
            // origin at `location - geometry().loc`
            // (`desktop/space/mod.rs:510`), so the two differ by exactly
            // `geometry().loc`, and dropping it is issue #101: every click in
            // Firefox and in Qt applications landed a shadow's width up and
            // left of where it was aimed, which for a row of buttons means the
            // one next door.
            //
            // Zero for a client with no decorations of its own, so a terminal
            // never noticed.
            let in_buffer = in_window + window.geometry().loc.to_f64();

            if let Some((surface, surface_offset)) = window.surface_under(in_buffer, kind) {
                let in_surface = in_buffer - surface_offset.to_f64();
                return Some((surface, location - in_surface));
            }
        }

        None
    }

    /// Ask a window to close, once it has finished leaving.
    ///
    /// A close is a request the client may refuse, so the compositor cannot
    /// simply animate the window away and drop it. What it can do is animate
    /// first and ask afterwards: the window shrinks and fades while it is
    /// still alive, and the request goes out when that lands. A client that
    /// refuses is left with a window that is drawn away -- so the transform is
    /// cleared in that case too, and the window comes back.
    ///
    /// This covers closes the compositor asks for -- a frame button, a
    /// binding, a script. A client that exits on its own plays the same fade,
    /// from the textures the renderer had already imported for its surfaces,
    /// taken at the moment it goes rather than held for every window on the
    /// chance: see [`Self::depart`] and `crate::remains` (#126).
    ///
    /// **Asked at most once per window.** See [`Pane::leaving`] for the four
    /// states that answer it, and why the narrower question this used to ask
    /// was wrong for most of the time a window spends leaving.
    pub(crate) fn close_pane(&mut self, id: crate::pane::PaneId) {
        // The pane is looked up before the "already leaving" guard rather than
        // after it, which the `closing` map could not do. Same answer either
        // way: an id with no pane returned early on the second check before
        // and returns early on the first one now, and a pane already on its
        // way out must not have its animation restarted.
        let Some(pane) = self.panes.get(id) else {
            return;
        };
        // **The guard was widened rather than the write order changed**, and
        // the two are not alternatives to each other. `settle_closing` clears
        // `closing_at` before it stamps `asked_at`, so swapping those two lines
        // closes a gap of a few statements that nothing can press a key inside
        // -- while leaving the whole of the grace period after them, which is
        // *hundreds of milliseconds of an invisible window*, answering "not
        // closing". That is the window a second `super+q` actually lands in,
        // and only the predicate closes it. Ordering is now irrelevant here
        // either way, which is worth more than picking one: `leaving` is true
        // across the whole transition however those two writes are arranged.
        //
        // **What this leaves the user with for a wedged client, written down
        // because it is a gap and not a decision.** There is no force-kill
        // anywhere in this compositor -- no `xkill`, no "application is not
        // responding", no binding that destroys a client rather than asking
        // it. Against a client that has hung, `super+q` therefore does one
        // thing per close cycle: animate out, ask, wait `GRACE`, come back.
        // Roughly 1.34s from press to the window standing there again, and
        // then it can be asked once more, for ever. Before the guard was
        // widened a user could at least hammer the binding -- which achieved
        // nothing either, since `send_close` is a request a wedged client is
        // not reading, but it did not *look* like the compositor ignoring the
        // keyboard. That is a real regression in what the session feels like,
        // and the honest fix is a kill path rather than a narrower guard here.
        // It wants a confirmation of its own and is not part of #127.
        if pane.leaving() {
            return;
        }
        // No `let else`. `pane_outer` answers for every pane there is -- see
        // its own documentation -- and the bail that used to be here was dead
        // code that read as a real case: a window the compositor cannot
        // *locate* and therefore declines to close.
        let outer = self.pane_outer(pane);
        let now = self.clock.now();
        present::close(pane, outer, now);
        if let Some(pane) = self.panes.get_mut(id) {
            pane.begin_closing(now + present::CLOSING);
        }
        // **In front for its fade** (#128's review, findings 2 and 6). The
        // layout is about to move a neighbour into this window's space, and the
        // window closed is not always the one on top: a strip's sweeps stack
        // its columns left to right, whichever has the keyboard. Raised here,
        // before `closing`, so the sweep below meets a leaving window above
        // whatever it moves -- and `map_laid_out` keeps it there. Its modals
        // go above it as on any raise. See
        // `a_closing_window_fades_in_front_of_the_neighbour_moving_into_its_space`.
        if let Some(window) = self.panes.get(id).and_then(Pane::client).cloned() {
            self.space.raise_element(&window, false);
            self.lift_modals_over(&window);
        }
        self.redraw = true;
        // **And the layout is told now, not when the client has gone** (#128).
        // After `begin_closing`, so the snapshot already lists the window as
        // `leaving`. The animation above is already pinned to the rectangle the
        // window is closing at -- `move_pane` declines to move a leaving pane's
        // transform -- so a layout that reflows here grows the neighbours into
        // the space while the window fades where it stood. See
        // `a_closing_window_hands_its_space_over_before_its_client_is_gone`.
        //
        // Every route here arrives with the scripts back in their slot, which
        // is what `trigger_closing` needs to deliver this at all: the frame
        // button is `frame_action` off a pointer press, and `sol.close` --
        // `super+q` included, which is a binding in `init.lua` -- is a
        // `Command::Close`, applied by `apply` after the dispatch that asked for
        // it has returned the scripts. See
        // `every_close_route_tells_the_layout_the_close_has_begun`.
        self.trigger_closing(id);
    }

    /// Send the close to every window whose leaving animation has landed.
    ///
    /// Returns whether any window is still on its way out, so the backend
    /// keeps drawing until they are gone.
    ///
    /// **Except the ones that have already been answered.** A pane marked
    /// `Pane::answered` is one [`Self::refused_with_a_dialog`] tried and failed
    /// to give back; this is where that retry lives, because this is the only
    /// deadline such a pane is on. See the loop.
    pub(crate) fn settle_closing(&mut self, now: std::time::Duration) -> bool {
        // Over the panes rather than over a map of timers, so a pane that has
        // gone cannot be visited at all. It could be before, between a
        // `Panes::remove` and the `sync_panes` that swept the map after it --
        // and every reader that found such an entry did nothing with it but
        // remove it, so nothing observable turned on that window.
        let due: Vec<crate::pane::PaneId> = self
            .panes
            .iter()
            .filter(|pane| pane.closing_at().is_some_and(|at| now >= at))
            .map(Pane::id)
            .collect();
        for id in due {
            // **A close that has already been answered is retried, not sent.**
            // `Pane::answered` is set by `refused_with_a_dialog` on a pane whose
            // `give_back` was declined -- `present::clear` goes through
            // `with_slot`, which hands back `None` rather than panicking when
            // the transform slot is busy. Nothing else can pick that up: inside
            // `CLOSING` the pane has no `asked_at`, so `settle_refused` is not
            // looking at it, and reaching this line would send the request and
            // close the parent out from under the very dialog that answered it.
            //
            // `continue` rather than `stop_closing`: leaving `closing_at` set is
            // what makes this the retry. The pane stays due, this loop visits it
            // again next frame, and the answer below keeps the backend drawing
            // until the slot frees and `give_back` clears both timers. A busy
            // slot costs a frame, which is what it costs everywhere else.
            if self.panes.get(id).is_some_and(Pane::answered) {
                self.give_back(id, now);
                continue;
            }
            if let Some(pane) = self.panes.get_mut(id) {
                pane.stop_closing();
            }
            let Some(window) = self.panes.get(id).and_then(Pane::client).cloned() else {
                // Nothing to ask. A window whose application never arrived is
                // gone when we say it is, which is the one case where closing
                // is entirely ours to decide. Its fade has just landed, so it
                // is already invisible and there is nothing left to draw.
                //
                // Told first and removed after, as on every route (#126): the
                // pane is still here for `close`'s own snapshot, as a window
                // that is leaving.
                if self.panes.get(id).is_some() {
                    self.trigger_close(id);
                    self.panes.remove(id);
                }
                continue;
            };
            // A request, not a kill: the client decides whether it can close,
            // and the window goes away when it does. Either protocol -- an X11
            // window used to be animated away and then asked *nothing*, so it
            // never closed and never came back, which from the other side of
            // the screen is a window that vanished.
            if let Some(toplevel) = window.toplevel() {
                toplevel.send_close();
            } else if let Some(x11) = window.x11_surface()
                && let Err(err) = x11.close()
            {
                tracing::warn!(?err, "could not ask an X11 window to close");
            }
            // Watched either way. Whether the request went out matters less
            // than whether the window is still here a moment later, and a
            // window we could not even ask is the one most in need of coming
            // back.
            if let Some(pane) = self.panes.get_mut(id) {
                pane.mark_asked(now);
            }
            // **And the keyboard leaves with the pixels.** After `mark_asked`,
            // not before: `stop_closing` above has already cleared the other
            // half of `Pane::leaving`, so until this line the pane would still
            // be a candidate for the keyboard it is about to give up.
            self.hand_off_keyboard(&window);
        }
        // Asked after the loop, not before it: `trigger_close` runs a script,
        // and a script that closes another window during it starts a timer
        // this answer has to count. That was true of `!self.closing.is_empty()`
        // in the same position, and is the reason this is a second pass rather
        // than a flag gathered during the first.
        self.panes.iter().any(|pane| pane.closing_at().is_some())
    }

    /// Who a press at `location` would belong to.
    ///
    /// The three links of [`claim_of`], fetched in the order `pointer_button`
    /// fetches them. Read-only: the surface link is a geometric claim rather
    /// than a delivery, for the reasons [`Self::surface_claiming`] gives.
    pub(crate) fn claim_under(&self, location: Point<f64, Logical>) -> Claim {
        claim_of(
            self.surface_claiming(true, location).is_some(),
            self.script_grab,
            self.chrome_under(location).map(|under| under.chrome),
        )
    }

    /// Say what the pointer is over the compositor's own chrome, or stop
    /// saying.
    ///
    /// **The one writer of the assertion, and it follows the press's whole
    /// precedence chain rather than its last link.** Issue #108 is the pointer
    /// describing an action other than the one a press will take, and the first
    /// fix for it read [`Self::chrome_under`] alone — which is the third thing
    /// `pointer_button` asks and not the first. So a thumbnail's corner in
    /// overview drew `NwseResize` while the press focused the window and left
    /// the mode, and the bottom edge of a scripted bar drew `NsResize` while
    /// the press went to the bar. [`Self::claim_under`] is the whole chain, and
    /// `Claim::cursor` says nothing for every link that is not chrome: where
    /// the press defers, the pointer defers with it.
    ///
    /// **Not while a drag is in progress.** A resize grab takes the pointer off
    /// the border it started on within a pixel of movement — the window
    /// follows, but the pointer is ahead of it, and past the window's edge
    /// entirely once the drag hits a minimum size or a screen edge.
    /// Recomputing would drop the resize cursor mid-drag and hand the pointer
    /// back to whatever client the cursor happened to be over, which is the one
    /// moment the shape must not change. Holding the last assertion for the
    /// length of the grab is also what makes a move drag keep the arrow, and
    /// what lets `input::pointer_button` assert a shape *as* it starts a grab
    /// and have it stay for the drag.
    pub(crate) fn assert_cursor(&mut self, location: Point<f64, Logical>, grabbed: bool) {
        if grabbed {
            return;
        }
        let icon = self.claim_under(location).cursor();
        if self.pointer.assert(icon) {
            self.redraw = true;
        }
    }

    /// The drag icon to draw, if there is a live one.
    ///
    /// **The dead-surface downgrade is the point of having a reader at all**,
    /// and it is the same one [`crate::cursor::Pointer::showing`] does for a
    /// client's cursor surface, for the same reason. A client that dies
    /// mid-drag never reaches [`ClientDndGrabHandler::dropped`]: the grab is
    /// unset when the buttons come up, and if the application is gone the
    /// buttons may never come up — the pointer is still grabbed by a `DnDGrab`
    /// whose origin no longer exists. So the field can outlive the client, and
    /// a destroyed surface handed to `render_elements_from_surface_tree` is a
    /// tree with no buffer in it: nothing drawn, and a stale icon kept forever.
    /// Clearing it on read gets rid of both.
    ///
    /// Takes `&mut self` because of that write, which is why this is not a
    /// plain getter.
    pub(crate) fn dnd_icon(&mut self) -> Option<WlSurface> {
        if let Some(icon) = self.dnd_icon.as_ref()
            && !icon.alive()
        {
            self.dnd_icon = None;
        }
        self.dnd_icon.clone()
    }

    /// Say again what the pointer is over, for a pointer that has not moved.
    ///
    /// **The other half of #108, and the one a motion handler cannot reach.**
    /// The compositor asserts its cursor when the pointer crosses into a
    /// titlebar or onto a resize border — but the crossing can also be the
    /// *window's*: a layout change, an animation landing, a workspace switch,
    /// a window resized by a script all move chrome under a pointer that is
    /// standing still, and there is no motion event for that. Without this a
    /// titlebar that slid under the pointer would keep whatever resize arrow
    /// was being shown a moment ago, which is the same disagreement between
    /// the shape and the action, arrived at from the other direction. Entering
    /// overview is the same crossing: nothing moved but the claim, and the
    /// pointer has to hear about it.
    ///
    /// **Called from [`crate::render::prepare`], not from [`Self::settle`].**
    /// `settle` runs after the frame it settles, so a titlebar sliding under a
    /// stationary pointer was drawn once with the previous shape and only
    /// corrected on the frame the self-inflicted damage bought. `prepare` runs
    /// once per frame ahead of every output, before any cursor element is
    /// built, so the shape this finds is the shape that frame draws.
    ///
    /// Once a frame is enough, and cheap: [`crate::cursor::Pointer::assert`]
    /// answers `false` when nothing changed, so the ordinary case costs one
    /// hit test and no damage. A frame that is not drawn is a screen on which
    /// nothing moved, so there is nothing to have missed.
    pub(crate) fn reassert_cursor(&mut self) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        self.assert_cursor(pointer.current_location(), pointer.is_grabbed());
    }

    /// Retire transforms that have landed, and say whether anything still
    /// needs the next frame.
    ///
    /// Every pane is visited deliberately: a short-circuiting check would leave
    /// later panes transformed forever.
    pub(crate) fn settle(&mut self, now: std::time::Duration) -> bool {
        let mut animating = false;
        for pane in self.panes.iter() {
            animating |= present::settle(pane, now);
        }
        // And the selections, which animate on the same clock and damage
        // nothing either. Not folded into the loop above: a group is not a
        // pane, and one that has landed has to be released exactly once.
        animating |= self.groups.settle(now);
        // A window that has finished leaving is told to close; until then the
        // session counts as animating so the frames keep coming.
        animating |= self.settle_closing(now);
        // A window whose application never turned up gives up its slot.
        self.settle_loading(now);
        // One whose client has gone and whose fade is over is dropped, and
        // until then the frames keep coming: nothing else asks for one while
        // it fades, since its client can no longer damage anything.
        animating |= self.settle_leaving(now);
        // And one asked to close that is still here comes back.
        animating |= self.settle_refused(now);
        self.animating = animating;
        animating
    }

    /// Drop every pane whose client has gone and whose fade is over, and say
    /// whether any is still fading.
    ///
    /// **On the clock alone** (`pane::LEAVING`), because that is the only
    /// thing about a fade anyone can observe: `present::close` never releases
    /// its transform, so "the animation finished" is not a state. Called from
    /// every frame's `settle`, so a pane is gone within a frame of its
    /// deadline with nothing else having to happen -- no client event, no
    /// `sync_panes`, no layout. `a_window_that_closes_itself_fades_out_and_is_gone_on_time`.
    pub(crate) fn settle_leaving(&mut self, now: std::time::Duration) -> bool {
        let done: Vec<crate::pane::PaneId> = self
            .panes
            .iter()
            .filter(|pane| pane.faded_out(now))
            .map(Pane::id)
            .collect();
        for id in done {
            if self.panes.remove(id) {
                self.redraw = true;
            }
        }
        self.panes.iter().any(Pane::ghost)
    }

    /// Act on a resize an edge drag asked for.
    ///
    /// Offered to layouts first. Only a window no layout claims is resized
    /// directly, which is what keeps a tiled window from growing over its
    /// neighbour instead of moving the seam between them.
    ///
    /// **This is the fork between the two resize paths, and it forks on who
    /// decides the rectangle — not on who talks to the client.** The claimed
    /// branch is the *tiled* path: the layout moves a seam and `move_pane`
    /// writes what the layout decided. #120 lives entirely on that side, in
    /// `Tiling::drag_seam` — which seam a dragged edge moves. The unclaimed
    /// branch is the *floating* path: `hold_resize` makes the drag's own
    /// rectangle authoritative until the client catches up, which is #113.
    ///
    /// What used to stand here said "nothing here sizes the window" of the
    /// claimed branch and that the two fixes never touch each other's side.
    /// Both were false, and issue #123 is what they cost: `move_pane` sizes the
    /// window, once per pane per frame, and the client-facing half of a resize —
    /// throttle the configure, keep the slot authoritative while the client
    /// catches up, bridge the last buffer, end on any answer or on `PATIENCE` —
    /// is the same problem whichever branch decided the rectangle. It is now the
    /// same code: [`Self::resize_bridge`] for the claimed branch, `resize_hold`
    /// for the unclaimed one, both out of `crate::resizing`.
    ///
    /// **The fork is per frame, not per gesture**, which is why arming is
    /// bracketed around the call rather than done after it. `trigger_resize`
    /// runs `apply` itself, so the layout's whole sweep — every `move_pane` it
    /// causes — happens inside that call and before this function learns
    /// whether the drag was claimed at all. A handler that returns without
    /// placing anything (`scrolling.lua`'s guard at screen x 0 does exactly
    /// that, and calls itself a defect) drops a single frame onto the unclaimed
    /// branch mid-gesture; the two must therefore be able to hand a pane back
    /// and forth without either leaving a rectangle behind.
    ///
    /// Once a frame, not once per pointer event, and the difference is the
    /// whole reason this is here rather than in the motion handler. A mouse
    /// reports movement up to a thousand times a second; each report was
    /// dispatching to Lua, laying out every window, and sending a configure to
    /// every client. Clients cannot answer at that rate and do not try — they
    /// fall behind, and the window being dragged stutters against the pointer
    /// instead of following it.
    ///
    /// `pending_resize` is one slot, so the last position before the frame is
    /// the one that counts. That is exactly the coalescing this wants: the
    /// pointer is wherever it is now, and the positions it passed through since
    /// the last frame are of no interest to anyone.
    ///
    /// Coalescing is the *rate* the client is asked at, not the rate the
    /// window moves at. Since #113 those are two different numbers:
    /// `hold_resize` puts the pane where the drag says on every one of these
    /// calls, and `crate::resizing::TELL_EVERY` decides which of them the
    /// client hears about.
    ///
    /// Returns whether anything was resized.
    pub(crate) fn settle_resize(&mut self) -> bool {
        let now = self.clock.now();
        let dragged = match self.pending_resize.take() {
            Some(request) => {
                // Armed before the layout is asked, because the layout's sweep
                // runs inside the asking. See [`Self::arm_resize_gesture`].
                self.arm_resize_gesture(&request);
                let claimed = self.trigger_resize(&request);
                self.resize_gesture = None;
                if claimed {
                    // A layout took it: the window's rectangle is the layout's
                    // arithmetic and went through `move_pane`, which wrote the
                    // slot, the space and — on the throttle's schedule — the
                    // client. The bridge armed above is now watching every pane
                    // it moved, so a floating hold on this window would be a
                    // second authority over one of them.
                    //
                    // This window's hold and no other's, which is the same care
                    // `begin_resize` takes and for the same reason: a hold on
                    // some other window is a gesture this one knows nothing
                    // about, and dropping it abandons that window's rectangle
                    // half-reconciled.
                    self.drop_resize_hold_for(&request.window);
                } else {
                    // **Nobody placed anything this frame, so the dragged pane
                    // goes back to the floating path.** `hold_resize` takes its
                    // slot, its authority and — since it is the same client in
                    // the same gesture — its hold; see `Hold::retargeted` there
                    // for why the hold is carried rather than rebuilt. Every
                    // *other* pane the gesture has moved keeps its own: those
                    // panes are not on the floating path, nothing else is
                    // watching them, and a layout that unclaims one frame and
                    // claims the next — which is `modes.lua` and `scrolling.lua`
                    // today — would otherwise abandon them mid-gesture with
                    // slots the client never agreed to.
                    self.hold_resize(&request, now);
                }
                self.redraw = true;
                true
            }
            None => false,
        };
        // **Before settling, and on every frame rather than on the ones that
        // carried a motion.** See [`Self::flush_resize`]: the throttle's own
        // trailing edge is the last thing a paused drag is waiting for, and a
        // paused drag is what every drag is for the moment before the button
        // comes up.
        self.flush_resize(now);
        // Whether or not the pointer moved this frame: a hold outlives the
        // gesture by however long the client takes to answer the last
        // configure, and something has to be watching for that answer.
        let settled = self.settle_resize_hold(now) | self.settle_resize_bridge(now);
        // A recorded release exists to be handed to a hold that had not been
        // born yet. If nothing survived this frame, nothing can be: the only
        // things that create one are `hold_resize` and `offers_size`, and the
        // only thing that reaches either is a motion from a grab which the
        // release has already ended. See [`Self::resize_ended`].
        if self.resize_hold.is_none() && self.resize_bridge.is_none() {
            self.resize_ended = None;
        }
        settled || dragged
    }

    /// Tell `move_pane` that the layout sweep it is about to see belongs to a
    /// live edge drag.
    ///
    /// Everything about the bridge's lifetime is decided here, so the three
    /// cases are together:
    ///
    /// * **No bridge.** One is created, empty. `offers_size` fills it with
    ///   whichever panes the sweep actually moves, which may be none — a
    ///   `resize` listener that claims the drag by running a command about some
    ///   other window claims it just as hard as one that lays anything out.
    ///   A purely floating drag therefore creates one here and
    ///   `settle_resize_bridge` drops it again at the end of the same frame,
    ///   every frame. That is not worth deferring: `Vec::new` does not
    ///   allocate, so the whole of the per-frame cost is one `Arc` refcount
    ///   either way, and the alternative — the gesture carrying the window so
    ///   the bridge can be built lazily at the first arm — puts a second copy
    ///   of "which window is being dragged" in the compositor for two atomic
    ///   increments a frame.
    /// * **A bridge for this same window.** The previous gesture on it ended and
    ///   its holds are waiting out `PATIENCE`. They are handed to this gesture
    ///   rather than reconciled: reconciling means adopting, and adopting a
    ///   tiled pane takes it off its tile, so a border nudged twice in a quarter
    ///   of a second would snap every pane the first nudge moved. See
    ///   `resizing::Hold::rearm`.
    /// * **A bridge for a different window.** That gesture is over and this one
    ///   will never place its panes, so it is ended the way its own deadline
    ///   would have ended it. The same rule `begin_resize` applies to a floating
    ///   hold, and for the same reason: a deadline expiring in the middle of
    ///   somebody else's gesture adopts whatever size the client happened to be
    ///   at.
    fn arm_resize_gesture(&mut self, request: &ResizeRequest) {
        let released = self
            .resize_ended
            .as_ref()
            .filter(|(ended, _)| ended == &request.window)
            .map(|&(_, at)| at);
        match self.resize_bridge.as_mut() {
            Some(bridge) if bridge.window == request.window => {
                for held in &mut bridge.panes {
                    held.hold.rearm(released);
                }
            }
            Some(_) => {
                self.adopt_bridge();
                self.resize_bridge = Some(Bridged {
                    window: request.window.clone(),
                    panes: Vec::new(),
                });
            }
            None => {
                self.resize_bridge = Some(Bridged {
                    window: request.window.clone(),
                    panes: Vec::new(),
                });
            }
        }
        self.resize_gesture = Some(Gesture { released });
    }

    /// Whether this pane's slot is the authority on its window's size just now.
    ///
    /// Either path can be the reason. A pane is under exactly one of them —
    /// `settle_resize` hands a pane from one to the other rather than letting
    /// both claim it — so this is an "or" and not a precedence.
    pub(crate) fn holding_resize(&self, pane: crate::pane::PaneId) -> bool {
        self.held_hold(pane).is_some()
    }

    /// The hold governing this pane, whichever path put it there.
    ///
    /// The floating slot first because it is one comparison; the bridge is a
    /// short list — the panes one gesture moved — walked only when the first
    /// misses.
    fn held_hold(&self, pane: crate::pane::PaneId) -> Option<&crate::resizing::Hold> {
        if let Some(held) = self.resize_hold.as_ref().filter(|held| held.pane == pane) {
            return Some(&held.hold);
        }
        self.bridged(pane).map(|held| &held.hold)
    }

    /// The same, to be written to. See [`Self::flush_resize`], which is the
    /// only caller: a trailing flush has to update `asked` and `told` on
    /// whichever of the two authorities is holding this pane, and it has no
    /// business knowing which.
    fn held_hold_mut(&mut self, pane: crate::pane::PaneId) -> Option<&mut crate::resizing::Hold> {
        if let Some(held) = self
            .resize_hold
            .as_mut()
            .filter(|held| held.pane == pane)
            .map(|held| &mut held.hold)
        {
            return Some(held);
        }
        self.resize_bridge
            .as_mut()?
            .panes
            .iter_mut()
            .find(|held| held.pane == pane)
            .map(|held| &mut held.hold)
    }

    /// This pane's entry in the bridge, if a tiled gesture is moving it.
    fn bridged(&self, pane: crate::pane::PaneId) -> Option<&crate::resizing::Held> {
        self.resize_bridge
            .as_ref()?
            .panes
            .iter()
            .find(|held| held.pane == pane)
    }

    /// Take this pane out of the bridge without reconciling anything.
    ///
    /// For the one case where something else has taken over its rectangle: the
    /// floating path claiming a pane the layout has stopped placing.
    fn drop_bridged(&mut self, pane: crate::pane::PaneId) {
        drop(self.take_bridged(pane));
    }

    /// The same, handing back what was taken.
    ///
    /// The floating path wants the hold rather than merely wanting it gone:
    /// it is the same client in the same gesture, so its throttle, its `asked`
    /// and its refusal all still apply. See `hold_resize`.
    fn take_bridged(&mut self, pane: crate::pane::PaneId) -> Option<crate::resizing::Held> {
        let bridge = self.resize_bridge.as_mut()?;
        let at = bridge.panes.iter().position(|held| held.pane == pane)?;
        Some(bridge.panes.swap_remove(at))
    }

    /// What fills this pane while its client catches up, or `None` if it is not
    /// being dragged. See [`crate::resizing::factor`].
    pub(crate) fn resize_fill(&self, pane: crate::pane::PaneId) -> Option<crate::resizing::Fill> {
        Some(self.held_hold(pane)?.fill(self.resizing.fill))
    }

    /// Which of this pane's edges a live drag is pulling, or `None` if none is.
    ///
    /// For a fill that does not stretch: the picture has to stay against the
    /// edges that are standing still, or it travels with the pointer and the
    /// window's contents slide about inside their own frame. See
    /// [`crate::resizing::Fill::Hold`].
    ///
    /// **A tiled pane's edges are its own**, which for the neighbour across a
    /// seam is the opposite side from the one the pointer has hold of, and for a
    /// pane merely pushed aside is neither. See [`crate::resizing::moved_edges`].
    pub(crate) fn resize_pins(&self, pane: crate::pane::PaneId) -> Option<(bool, bool)> {
        Some(self.held_hold(pane)?.pins())
    }

    /// The slot a held window's pane is keeping, if this is that window.
    ///
    /// `sync_panes` asks: the space is normally the authority it copies into
    /// every pane's slot, and copying it over a held slot would undo the drag
    /// between one frame and the next — silently, which is exactly how the
    /// first attempt at `rescue_offscreen` went wrong.
    ///
    /// **A tiled drag is the same disagreement, once per moved pane.** Before
    /// #123 the layout wrote a slot and `sync_panes` overwrote it with the
    /// client's committed rectangle on the very next frame, so the authority the
    /// layout had just asserted survived exactly as long as the transform
    /// `move_pane` left behind — which, at `duration = 0`, is until `Solium::settle`
    /// runs after the same frame's render. That is the alternation in #123: a
    /// frame carrying a motion drew the layout's rectangle, and a frame without
    /// one drew the client's.
    fn held_slot(&self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        let pane = self
            .resize_hold
            .as_ref()
            .filter(|held| &held.window == window)
            .map(|held| held.pane)
            .or_else(|| {
                self.resize_bridge
                    .as_ref()?
                    .panes
                    .iter()
                    .find(|held| &held.window == window)
                    .map(|held| held.pane)
            })?;
        self.panes.get(pane).map(Pane::slot)
    }

    /// A fresh edge drag is starting on this window.
    ///
    /// The previous drag's hold may still be live: its deadline runs for a
    /// quarter of a second after the button came up, and a second drag can
    /// easily begin inside that — a border nudged twice, or a double-click that
    /// turns into a drag. Inheriting it would let the *old* gesture's deadline
    /// expire in the middle of the new one and adopt whatever size the client
    /// happened to be at, which is the shake back again with a longer period.
    ///
    /// Holds on other windows are left alone: one of those expiring reconciles
    /// its own window correctly and has nothing to do with this drag.
    ///
    /// **Reconciled rather than abandoned, and the rectangle it lands on is the
    /// answer.** Simply forgetting the old hold leaves the pane's slot holding
    /// a size the client never agreed to with nothing left watching for the
    /// answer, so `pane_geometry` falls back to `real_geometry` — the drag's
    /// origin paired with the client's old size, which is issue #113's
    /// rectangle exactly, for every frame until the new drag's first motion.
    /// Ending it the way the deadline would ends it *somewhere*, which is all
    /// the next gesture needs.
    ///
    /// Returning the rectangle is what stops that reconciliation being visible:
    /// a `ResizeGrab` computes every frame from the rectangle it was given, so
    /// a caller that read one before this ran would drag from a rectangle this
    /// has since changed and the window would jump on the first motion.
    /// `None` for a window with no pane, which is not a case any caller can
    /// reach — every one of them found the window through a pane — and each has
    /// its own rectangle to fall back on.
    pub(crate) fn begin_resize(&mut self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        if self
            .resize_hold
            .as_ref()
            .is_some_and(|held| &held.window == window)
        {
            let taken = window.geometry().size;
            self.adopt_resize(taken);
        }
        // **And the tiled half of the same problem, which is not reconciled but
        // stopped.** `arm_resize_gesture` already rearms this window's bridge,
        // and that runs on the first *motion* — so a press, a pause and then a
        // drag leaves the previous gesture's deadline running across the whole
        // of the pause, and `settle_resize_bridge` runs every frame. A press
        // held for longer than `PATIENCE` therefore adopts every pane the last
        // drag moved, which is exactly the snap `resizing::Hold::rearm` exists
        // to prevent, arriving between the press and the first pixel of motion.
        //
        // Stopped rather than adopted, for `rearm`'s reason: adopting a tiled
        // pane takes it off its tile, so a border nudged twice inside a quarter
        // second would snap every pane the first nudge moved. The new gesture
        // owns these panes and will keep placing them.
        //
        // A bridge belonging to a *different* window is left alone. That
        // gesture's deadline expiring reconciles its own panes correctly and
        // has nothing to do with this drag — the same rule the floating hold
        // above follows, and `arm_resize_gesture` ends it on the first motion.
        if let Some(bridge) = self
            .resize_bridge
            .as_mut()
            .filter(|bridge| &bridge.window == window)
        {
            for held in &mut bridge.panes {
                held.hold.rearm(None);
            }
        }
        // Whatever gesture that record belonged to, it is not this one.
        self.resize_ended = None;
        self.pane_outer_of(self.panes.id_of(window)?)
    }

    /// Let go of a hold without reconciling anything.
    ///
    /// For the cases where the rectangle has been decided by someone else — a
    /// layout claiming the drag, the window going away — rather than by the
    /// client answering. Nothing to adopt: whoever took over owns the
    /// rectangle now.
    fn drop_resize_hold(&mut self) {
        self.resize_hold = None;
    }

    /// The same, for one window's hold and nobody else's.
    fn drop_resize_hold_for(&mut self, window: &Window) {
        if self
            .resize_hold
            .as_ref()
            .is_some_and(|held| &held.window == window)
        {
            self.drop_resize_hold();
        }
    }

    /// End a hold by letting the client's own size win.
    ///
    /// The rule [`crate::resizing::Settle::Adopt`] names, in one place because
    /// two things reach it: the deadline expiring, and a fresh gesture arriving
    /// before it does. Either way the rectangle the old hold was waiting on
    /// will never be agreed, and the slot has to stop claiming it — pinned to
    /// the edges that drag was not holding, or the gesture ends by moving the
    /// one edge the user never touched.
    fn adopt_resize(&mut self, taken: Size<i32, Logical>) {
        let Some(held) = self.resize_hold.as_ref() else {
            return;
        };
        let (window, pane, hold) = (held.window.clone(), held.pane, held.hold);
        self.drop_resize_hold();
        self.land_on(&window, pane, &hold, taken);
    }

    /// The same for every pane a tiled gesture moved, all at once.
    ///
    /// Used where a whole bridge has been orphaned — a gesture starting on a
    /// different window while this one's holds are still waiting — which is the
    /// bridge's version of what `begin_resize` does to a stale floating hold.
    /// Each pane lands on its own client's size, because each was waiting on its
    /// own client.
    fn adopt_bridge(&mut self) {
        let Some(bridge) = self.resize_bridge.take() else {
            return;
        };
        for held in bridge.panes {
            let taken = held.window.geometry().size;
            self.land_on(&held.window, held.pane, &held.hold, taken);
        }
    }

    /// Put one pane's slot where the client's own size says, and stop holding it.
    ///
    /// The arithmetic behind both of the above, in one place because there are
    /// now four ways in: the deadline expiring, a fresh gesture arriving before
    /// it does, a client refusing a size it was offered, and a whole tiled
    /// gesture being orphaned.
    ///
    /// **The slot stops filling its tile, and that is the honest outcome rather
    /// than a shortcut.** A tiled pane whose client will not take the size the
    /// layout gave it has a gap on one side of it whatever this does; putting
    /// the gap against the edge that moved — which is what `anchored` does with
    /// this pane's own edges — is the difference between the window staying
    /// where the user put it and its far edge walking across the desktop. Issue
    /// #115 is where reading a client's minimum belongs, and until then a
    /// refusal is only visible here.
    fn land_on(
        &mut self,
        window: &Window,
        pane: crate::pane::PaneId,
        hold: &crate::resizing::Hold,
        taken: Size<i32, Logical>,
    ) {
        let Some(slot) = self.panes.get(pane).map(Pane::slot) else {
            return;
        };
        let landed = hold.anchored(slot, taken);
        if let Some(pane) = self.panes.get_mut(pane) {
            pane.set_slot(landed);
        }
        self.map_stacked(window.clone(), landed.loc, false);
        self.redraw = true;
        tracing::debug!(
            asked = ?slot.size,
            given = ?taken,
            "a client settled at a size of its own"
        );
    }

    /// Put the pane where the drag says, now, and let the client catch up.
    ///
    /// **This is the fix for #113.** What it does *not* do is as important as
    /// what it does: it does not wait for the client, and it does not derive
    /// the window's rectangle from the client's size. The pane's slot takes the
    /// dragged rectangle whole — origin and size in the same frame — so the
    /// edge under the pointer is the edge that moves and the opposite edge does
    /// not move at all.
    ///
    /// The space is told the new *position* in the same breath, deliberately.
    /// Position never needed a client's consent; only size does. Holding the
    /// position back too would put the space and the slot into exactly the
    /// disagreement issue #84 is about, for no gain, and would break every
    /// reader of `real_geometry` for the length of a drag.
    fn hold_resize(&mut self, request: &ResizeRequest, now: Duration) {
        let Some(pane) = self.panes.id_of(&request.window) else {
            // A window with no pane has nowhere to hold a rectangle. That is
            // not a case anyone should reach — `sync_panes` gives every client
            // in the space a pane — but the old behaviour is a correct
            // fallback rather than a guess, so take it and say nothing.
            self.resize_to(&request.window, request.wanted);
            return;
        };
        // **`insets_of`, which is what every reader of this slot uses.**
        // `frame_insets` is the other spelling and it answers differently for
        // `Frame::Pending`: `is_decorated` is false there, so it reserves
        // nothing, while `insets_for` reserves a titlebar so that a window does
        // not change shape when its frame arrives. A pane whose decoration
        // failed to build is `Pending` *permanently* — see
        // `decoration::Decorations::insert` — so writing the slot with one
        // spelling and reading it back with the other would re-grow the
        // rectangle by a titlebar on every frame of the drag: the top edge a
        // titlebar above where the pointer is and a client sized that much too
        // tall. The round trip `pane_outer(inner(wanted)) == wanted` that this
        // whole fix rests on holds only when both halves ask the same question.
        let client = inner(request.wanted, self.insets_of(pane));
        if let Some(held) = self.panes.get_mut(pane) {
            held.set_slot(client);
        }
        self.map_stacked(request.window.clone(), client.loc, false);

        let size = request.window.geometry().size;
        // **This pane's hold, wherever the last frame left it.** A bridge entry
        // means the layout claimed the drag on an earlier frame and has stopped
        // claiming it — `scrolling.lua`'s guard at screen x 0 is one frame of
        // exactly that — and it is still the same client in the same gesture,
        // so it keeps its throttle, its `asked`, and whether it has refused
        // anything. Rebuilding instead reset `Hold::told`, which handed every
        // flip a free configure in each direction; a handler that alternated
        // therefore restored the sixty a second `crate::resizing::TELL_EVERY`
        // exists to remove.
        //
        // Taken and not copied: leaving it in the bridge would make two things
        // answer `holding_resize` for one pane with two different opinions
        // about which edges are pulled.
        let carried = self.take_bridged(pane).map(|mut held| {
            // The pointer's edges now, because this pane is the one under the
            // hand again. A tiled pane's edges are derived from what moved; a
            // floating one's are the grab's. See `crate::resizing::moved_edges`.
            held.hold.retargeted(request.edges);
            held.hold
        });
        match &mut self.resize_hold {
            // The same drag, still going. The client hears about it on the
            // throttle's schedule, not this frame's.
            //
            // A `carried` here would mean one pane held by both authorities at
            // once, which `settle_resize` hands back and forth precisely to
            // avoid; dropping it is how that is repaired rather than an
            // oversight, and the floating hold is the newer of the two.
            Some(held) if held.pane == pane => {
                if let Some(tell) = held.hold.dragged(client, size, now) {
                    size_window(&request.window, tell);
                }
            }
            // A new drag — or a drag that has moved to a different window,
            // which a grab cannot do but a script rebinding one could. Either
            // way the previous hold is over and its window keeps whatever
            // rectangle it last had.
            //
            // **Born knowing whether its gesture is still going.** It very
            // often is not: the button comes up during input dispatch and this
            // runs at the frame, so every gesture quick enough to fit in one
            // dispatch batch arrives here already over. A hold that took
            // `released: None` regardless would wait for a release that has
            // already happened, which never comes again — see
            // [`Self::resize_ended`] and `resizing::Hold::new`.
            _ => {
                let released = self
                    .resize_ended
                    .as_ref()
                    .filter(|(ended, _)| ended == &request.window)
                    .map(|&(_, at)| at);
                let hold = match carried {
                    Some(mut hold) => {
                        hold.rearm(released);
                        if let Some(tell) = hold.dragged(client, size, now) {
                            size_window(&request.window, tell);
                        }
                        hold
                    }
                    None => {
                        size_window(&request.window, client);
                        crate::resizing::Hold::new(request.edges, size, client, now, released)
                    }
                };
                self.resize_hold = Some(crate::resizing::Held {
                    window: request.window.clone(),
                    pane,
                    hold,
                });
            }
        }
    }

    /// The pointer has let go of an edge drag.
    ///
    /// Sends the final configure — the one the throttle must not get the last
    /// word on — and starts the deadline. The pane keeps the rectangle the
    /// gesture ended on until the client answers or the deadline runs out; see
    /// `settle_resize_hold`.
    ///
    /// **Records the release whether or not there is a hold to record it on**,
    /// which is the whole of why a gesture can always be let go of. See
    /// [`Self::resize_ended`].
    ///
    /// Called from inside the pointer grab, so it must not touch the seat. It
    /// does not: a clock, a pane's slot, the request the last motion left
    /// behind, and one configure.
    pub(crate) fn release_resize(&mut self, window: &Window) {
        let now = self.clock.now();
        // **Written down first, and unconditionally.** There may be no hold yet
        // — a press, a motion and this release inside one dispatch batch all
        // run before the frame that creates one — and a release that went
        // unrecorded because there was nothing to record it on would leave the
        // hold born a moment later waiting for it for ever. See
        // [`Self::resize_ended`].
        self.resize_ended = Some((window.clone(), now));
        self.release_bridge(window, now);

        let Some(held) = self.resize_hold.as_ref() else {
            return;
        };
        if &held.window != window {
            return;
        }
        let Some(slot) = self.panes.get(held.pane).map(Pane::slot) else {
            self.drop_resize_hold();
            return;
        };
        // **The rectangle the gesture ended on, which is not always the slot's.**
        // The last motion of a drag is routinely still sitting in
        // `pending_resize` when the button comes up — a grab's callbacks run
        // during input dispatch and `settle_resize` runs at the frame — so the
        // slot is one motion out of date here. Telling the client the stale size
        // sends two configures for one release, and if it answers the first,
        // `Hold::note` records that as a refusal of a size it was never offered:
        // `settle` then takes the `declined == asked` path and adopts the
        // *pre-release* rectangle on the spot, throwing away the end of the
        // drag rather than waiting for the answer to the size that was.
        let pane = held.pane;
        let client = self
            .pending_resize
            .as_ref()
            .filter(|request| &request.window == window)
            .map_or(slot, |request| inner(request.wanted, self.insets_of(pane)));
        let size = window.geometry().size;
        let Some(held) = self.resize_hold.as_mut() else {
            return;
        };
        let tell = held.hold.release(client, size, now);
        size_window(window, tell);
        self.redraw = true;
    }

    /// The same for a tiled gesture: one final configure per pane it moved.
    ///
    /// **This is what stops the throttle losing the end of a drag.** A pane's
    /// hold keeps `asked` at the rectangle the client was last *told*, so a pane
    /// whose last change fell inside an interval has an `asked` the drag has
    /// already moved on from. Left there, its client would answer the size
    /// before that one, `settle` would wait out `PATIENCE` for an answer that
    /// cannot come, and the gesture would end by snapping the pane to a size
    /// from the middle of the drag. `Hold::release` sends unconditionally for
    /// exactly this reason on the floating path; the tiled path needs it once
    /// per moved pane.
    ///
    /// The slot is the rectangle, and unlike the floating path there is no
    /// pending motion to reconcile against: the layout — not the pointer —
    /// decides a tiled pane's rectangle, and the layout's last word is what is
    /// in the slot. A motion still sitting in `pending_resize` reaches the
    /// layout on the next frame and `Hold::dragged` sends it then, unthrottled,
    /// because a hold that has been released no longer consults the interval.
    fn release_bridge(&mut self, window: &Window, now: Duration) {
        let Some(bridge) = self.resize_bridge.as_ref() else {
            return;
        };
        if &bridge.window != window {
            return;
        }
        let moved: Vec<(Window, crate::pane::PaneId)> = bridge
            .panes
            .iter()
            .map(|held| (held.window.clone(), held.pane))
            .collect();
        for (client, pane) in moved {
            // The pane or its client has gone, or the pane has been given a
            // different client since the gesture started. The same check
            // `settle_resize_bridge` makes, and for the same reason: a
            // `size_window` on the window this hold remembers would configure a
            // client that no longer owns the rectangle being sent, and the slot
            // read below is not that client's anyway.
            if self.panes.get(pane).and_then(Pane::client) != Some(&client) {
                continue;
            }
            let Some(slot) = self.panes.get(pane).map(Pane::slot) else {
                continue;
            };
            let committed = client.geometry().size;
            let Some(held) = self
                .resize_bridge
                .as_mut()
                .and_then(|bridge| bridge.panes.iter_mut().find(|held| held.pane == pane))
            else {
                continue;
            };
            let tell = held.hold.release(slot, committed, now);
            size_window(&client, tell);
        }
        self.redraw = true;
    }

    /// Send the configure the throttle is still sitting on, once its interval
    /// has passed.
    ///
    /// **`crate::resizing::TELL_EVERY` is a rate, and a rate needs a trailing
    /// edge.** `Hold::dragged` is reached from `move_pane` and from
    /// `hold_resize`, and `settle_resize` reaches either only on a frame whose
    /// `pending_resize` carried a motion. So the offers a drag makes in the
    /// last interval before it stops moving were recorded in the pane's slot,
    /// drawn from the pane's slot, and never sent: the client sits at the size
    /// it was told up to a tenth of a second earlier while the pane is drawn
    /// where the pointer is, and the bridge between them is the whole of that
    /// gap — drag speed times the interval, which on an ordinary seam drag is
    /// tens of pixels of stretch or, under `Fill::Hold`, tens of pixels of
    /// uncovered background.
    ///
    /// **Pausing before releasing is what people do**, so this is the ordinary
    /// end of a drag rather than an edge case, and it is held until the pointer
    /// moves again or the button comes up. Before #123 the tiled path
    /// configured on every frame and so had no tail at all, which makes this
    /// exactly the symptom that was reported.
    ///
    /// Both paths, because neither had it. `settle_resize_hold` looked like the
    /// floating path's answer and is not: `Hold::settle` decides whether a hold
    /// is over and never sends anything, so a paused floating drag sat on its
    /// last offer in the same way. The only thing that ever sent unconditionally
    /// was the release.
    ///
    /// The slot is the rectangle, for the same reason `release_bridge` uses it:
    /// it is what `move_pane` and `hold_resize` wrote, so it is the offer the
    /// throttle swallowed. A hold whose slot it has already sent answers `None`
    /// and costs a comparison.
    fn flush_resize(&mut self, now: Duration) {
        let held: Vec<(Window, crate::pane::PaneId)> = self
            .resize_hold
            .iter()
            .chain(self.resize_bridge.iter().flat_map(|bridge| &bridge.panes))
            .map(|held| (held.window.clone(), held.pane))
            .collect();
        for (window, pane) in held {
            // The pane has been given a different client since the gesture
            // started, or has lost the one it had. The same check
            // `release_bridge`, `settle_resize_bridge` and `settle_resize_hold`
            // all make, and this is the site that needs it most: it runs first
            // of the four on every frame, so without it the *new* client's slot
            // is configured onto the window this hold remembers — and
            // `committed` below is read off that stale window too, so the
            // throttle's bookkeeping is answered about one client with the
            // other one's size.
            //
            // Skipped rather than dropped, which is `release_bridge`'s choice
            // and right for the same reason: the two settle passes run
            // immediately after this one on the same frame and each drops what
            // it owns. A flush that dropped holds would be deciding a lifetime
            // question from the function whose whole job is the throttle's
            // trailing edge.
            if self.panes.get(pane).and_then(Pane::client) != Some(&window) {
                continue;
            }
            let Some(slot) = self.panes.get(pane).map(Pane::slot) else {
                continue;
            };
            let committed = window.geometry().size;
            let Some(hold) = self.held_hold_mut(pane) else {
                continue;
            };
            let Some(tell) = hold.dragged(slot, committed, now) else {
                continue;
            };
            size_window(&window, tell);
            self.redraw = true;
            if crate::resizing::trace::on() {
                crate::resizing::trace::line(
                    "flush",
                    format_args!(
                        "pane={} slot={},{} {}x{} committed={}x{} asked={},{} {}x{} told=1 \
                         held=1 refused={} unanswered={}",
                        pane.get(),
                        slot.loc.x,
                        slot.loc.y,
                        slot.size.w,
                        slot.size.h,
                        committed.w,
                        committed.h,
                        tell.loc.x,
                        tell.loc.y,
                        tell.size.w,
                        tell.size.h,
                        u8::from(
                            self.held_hold(pane)
                                .is_some_and(crate::resizing::Hold::refused)
                        ),
                        self.held_hold(pane)
                            .map_or(0, crate::resizing::Hold::unanswered),
                    ),
                );
            }
        }
    }

    /// Watch a live hold for the client's answer, and end it when one comes.
    ///
    /// Returns whether the window's rectangle changed, which it only does in
    /// the case that is the whole reason this is careful: the client answered
    /// with a size that is not the one it was asked for, or answered nothing at
    /// all. See `crate::resizing::Settle`.
    fn settle_resize_hold(&mut self, now: Duration) -> bool {
        let Some(held) = self.resize_hold.as_ref() else {
            return false;
        };
        let (window, pane) = (held.window.clone(), held.pane);
        // The pane or its client has gone. A hold pointing at neither would
        // keep `holding_resize` true for a pane id that has been reused by
        // nothing, and there is no rectangle left to reconcile.
        if self.panes.get(pane).and_then(Pane::client) != Some(&window) {
            self.drop_resize_hold();
            return false;
        }
        let size = window.geometry().size;
        let Some(held) = self.resize_hold.as_mut() else {
            return false;
        };
        match held.hold.settle(size, now) {
            crate::resizing::Settle::Waiting => false,
            // The client is the size the pane is. Everything agrees again, so
            // there is nothing to hold and nothing to move.
            crate::resizing::Settle::Done => {
                self.drop_resize_hold();
                false
            }
            // **The client's answer wins.** It refused the size it was offered
            // — a minimum width, a cell grid — or it never answered at all, and
            // either way the alternative is a window drawn at a size its client
            // will never reach, stretched, for as long as it is open. One snap
            // at the end of a gesture is the cheaper of the two, and it is also
            // issue #115 becoming visible rather than staying hidden behind a
            // blur.
            crate::resizing::Settle::Adopt(taken) => {
                self.adopt_resize(taken);
                true
            }
        }
    }

    /// The same, once per pane a tiled gesture moved.
    ///
    /// Each pane settles on its own client's answer and on its own deadline,
    /// which is the whole reason there is a hold per pane rather than one for
    /// the gesture: a seam's two windows are two applications, and Firefox
    /// answering in 80 ms says nothing about the terminal beside it.
    ///
    /// **Every pane is visited, and a hold is never left behind for a pane or a
    /// client that has gone.** A hold pointing at neither would answer
    /// `holding_resize` for a pane id nothing owns, and `pane_geometry` would
    /// keep returning a slot forever.
    fn settle_resize_bridge(&mut self, now: Duration) -> bool {
        let Some(bridge) = self.resize_bridge.as_ref() else {
            return false;
        };
        let watching: Vec<(Window, crate::pane::PaneId)> = bridge
            .panes
            .iter()
            .map(|held| (held.window.clone(), held.pane))
            .collect();
        let mut moved = false;
        for (window, pane) in watching {
            // The pane or its client has gone; there is no rectangle left to
            // reconcile and nothing to reconcile it against.
            if self.panes.get(pane).and_then(Pane::client) != Some(&window) {
                self.drop_bridged(pane);
                continue;
            }
            let committed = window.geometry().size;
            let Some(held) = self
                .resize_bridge
                .as_mut()
                .and_then(|bridge| bridge.panes.iter_mut().find(|held| held.pane == pane))
            else {
                continue;
            };
            match held.hold.settle(committed, now) {
                // Still being dragged, or the client has not answered the last
                // configure yet. The layout's rectangle stays authoritative.
                crate::resizing::Settle::Waiting => {}
                // The client is the size the layout made it. Slot, space and
                // client agree again, so there is nothing left to hold.
                crate::resizing::Settle::Done => self.drop_bridged(pane),
                // **Any answer ends the bridge, including a refusal**, which is
                // the trap `crate::resizing` names and the one that cost a
                // permanently blurred window once already. A client with a
                // minimum size — Firefox has one, a terminal rounds to its cell
                // grid — will never reach what the layout offered, so waiting
                // for it means stretching a buffer towards a size nothing will
                // ever agree to, for as long as the window is open.
                crate::resizing::Settle::Adopt(taken) => {
                    let hold = held.hold;
                    self.drop_bridged(pane);
                    self.land_on(&window, pane, &hold, taken);
                    moved = true;
                }
            }
        }
        // An empty bridge is no bridge: `settle_resize` reads this to know
        // whether a recorded release can still be handed to anything.
        if self
            .resize_bridge
            .as_ref()
            .is_some_and(|bridge| bridge.panes.is_empty())
        {
            self.resize_bridge = None;
        }
        moved
    }

    /// Give the keyboard to something, if a window went and left it nowhere.
    ///
    /// Focus is a Wayland concept and belongs to a surface, so when the focused
    /// window's surface dies the seat is simply left holding nothing. Nothing
    /// takes it back: focus was only ever set when a window opened or the
    /// pointer moved. So closing a window meant the keyboard went dead, every
    /// binding that acts on "the focused window" stopped working, and the only
    /// way out was to move the mouse over something. From the other side of the
    /// screen that is a session that broke when you closed a window.
    ///
    /// The window under the pointer first, because with focus-follows-mouse
    /// that is where focus would land the moment you moved; the topmost
    /// otherwise. Called where a window went, and only when nothing has focus,
    /// so it cannot argue with a script that has just chosen one.
    ///
    /// **While locked, the only place the keyboard can go is a lock surface**,
    /// so that is where this sends it: to the lock screen on the monitor the
    /// pointer is on, or any surviving one, unless it is already on one. A
    /// window is never considered, and the gate in `focus.rs` would refuse one
    /// anyway. What this adds over the gate is the other half: when the lock
    /// surface the keyboard was on dies -- its monitor unplugged while locked --
    /// the keyboard is handed to one that is still there, rather than left on
    /// nothing until another maps (see `Solium::lock_surface_destroyed`).
    ///
    /// **Nor, just after an unlock, while the key that unlocked is still
    /// down.** `unlock` leaves the keyboard on nothing until every key is up,
    /// so that no window is told in `wl_keyboard.enter` that Enter is held, and
    /// `input::key` calls this once they are (`refocus_on_release`). An empty
    /// seat is exactly what the rest of this function hands a window, so
    /// without the early return any other caller in that tenth of a second
    /// hands the held key over after all: `sync_panes`, and [`Self::give_back`]
    /// wherever a close in flight is given back -- a refusal's deadline, a
    /// dialog answering it, or `settle_closing`'s retry.
    /// `a_close_in_flight_across_a_lock_never_takes_the_keyboard` drives the
    /// first two.
    ///
    /// **Both arms judge where windows are settling, not where they are
    /// drawn.** That is [`SETTLED`]'s rule, and the tests for each case are
    /// named there. Asking at the present was #127's fourth review, twice
    /// over: `give_back`'s call here found the window it was giving back still
    /// at its restore's progress zero — `present::close`'s opacity zero — and
    /// declined it, and a workspace switch's first frame had the desk being
    /// left on stage and the desk arriving off it.
    ///
    /// **Neither candidate may be a window on its way out.** The pointer arm
    /// gets that from hit-testing the destination, where a closing pane is at
    /// opacity zero from the press onwards; hit-testing the present, as it did
    /// before, only got it once the fade had landed, and a pane still fading
    /// under the pointer was handed the keyboard. The topmost arm had to be
    /// told as well: it reads the pane list directly, and the pane a close is
    /// playing on is usually the topmost one there is, so without the filter
    /// [`Self::hand_off_keyboard`] would take the keyboard off a closing window
    /// and give it straight back.
    ///
    /// **And neither may be a window that is not on screen**, which is the
    /// other half of the same sentence and was missing from the topmost arm
    /// for exactly as long. `window_under` gates on `Frame::covers` and so
    /// asks about pixels twice over — opacity and the rectangle; the topmost
    /// arm asked about neither. A hidden workspace is parked a screen away
    /// rather than unmapped, so closing the only window on the workspace in
    /// view handed the keyboard to a window on a desk the user cannot see, and
    /// with it [`Self::focus_window`]'s `trigger_focus` — which is what a
    /// workspace script acts on. `hand_off_keyboard`'s "nothing is focused when
    /// there is nothing else open" was false in that case, and on a refusal
    /// `give_back`'s call here then *declined*, because something was focused —
    /// so it was permanent. That is #127's own symptom reached by a second
    /// route, and it is [`Self::on_stage`] that closes it: the same visibility
    /// question the pointer path already asks, put to the screens rather than
    /// to a point.
    pub(crate) fn settle_focus(&mut self) {
        if self.lock.is_some() {
            self.settle_lock_focus();
            return;
        }
        if self.refocus_on_release {
            return;
        }
        if self.focused_window().is_some() {
            return;
        }
        let at = self
            .seat
            .get_pointer()
            .map(|pointer| pointer.current_location());
        // Once for the whole walk, and at the destination rather than the
        // frame being drawn: focus is about what the user is going to work
        // with. See `SETTLED`.
        let landed = self.settling();
        let screens = self.screens();
        let next = at
            .and_then(|at| self.window_under_at(at, landed))
            .map(|(window, _)| window)
            .or_else(|| {
                self.panes
                    .iter()
                    .rev()
                    .filter(|pane| !pane.leaving())
                    .filter(|pane| self.on_stage(pane, &screens, landed))
                    .find_map(|pane| pane.client().cloned())
            });
        if let Some(window) = next {
            tracing::debug!("a window went and the keyboard had nowhere to be");
            self.focus_window(&window, SERIAL_COUNTER.next_serial());
        }
    }

    /// Take the keyboard off a window that is no longer on screen, and give it
    /// to whatever is.
    ///
    /// **The half of issue #127's hit-test fix that a mouse cannot reach.** The
    /// review before this one stopped an invisible closing pane from winning
    /// `window_under`, which fixed the keystrokes that follow a *click*: the
    /// press lands in the window that is drawn and focus goes there with it.
    /// It fixed nothing for the user who never touches the mouse. `close_pane`
    /// does not move focus, and `settle_focus` runs only from `sync_panes`,
    /// which calls it when the *pane set* changes -- and a close that has been
    /// asked and not yet answered changes nothing. So `super+q` followed by
    /// carrying on typing put every keystroke into an invisible window for the
    /// whole 190 ms animation plus the 1000 ms grace period, with the window
    /// that reflowed into its place on screen taking the blame. Close, keep
    /// typing is the ordinary way to meet this; clicking first is the rare one.
    ///
    /// **Here, at the request, and not at the press.** The boundary is the same
    /// one `Frame::covers` draws for the pointer, deliberately: while the
    /// leaving animation is playing the window is still *there* -- shrinking
    /// and fading, but drawn, and still alive -- and a half-faded window that
    /// keeps its clicks must keep its keystrokes too, or the two halves of a
    /// press disagree about which window the user is looking at. The animation
    /// landing is the instant the window stops covering anything, and it is the
    /// same instant `settle_closing` sends the request. So the exposure is the
    /// 190 ms in which the window is visible, rather than the 1190 ms in which
    /// it is not.
    ///
    /// **What it is not: a decision about where focus should end up.** That is
    /// [`Self::settle_focus`]'s, unchanged and already the answer everywhere
    /// else a window goes -- the window under the pointer, else the topmost.
    /// This only clears the seat first, because `settle_focus` declines to
    /// argue with a focus that is already set, and the focus that is set is the
    /// one being taken away.
    ///
    /// Nothing is focused when there is nothing else open **on screen** — a
    /// window parked on a hidden workspace is not a candidate, which is
    /// [`Self::settle_focus`]'s own note and was not true until this round.
    /// That is the right answer rather than a gap: typing into a window that is
    /// not on screen is the fault, and typing into nothing at least loses no
    /// keystrokes to the wrong application. [`Self::give_back`] calls
    /// `settle_focus` again, and the window that comes back takes the keyboard
    /// on the frame its restore starts —
    /// `a_refused_close_gives_the_keyboard_back_to_the_window_it_brings_back`
    /// asserts both the empty seat and the return. The sentence that stood here
    /// before it claimed the second half and it did not hold: focus was judged
    /// at the restore's progress zero, where the window is still invisible.
    fn hand_off_keyboard(&mut self, leaving: &Window) {
        if !self.is_focused(leaving) {
            return;
        }
        // The window's own menu first: its grab would refuse the line below.
        // See `release_grabs_of`.
        self.release_grabs_of(leaving);
        self.give_keyboard(None, SERIAL_COUNTER.next_serial());
        crate::xwayland::activate(self, None);
        self.settle_focus();
    }

    /// Bring back a window that was asked to close and did not.
    ///
    /// The window is animated away before the request goes out, because
    /// waiting for the client would mean nothing happening for as long as the
    /// client took. When the client honours it, that is right. When it does
    /// not -- a terminal asking whether you meant it, an editor with unsaved
    /// work -- the window is left drawn away and invisible, still holding its
    /// place in the layout, until something unrelated happens to move it. From
    /// the other side of the screen that is a window that vanished and a
    /// session that lost it.
    ///
    /// There is no refusal in the protocol, so the only evidence is the window
    /// still being here a moment later. It comes back, fading up out of the
    /// frame it was left at rather than snapping -- `present::clear` starts
    /// from what is on screen, which here is the held, shrunk, transparent end
    /// of the leaving animation, so the recovery is that animation run
    /// backwards.
    ///
    /// Returns whether anything is still being waited on.
    pub(crate) fn settle_refused(&mut self, now: std::time::Duration) -> bool {
        /// How long a client has to act on the request before the compositor
        /// decides it is not going to and gives the window back.
        ///
        /// **A deadline on refusal, and it was set as though it were a deadline
        /// on slowness.** At 400ms this fired on clients that were doing
        /// exactly what they had been asked: Electron runs `before-quit`
        /// handlers on the main JS thread, the JVM runs window listeners behind
        /// class loading, Firefox flushes its session store, and all three
        /// routinely take longer than that between receiving
        /// `xdg_toplevel.close` and destroying the toplevel. What the user saw
        /// on an ordinary, successful close was the window fade away, fade back
        /// in, and then vanish with no animation at all when the client finally
        /// went. That is issue #127's third fault, and the fix is not a bigger
        /// number for its own sake -- it is that the number was measuring the
        /// wrong thing.
        ///
        /// **The two costs are not symmetric.** Too short, and every slow-but-
        /// honest close flickers; that is the common case and the user sees it
        /// daily. Too long, and a window that really did refuse stays invisible
        /// for longer -- but it *does* still come back, so the cost is a wait
        /// rather than a loss. The recoverable failure is the one to take.
        ///
        /// **The upper bound is human, not arithmetic** — and it is measured
        /// from the *press*, which is the only clock the user has. This
        /// constant is not that bound; it is the largest part of it. The whole
        /// span from `super+q` to a refused window standing at full opacity
        /// again is
        ///
        /// ```text
        ///   present::CLOSING  190 ms   the leaving animation, before the ask
        /// + GRACE           1000 ms   this constant: waiting for an answer
        /// + the recovery     150 ms   `give_back`'s fade back in
        /// = 1340 ms
        /// ```
        ///
        /// A window returning inside about a second and a half still reads as
        /// the answer to the key that was pressed; past that it reads as the
        /// session doing something by itself, which is the confusion the
        /// original comment here named and was right to name. One second is
        /// the largest value of *this* term that keeps the total inside that,
        /// so it is the value.
        ///
        /// The test that pins this measures from the request rather than from
        /// the press — it has the pane and not the keystroke — so its bound is
        /// this constant plus the recovery, and the two numbers are the same
        /// claim in two frames of reference. See
        /// `a_client_that_takes_six_hundred_milliseconds_to_close_is_never_shown_again`.
        ///
        /// **This is a deadline, and a deadline is the weakest evidence there
        /// is.** It fires on a client that said nothing, because saying nothing
        /// is all the protocol requires of a refusal. Evidence that arrives
        /// *before* it is better than the clock in every case, and
        /// [`Self::refused_with_a_dialog`] is the one piece of it acted on
        /// today: a client that answers a close by putting a new window on
        /// screen has told us what it is doing in as many words.
        ///
        /// What is still *not* done, deliberately: ending the wait early on the
        /// evidence that a client is honouring the request -- `Self::has_content`
        /// goes false as a client tears its surface down, which distinguishes
        /// "closing" from "refusing" far better than any timeout can. The
        /// asymmetry is the whole reason the dialog case could be taken and this
        /// one could not. Reading the dialog wrong ends the grace early and
        /// gives a window *back* that might have been about to go, which the
        /// next frame's `Panes::sync` corrects for free. Reading a teardown
        /// wrong means never bringing the window back at all, and this deadline
        /// is the only thing standing between a refused close and a lost window.
        const GRACE: std::time::Duration = std::time::Duration::from_millis(1000);

        // Over the panes, for the reason `settle_closing` is: a window that
        // has gone is a window that answered, and there is nothing left of it
        // to bring back.
        let due: Vec<crate::pane::PaneId> = self
            .panes
            .iter()
            .filter(|pane| {
                pane.asked_at()
                    .is_some_and(|at| now.saturating_sub(at) >= GRACE)
            })
            .map(Pane::id)
            .collect();
        for id in due {
            tracing::debug!(
                pane = id.get(),
                "a window refused to close; bringing it back"
            );
            self.give_back(id, now);
        }
        self.panes.iter().any(|pane| pane.asked_at().is_some())
    }

    /// Put a window that was asked to close back on screen, and stop waiting on
    /// it.
    ///
    /// The two halves of undoing a close, in the order that cannot strand a
    /// window: **the transform is restored first, and the wait is retired only
    /// if that worked.**
    ///
    /// The other order is issue #127's review finding 5. `present::clear` is
    /// not guaranteed to do anything -- `with_slot` declines rather than panics
    /// when the transform slot is already borrowed, which is the right trade
    /// for a compositor and the reason the call reports now. Retiring
    /// `asked_at` first meant that on such a frame the pane stopped being
    /// `Pane::leaving` while still holding `present::close`'s non-releasing,
    /// opacity-zero transform. Nothing else clears it: `settle_refused` will
    /// never look at the pane again, `close_pane` would decline a second
    /// `super+q` -- no, worse, it would *accept* one and animate an already
    /// invisible window out -- and the only remaining rescue is `move_pane`'s
    /// `present::from`, which needs a layout sweep that never comes in floating
    /// mode with no layout script. The result is a live window holding its
    /// place in the layout that nobody can see or reach.
    ///
    /// **Answering `false` is a retry, and each caller owns a different half of
    /// it.** On `settle_refused`'s path `asked_at` is left set, so the pane is
    /// still due next frame, `settle_refused` returns `true`, and the backend
    /// keeps drawing. On [`Self::refused_with_a_dialog`]'s path `asked_at` is
    /// `None` — the request has not gone out yet — so that mechanism does not
    /// reach it at all, and `Pane::answered` is what carries the retry:
    /// `settle_closing` finds the flag at the `CLOSING` deadline and calls this
    /// again instead of sending the request. Either way a busy slot costs a
    /// frame, which is what it costs everywhere else.
    ///
    /// That second half is #127's third review, finding 3, and it is the third
    /// comment in this area to have claimed coverage from the shape of the code
    /// rather than from anything that asserts it. The paragraph below used to
    /// be true only when `present::clear` happened to succeed;
    /// `a_dialog_whose_give_back_is_declined_does_not_lose_its_parent` is what
    /// makes it true when it does not.
    ///
    /// **Both timers, not just the one each caller happens to be holding.**
    /// `stop_closing` is a no-op on `settle_refused`'s path, where the request
    /// has long gone out and `closing_at` was cleared with it, and it is the
    /// whole of the point on [`Self::refused_with_a_dialog`]'s: a GTK file
    /// chooser is up well inside the 190 ms `CLOSING` window, and without this
    /// `settle_closing` would go on to send the request and close the parent
    /// out from under its own dialog. Undoing a close is one operation, so it
    /// is written once.
    ///
    /// **And the keyboard, because this is the other side of
    /// [`Self::hand_off_keyboard`].** The handoff leaves the seat holding
    /// nothing when the closing window was the only one open; a window coming
    /// back to a session whose keyboard is idle is exactly the case
    /// `settle_focus` exists for. It declines when something else has focus,
    /// so a window the user has moved on from does not steal it back.
    ///
    /// **Called on the frame the restore starts, and that is safe only because
    /// `settle_focus` judges the destination.** Here the pane is still drawn at
    /// `present::close`'s opacity zero; asked about the present, `settle_focus`
    /// declined the very window being given back, whenever too little real
    /// time had passed since `now` for the fade to show. See [`SETTLED`], and
    /// `a_refused_close_gives_the_keyboard_back_to_the_window_it_brings_back`,
    /// which pins the progress-zero case rather than hoping for it.
    fn give_back(&mut self, id: crate::pane::PaneId, now: std::time::Duration) -> bool {
        /// How long the window takes to fade back in.
        const RETURN: std::time::Duration = std::time::Duration::from_millis(150);
        // **A let-go the close kept waiting is taken first, before the return
        // is aimed** (`Pane::let_go`). A layout that let the window out of its
        // tile during the fade -- a switch to floating inside it -- has said it
        // is in no tile now, and the return below lands on `pane_outer`, which
        // a tile caps. Aimed with the tile still held, a window wider than its
        // tile would fade back in to the tile's width rather than its own. And
        // it is put back if the give-back declines, because the window is then
        // still fading and its tile is still what cuts it.
        // `a_mode_switched_during_a_fade_does_not_squash_the_window_leaving`
        // asserts where the return lands, and
        // `a_give_back_that_declines_keeps_a_fade_cut_after_a_mode_switch` the
        // putting back.
        let owed = self.panes.get_mut(id).and_then(Pane::take_let_go);
        let Some(pane) = self.panes.get(id) else {
            // No pane, nothing to give back and nothing left waiting: a window
            // that went while this ran answered the close after all.
            return true;
        };
        let outer = self.pane_outer(pane);
        if !present::clear(pane, outer, now, RETURN, solium_animation::Curve::OutCubic) {
            if let (Some(owed), Some(pane)) = (owed, self.panes.get_mut(id)) {
                pane.owe_let_go(owed);
            }
            return false;
        }
        if let Some(pane) = self.panes.get_mut(id) {
            pane.forget_asked();
            pane.stop_closing();
            // And the debt, with the timers it was standing in for. Nothing is
            // owed once the transform has been restored, and leaving it set
            // would have `settle_closing` retry a give-back that already
            // happened on the next close this pane is ever given.
            pane.settled_answer();
        }
        self.redraw = true;
        // **The layout hears the refusal here, which is once per refusal.**
        // After the transform took and the timers were retired, and at no other
        // point: all three routes a refused window comes back by -- the grace
        // deadline in `settle_refused`, a dialog's answer in
        // `refused_with_a_dialog`, and `settle_closing`'s retry of an answer
        // whose first give-back was declined -- end in this function, and a
        // declined attempt returns above without telling anyone. See
        // `a_refusal_tells_the_layout_once_on_the_frame_the_window_comes_back`.
        //
        // The pane is not `leaving` any more, so a layout putting it back moves
        // it like any other window -- and `move_pane` animates from what is on
        // screen, which here is the held, shrunk, transparent end of the close:
        // the window fades in as it slides to wherever the layout puts it, over
        // the layout's duration rather than `RETURN`. Placed by nothing, it
        // plays the fade above. See
        // `a_refused_window_fades_back_in_from_where_it_vanished`, and
        // `a_refusal_by_dialog_fades_the_window_back_and_moves_its_neighbour_once`
        // for the dialog route, which sweeps several times in one dispatch.
        self.trigger_refused(id);
        self.settle_focus();
        true
    }

    /// A window that was asked to close has answered by opening another one.
    ///
    /// **The case that inverts `GRACE`'s argument**, and issue #127's review
    /// finding 3. `settle_refused` reasons that too long a grace period only
    /// makes a genuinely refused window wait, which is true for the client the
    /// grace period was lengthened for -- the honest-but-slow one -- and
    /// backwards for the client that refuses on purpose. "Save your changes
    /// before closing?" is a refusal delivered as a question, and under a flat
    /// deadline it left the parent window a hole for the whole second: the
    /// dialog floating over the space where its document used to be, with
    /// nothing to read and nothing to decide against. A second `super+q` could
    /// not clear it either, because the widened `Pane::leaving` guard correctly
    /// declines to start a second close on a pane that is still in one.
    ///
    /// **A new window from a client we just asked to close is evidence, and it
    /// is the safe kind.** The direction of the risk is what makes this
    /// actionable where reading a teardown is not: acting on it *returns* a
    /// window, so being wrong costs a window coming back that was going to
    /// leave anyway -- which the next `Panes::sync` undoes by itself when the
    /// client does finish closing. See `GRACE`'s note.
    ///
    /// Parentage rather than the client connection, deliberately. A file
    /// chooser is `set_parent`'d to the document that raised it, which says
    /// *this* window is what the dialog is about; a client that happens to open
    /// an unrelated window elsewhere in the same process while a close is
    /// pending has said nothing about the window being closed.
    ///
    /// **Three call sites, named because a claim about coverage is worth
    /// exactly what can be checked against it.** [`Self::parent_of`] reading
    /// both protocols' spellings of a parent is necessary and is not
    /// sufficient: what decides whether an X11 dialog is covered is whether any
    /// X11 *event* reaches this function, and the first draft of this comment
    /// asserted that it did on the strength of `parent_of` alone, while the
    /// only caller was `XdgShellHandler::parent_changed`. An XWayland
    /// application's unsaved-changes dialog left its parent a 1.19 s hole for
    /// the whole of #127's review. The callers are:
    ///
    /// * `XdgShellHandler::parent_changed` — `xdg_toplevel.set_parent`.
    /// * `XwmHandler::map_window_request` — an X11 window appearing with
    ///   `WM_TRANSIENT_FOR` already set, which is where nearly every one of
    ///   them arrives: smithay reads the property at `CreateNotify`, strictly
    ///   before the `MapRequest`.
    /// * `XwmHandler::property_notify` for `WmWindowProperty::TransientFor` —
    ///   the client that maps first and says whose dialog it is afterwards.
    ///
    /// **They are three equivalent call sites because the gate below is in this
    /// body, and the round that added the third listed them as equivalent while
    /// they were not.** The rule that a window which places itself is not an
    /// answer to anything stood as a comment at `map_window_request`'s call,
    /// and `property_notify` did not repeat it — so an X11 menu, tooltip,
    /// notification, splash or override-redirect window that set
    /// `WM_TRANSIENT_FOR` after mapping cancelled its parent's close. `super+q`
    /// faded the window out, brought it back, and never closed it. A rule kept
    /// at one caller is a rule broken at the next one added, which is the same
    /// argument [`Self::map_stacked`] makes about restacking.
    ///
    /// **And the honest limit, which is the part the comment before this one
    /// left out.** Only the first of the three is pinned by a test *through its
    /// own protocol*: an `X11Surface` cannot be built without a live XWayland
    /// and nothing in this suite has one. So the two X11 hooks are verified by
    /// reading them, and what they share with the tested path is the body below
    /// — which is every line of the decision, the `managed` gate included.
    /// `a_window_that_places_itself_does_not_cancel_a_close` pins that gate at
    /// the level the X11 hooks rely on: a child holding the *unmanaged* pane
    /// `take_unmanaged_pane` gives every menu, tooltip and override-redirect
    /// window, driven through the one caller this suite can drive. That is a
    /// weaker claim than "covered" and it is the one that is true.
    pub(crate) fn refused_with_a_dialog(&mut self, child: &Window) {
        // **A window that places itself is not an answer to anything**, and the
        // question is asked of the child before its parent is even looked up.
        //
        // `Pane::managed` is false for exactly the windows that mean nothing
        // here and is set in one place, `take_unmanaged_pane` — which is what
        // both of XWayland's self-placing branches call, and the drag icon's,
        // so a menu, a tooltip, a splash, a notification and an
        // override-redirect window are all caught by one question. A child with
        // no pane at all is not on screen and cannot be an answer either.
        if !self.panes.of(child).is_some_and(Pane::managed) {
            return;
        }
        let Parentage::Window(parent) = self.parent_of(child) else {
            return;
        };
        let Some(id) = self
            .panes
            .iter()
            .map(Pane::id)
            .find(|id| id.get() == parent)
        else {
            return;
        };
        if !self.panes.get(id).is_some_and(|pane| pane.leaving()) {
            return;
        }
        let now = self.clock.now();
        tracing::debug!(
            pane = id.get(),
            "a window answered a close with a dialog; bringing it back"
        );
        // **The answer is recorded before it is acted on, because acting on it
        // can fail.** That is #127's third review, finding 3: `give_back`
        // returns whether `present::clear` took, and this caller dropped the
        // answer. `settle_refused` cannot pick a declined give-back up here —
        // `asked_at` is still `None` inside `CLOSING` by definition, so the pane
        // is on no deadline that function reads — and `settle_closing` went on
        // to send the request and close the parent out from under its own
        // dialog. `Pane::answered` outlives the failed attempt, and
        // `settle_closing` retries from it. `give_back` clears it when it takes.
        if let Some(pane) = self.panes.get_mut(id) {
            pane.mark_answered();
        }
        // **The transform first, and nothing is retired unless it took.** Same
        // rule as `give_back`'s own, for the same reason: a frame that cleared
        // `closing_at` without restoring the presentation would leave a pane
        // that is not `leaving()`, not due at any deadline, and still holding
        // an opacity-zero transform -- finding 5 reintroduced by the fix for
        // finding 3. Declining now leaves the close where it was *and* the debt
        // recorded, rather than leaving the close to run to its end.
        //
        // `give_back` retires both timers, which is what matters for the dialog
        // that beat the 190 ms `CLOSING` deadline: without `stop_closing` the
        // request would still go out afterwards and close the parent out from
        // under its own dialog. See it for why that lives there and not here.
        self.give_back(id, now);
    }

    /// Act on a frame button.
    pub(crate) fn frame_action(&mut self, pane: crate::pane::PaneId, action: Action) {
        match action {
            Action::Close => self.close_pane(pane),
            Action::ToggleMaximize => {
                if let Some(window) = self.panes.get(pane).and_then(Pane::client).cloned() {
                    self.toggle_maximize(&window);
                }
            }
        }
    }

    /// Fill the work area, or go back to where the window was.
    fn toggle_maximize(&mut self, window: &Window) {
        let Some(id) = self.panes.id_of(window) else {
            return;
        };
        let Some(toplevel) = window.toplevel().cloned() else {
            return;
        };
        let Some(current) = self.real_geometry(window) else {
            return;
        };
        let Some(filled) = self.maximised(window, current) else {
            return;
        };

        // On the pane and not on its frame, which is where it was until #92.
        // For this toggle a window with no frame to keep it on was latent, not
        // seen: its one caller is `frame_action`, reached only from a button
        // on a `Styled` frame, so a window with no frame had no button to
        // press either. The frameless case was reachable only through
        // fullscreen, where a client drawing its own frame had no rect kept
        // at all.
        let restore = self.panes.get_mut(id).and_then(Pane::take_restore);

        let (location, size, maximized) = match restore {
            // Restoring: back to exactly where it was, because that rect was
            // stored rather than recomputed -- unless the monitor it was on
            // has gone since, see `back_on_a_screen`.
            Some(previous) => {
                let back = self.back_on_a_screen(window, previous);
                (back.loc, back.size, false)
            }
            None => (filled.loc, filled.size, true),
        };

        // **And out of its tile, or back into it (#133).** A tiled client is
        // held inside `Pane::placed`, and a maximised one is not tiled: left
        // there, the work area it is about to be configured to would be cut
        // down to the tile it is leaving. The tile is kept for the way back,
        // so a window restored into it is tiled again at once rather than at
        // the next sweep -- which is what a tiled edge drag started from it
        // reads (#124).
        if let Some(pane) = self.panes.get_mut(id) {
            if maximized {
                pane.set_restore(Some(current));
                pane.leave_tile();
            } else {
                pane.return_to_tile();
            }
        }

        toplevel.with_pending_state(|state| {
            state.size = Some(size);
            if maximized {
                state.states.set(xdg_toplevel::State::Maximized);
            } else {
                state.states.unset(xdg_toplevel::State::Maximized);
            }
        });
        toplevel.send_pending_configure();
        self.map_stacked(window.clone(), location, true);
        tracing::debug!(maximized, "window maximise toggled");
    }

    /// Where this window goes when it is maximised: the work area of the
    /// monitor `on` is on, less the window's frame.
    ///
    /// The monitor the window is on, not the one the pointer is on: a window
    /// maximised while you point at the other screen must fill its own, and
    /// jumping across is the last thing a maximise should do.
    ///
    /// The frame's height comes out of the client's share, which is the same
    /// arithmetic as placement: a maximised window and its frame together fill
    /// the work area exactly. Asked by [`Self::toggle_maximize`], and by
    /// `unfullscreen_request` for a window that was maximised when it went
    /// fullscreen and so goes back to being maximised.
    fn maximised(
        &self,
        window: &Window,
        on: Rectangle<i32, Logical>,
    ) -> Option<Rectangle<i32, Logical>> {
        Some(inner(self.work_area_of(on)?, self.frame_insets(window)))
    }

    /// A kept rect this window is being put back at, moved onto a screen if
    /// no part of it, frame included, is on one.
    ///
    /// **The rect was stored, and the monitor it was stored on may have gone
    /// since** -- unplugged, or disconnected when it slept. `rescue_offscreen`
    /// brings the window itself onto a remaining screen when that happens, but
    /// not the rect it goes back to, and it runs only when the monitors
    /// change: a window put back at the rect as it was sat on no screen until
    /// the next hotplug. Moved by the same rule as that rescue, so the two
    /// agree about where a stranded window goes.
    fn back_on_a_screen(
        &self,
        window: &Window,
        back: Rectangle<i32, Logical>,
    ) -> Rectangle<i32, Logical> {
        let insets = self.frame_insets(window);
        self.rescued(grown(back, insets))
            .map_or(back, |outer| inner(outer, insets))
    }

    /// How far the client sits below its window's top edge.
    pub(crate) fn frame_insets(&self, window: &Window) -> Insets {
        if !self.is_decorated(window) {
            return Insets::NONE;
        }
        // What the decoration asked for, since the decoration is what draws
        // it. A window whose frame has not been built yet falls back to the
        // default bar height so its first layout is not visibly wrong.
        self.panes
            .id_of(window)
            .map_or(Insets::NONE, |id| self.insets_of(id))
    }

    /// The same, for a caller that already knows which pane it means.
    /// How much room this pane's frame takes. See [`insets_for`] for what each
    /// answer means and why the fallback is a titlebar rather than nothing.
    ///
    /// **Asked of the pane, and there is nothing else to ask.** It is the
    /// reader the two tables actually hurt — `frames` was checked first, so a
    /// pane in both had `bare` silently ignored — and a frame is one value on
    /// one pane now.
    ///
    /// An id with no pane reserves nothing. It used to answer from the tables,
    /// which outlived their panes until the next sweep, so a retired id could
    /// still be told a titlebar's worth; there is no longer anywhere for that
    /// answer to come from. No caller can reach it today — every one holds a
    /// live pane — which is why it is a fallback and not a `debug_assert`.
    pub(crate) fn insets_of(&self, id: crate::pane::PaneId) -> Insets {
        self.panes
            .get(id)
            .map_or(Insets::NONE, |pane| insets_for(pane.frame()))
    }

    /// Raise a window and give it the keyboard.
    /// Report what the compositor is holding, once a second, when asked.
    ///
    /// Enabled with `SOLIUM_MEMDIAG=1`. A leak hunt needs to know *which*
    /// number is growing: resident memory alone cannot tell a forgotten
    /// decoration from a Lua heap that never shrinks from an allocator that
    /// simply keeps what it has.
    ///
    /// **`decorations` is no longer among them, and not because it was
    /// redundant.** It counted `Decorations::frames`, and its whole value was
    /// that an entry there could belong to no pane — which is the leak it was
    /// added to show. A frame is part of its pane now, so such an entry cannot
    /// exist and the number cannot be computed; counting framed panes instead
    /// would put a plausible figure on the line that is blind to exactly the
    /// thing it was watching for. `windows` is what answers now: a decoration
    /// that is still held is a pane that is still held.
    pub(crate) fn memory_report(&mut self) {
        if !crate::dev::memory_diagnostics() {
            return;
        }
        let now = self.clock.now();
        if now.saturating_sub(self.reported_at) < std::time::Duration::from_secs(1) {
            return;
        }
        self.reported_at = now;

        // statm's second field is resident pages.
        let rss_kb = std::fs::read_to_string("/proc/self/statm")
            .ok()
            .and_then(|statm| {
                statm
                    .split_whitespace()
                    .nth(1)
                    .and_then(|pages| pages.parse::<usize>().ok())
            })
            .map_or(0, |pages| pages * 4);

        let pointer_at = self
            .seat
            .get_pointer()
            .map(|pointer| pointer.current_location());
        let focused_inside = self
            .focused_window()
            .map(|window| self.pointer_inside(&window));
        tracing::info!(
            pointer = format!("{pointer_at:?}"),
            focused_inside = format!("{focused_inside:?}"),
            windows = self.panes.len(),
            lua_kb = self
                .scripts
                .as_ref()
                .map_or(0, |scripts| scripts.used_memory() / 1024),
            rss_kb,
            "MEMDIAG"
        );
    }

    /// The decorated window under `location`, frame or client, and where the
    /// pointer lands in its frame's own space.
    ///
    /// Wider than [`Self::chrome_under`] on purpose: a decoration that glows
    /// where the cursor is has to be told about the cursor while it is over
    /// the client, which is the client's surface and reports nothing to us.
    /// Ownership of clicks -- and, since #108, the pointer's shape -- is still
    /// decided by `chrome_under`; this is only for looking.
    pub(crate) fn decorated_under(
        &self,
        location: Point<f64, Logical>,
    ) -> Option<(crate::pane::PaneId, Point<f64, Logical>)> {
        let now = self.clock.now();
        let screens = self.screens();
        self.panes.iter().rev().find_map(|pane| {
            // Only a built frame is listening. There is no scene to tell about
            // the pointer until there is one.
            pane.decoration()?;
            let outer = self.pane_outer(pane);
            // Nor one on a screen that does not draw it, nor the frame of a
            // window whose client has gone: [`shown_at`], whose test is
            // `a_point_is_on_a_window_only_on_a_monitor_that_draws_it`. A pane
            // with a built frame needs Qt, which this binary's tests cannot
            // start, so this walk is not driven by one.
            if !shown_at(outer, pane.ghost(), location, &screens) {
                return None;
            }
            let drawn = self.drawn_at(pane, outer, now);
            // An invisible frame has nothing to glow. This walk only forwards
            // the pointer to a decoration's scene, so the cost of getting it
            // wrong is a hover state on a window that is not there rather than
            // a lost click -- but it is the same question as the other three
            // walks and it gets the same answer. See `present::Frame::covers`.
            if !drawn.covers(location) {
                return None;
            }
            let in_outer = present::to_window_space(drawn, outer, location) - outer.loc.to_f64();
            Some((pane.id(), in_outer))
        })
    }

    /// Put a stand-in on screen for an application that was just asked for.
    ///
    /// At the pointer, because that is where the asking happened and where the
    /// eye already is. It is not where the window will end up -- the layout
    /// decides that when the window exists -- but the window grows out of this
    /// rect when it arrives, so the movement is continuous either way.
    /// Open a window for an application that has been asked for.
    ///
    /// The window's life begins here rather than when the client connects: it
    /// takes a slot, the other windows move aside for it, and it can be closed
    /// while it waits. What arrives later maps *into* it.
    ///
    /// Its first slot is under the pointer, because that is where the asking
    /// happened and where the eye already is. The layout is told immediately
    /// and usually moves it somewhere better in the same breath — which is the
    /// point: the arrangement settles before the application has done
    /// anything at all.
    pub(crate) fn begin_loading(&mut self, program: &str, pid: Option<u32>) -> crate::pane::PaneId {
        let name = std::path::Path::new(program)
            .file_name()
            .map_or(program, |name| name.to_str().unwrap_or(program));
        // Resolved now, so reloading the configuration mid-wait does not
        // change what a window already on screen looks like halfway through.
        let source = crate::pane::loading_source(self.loading.scene.as_deref());
        // Built here rather than at the first draw: the scene is what the
        // window *is* until its application arrives, and a window that is
        // empty for its first frame is a window that flickers.
        let properties = format!(
            "{{\"program\":\"{}\",\"waited\":0}}",
            name.replace('"', "'")
        );
        let scene = match crate::surface::ShellSurface::new(source.clone(), &properties) {
            Ok(scene) => Some(scene),
            Err(err) => {
                // The window still opens. It takes its slot, it can be closed,
                // and its application will still arrive in it -- it just has
                // nothing to show meanwhile, which beats not opening.
                tracing::warn!(
                    ?err,
                    program = name,
                    "no scene for a window that is loading"
                );
                None
            }
        };
        self.open_loading(name, pid, source, scene)
    }

    /// The rest of [`Self::begin_loading`]: the window itself, once its scene
    /// has been built, or has failed to be and is `None`.
    ///
    /// Apart so that a test can open a window for an application without
    /// starting Qt, which a test process holding a raw libwayland connection
    /// does not survive -- see
    /// `changed_output_resends_fractional_scale_and_unchanged_output_does_not`.
    /// Everything a layout hears and does about the window is on this side of
    /// the cut, and `None` is the path a failed scene already takes.
    fn open_loading(
        &mut self,
        name: &str,
        pid: Option<u32>,
        source: std::path::PathBuf,
        scene: Option<crate::surface::ShellSurface>,
    ) -> crate::pane::PaneId {
        // A window's worth of screen from the very first frame, before anyone
        // is asked where it should go. A layout usually moves it in the same
        // breath, but this is what it falls back to -- and the fallback has to
        // be the shape of the window that is coming, because for a floating
        // arrangement this *is* where the window ends up. Getting this wrong
        // is not subtle: the application arrives sized to whatever is here.
        let area = self.launch_slot();
        let id = self.panes.open(Pane::loading(
            name,
            pid,
            area,
            source,
            scene,
            self.clock.now(),
        ));

        // Built whether or not it will be drawn yet. The room it takes is
        // reserved from the first frame, so the window is the same shape
        // before and after its application arrives -- and it is the *same*
        // frame, keyed by pane, so whatever animation is running in it carries
        // straight through the handover instead of starting again.
        self.decorations
            .insert(&mut self.panes, id, area.size.w, area.size.h);
        // And the frame's share comes off the slot, exactly as it does for a
        // window the layout placed, so the client is sized to the same rect
        // either way.
        let client = inner(area, self.insets_of(id));
        if let Some(pane) = self.panes.get_mut(id) {
            pane.set_slot(client);
        }
        tracing::debug!(program = name, ?pid, "a window opened for an application");

        // Told as an *open*, not as a relayout. A layout keeps its own
        // arrangement and adds to it when it hears a window opened; a relayout
        // only re-runs what it already holds, so the new window would never
        // join. This is the whole of "the other windows move aside": the
        // window opened, and it opened before its application existed.
        //
        // Unless it was asked not to. `reserves_a_slot` has to gate the event
        // and not only the snapshot: a layout that has been told a window
        // opened keeps it in its own arrangement, and would go on placing it
        // however the snapshot were filtered afterwards.
        if self.loading.reserves_a_slot {
            self.trigger_open(id);
        }
        self.redraw = true;
        id
    }

    /// Give up on applications that never arrived.
    ///
    /// A window that waits forever holds a slot forever. It goes exactly as if
    /// it had been closed, and the layout is told — so the arrangement heals
    /// rather than keeping a gap for something that is not coming.
    ///
    /// Returns whether anything went, so the backend redraws.
    pub(crate) fn settle_loading(&mut self, now: std::time::Duration) -> bool {
        let patience = self.loading.patience;
        let gone: Vec<(crate::pane::PaneId, String)> = self
            .panes
            .iter()
            // `is_loading` as well, because `expired` answers for a pane whose
            // client has gone too, and that one is `settle_leaving`'s to drop.
            // (`depart` would do nothing with it -- it has gone already -- but
            // the log line below would call it an application that never came.)
            .filter(|pane| pane.is_loading() && pane.expired(now, patience))
            .map(|pane| (pane.id(), pane.program().unwrap_or_default().to_owned()))
            .collect();
        if gone.is_empty() {
            return false;
        }
        for (id, program) in gone {
            tracing::info!(program, "gave up on an application that never arrived");
            // The way every window goes: the layout is told while the pane is
            // still here, and it fades out of its place as the layout closes
            // up -- the scene that stood in for the application fading as a
            // closed window does. This used to remove the pane first so that
            // "a layout should not be laying out around a window that is
            // already gone", which `close`'s own snapshot now answers for every
            // route alike: the window is listed there as leaving, and in no
            // event after it. See `Self::depart`.
            self.depart(id);
        }
        self.redraw = true;
        true
    }

    /// Where a window goes when nothing else has an opinion about it.
    ///
    /// A window's worth of screen, inset from the work area. Used for a window
    /// opened for an application when no layout placed it — floating, or no
    /// scripts at all — so that what is on screen while the application starts
    /// is the shape and size of the window that is coming.
    fn launch_slot(&self) -> Rectangle<i32, Logical> {
        let area = self
            .work_area()
            .unwrap_or_else(|| Rectangle::new((0, 0).into(), (1280, 800).into()));
        let inset = 48;
        Rectangle::new(
            (area.loc.x + inset, area.loc.y + inset).into(),
            (
                (area.size.w - inset * 2).max(200),
                (area.size.h - inset * 2).max(150),
            )
                .into(),
        )
    }

    /// Whether this window still has anything of its own to show.
    ///
    /// A client that is closing tears its surface down before the compositor
    /// hears the toplevel is gone, so for a few frames the window is still in
    /// the space with nothing in it -- and the frame, which is *ours* and
    /// perfectly valid, goes on being drawn around an empty rectangle. A
    /// titlebar hanging in the air after the window has gone reads as a bug in
    /// closing, which is the moment the user is least willing to forgive one.
    pub(crate) fn has_content(&self, window: &Window) -> bool {
        let Some(surface) = window.wl_surface() else {
            return false;
        };
        smithay::backend::renderer::utils::with_renderer_surface_state(&surface, |state| {
            state.buffer().is_some()
        })
        .unwrap_or(false)
    }

    /// Whether the pointer is anywhere over this window, frame included.
    ///
    /// A decoration that lights up as the pointer approaches needs this even
    /// while the pointer is over the client area, which is the client's
    /// surface and sends us nothing.
    pub(crate) fn pointer_inside(&self, window: &Window) -> bool {
        let Some(outer) = self.outer_geometry(window) else {
            return false;
        };
        let Some(pointer) = self.seat.get_pointer() else {
            return false;
        };
        outer.to_f64().contains(pointer.current_location())
    }

    /// Point the keyboard at the lock screen.
    ///
    /// The surface on the monitor the pointer is on, so that on a two-monitor
    /// desk the password goes into the field the user is looking at; any
    /// surface otherwise, because typing into the wrong screen's lock dialog
    /// still beats typing into nothing.
    ///
    /// The lock client is given the selection like any other focused client.
    /// Withholding it would look like caution and buy none: any client that
    /// can take focus can already read the clipboard, so the only thing the
    /// restriction would achieve is breaking paste from a password manager.
    ///
    /// Only surfaces that are still alive are candidates (see `Lock::surfaces`).
    /// With none left the keyboard is left where it is; `settle_lock_focus`
    /// is the caller that then takes it off a dead one.
    pub(crate) fn focus_lock(&mut self) {
        let Some(lock) = self.lock.as_ref() else {
            return;
        };
        let here = self
            .active_output()
            .and_then(|output| lock.surface_for(&output))
            .map(|surface| surface.wl_surface().clone());
        let Some(surface) = here.or_else(|| {
            lock.surfaces()
                .next()
                .map(|surface| surface.wl_surface().clone())
        }) else {
            return;
        };
        // Through the gate like everything else, and the one call it must
        // always let through: this surface is the lock's own.
        self.give_keyboard(Some(surface), SERIAL_COUNTER.next_serial());
    }

    /// `settle_focus`, while locked.
    ///
    /// Left alone if the keyboard is on a live lock surface, because the user
    /// may be typing into the one on the other monitor and a window opening
    /// behind the lock is no reason to move them. Otherwise to the lock screen
    /// (`focus_lock`), and to nothing if there is no lock surface left at all
    /// -- a lock client that has died leaves the keyboard on a surface that no
    /// longer exists, and "nothing" is the honest name for that.
    fn settle_lock_focus(&mut self) {
        let Some(lock) = self.lock.as_ref() else {
            return;
        };
        let focus = self
            .seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus());
        if focus
            .as_ref()
            .is_some_and(|focus| lock.surfaces().any(|each| each.wl_surface() == focus))
        {
            return;
        }
        if lock.surfaces().next().is_some() {
            self.focus_lock();
        } else if focus.is_some() {
            self.give_keyboard(None, SERIAL_COUNTER.next_serial());
        }
    }

    /// Whether anything is holding the machine awake.
    ///
    /// An inhibitor applies while its surface is visible, and the protocol
    /// leaves "visible" to us. Two answers here are worth stating outright:
    ///
    /// * A window whose slot is off every monitor does not count. That is what
    ///   a workspace switch does to the windows it hides, so a video paused on
    ///   another workspace stops holding the screen on -- which is what anyone
    ///   would expect and what the protocol means.
    /// * **Nothing counts while the session is locked.** Otherwise a player
    ///   left running behind a lock screen keeps the machine awake all night
    ///   displaying a lock screen, which is the exact opposite of what both
    ///   features are for.
    pub(crate) fn idle_inhibited(&self) -> bool {
        if self.lock.is_some() {
            return false;
        }
        // Collected first: `inhibiting` borrows `self.idle` and the visibility
        // test borrows the rest of `self`.
        self.idle
            .inhibiting()
            .any(|surface| self.surface_is_visible(surface))
    }

    /// Whether a surface belongs to a window that is drawn somewhere.
    ///
    /// Against where the window is **drawn**, not where it lives. Those are
    /// different rectangles and the difference is the whole question: a
    /// workspace switch does not move a window's slot, it slides the window
    /// away from it with a presentation transform, so a window on a workspace
    /// nobody is looking at still has its slot squarely on a monitor. Asking
    /// the slot said such a window was visible, and a video paused on another
    /// workspace went on holding the machine awake. Found by hiding one and
    /// waiting.
    ///
    /// **And drawn on a screen that draws it**, through `nothing_on_stage`:
    /// with two monitors side by side the left one's hidden workspace is
    /// carried onto the right one, which never draws it, and the video there
    /// went on holding the machine awake from a screen it was not on (#134's
    /// third review,
    /// `on_two_monitors_a_window_on_the_left_monitors_hidden_workspace_is_not_on_stage`).
    ///
    /// Being the drawn rect also settles the cases that have not arrived yet
    /// in the same breath: a thumbnail in overview is visible, and a window
    /// minimised to nothing (#30) will not be.
    ///
    /// Windows only. A layer surface could hold an inhibitor too, and if one
    /// ever does this is where it would be answered -- but a bar is not what
    /// asks to keep the machine awake, and guessing at the semantics for a
    /// case with no client behind it is how a wrong answer gets written down.
    fn surface_is_visible(&self, surface: &WlSurface) -> bool {
        let now = self.clock.now();
        self.panes.iter().any(|pane| {
            pane.client().is_some_and(|window| {
                window
                    .wl_surface()
                    .is_some_and(|owned| owned.as_ref() == surface)
                    && {
                        let slot = self.pane_outer(pane);
                        let drawn = self.drawn_at(pane, slot, now).rect;
                        nothing_on_stage([(slot, drawn)], &self.screens()) == Some(false)
                    }
            })
        })
    }

    pub(crate) fn focus_window(&mut self, window: &Window, serial: Serial) {
        let Some(location) = self.space.element_location(window) else {
            return;
        };
        // The rule in `focus.rs`, asked before anything happens rather than
        // left to `give_keyboard` to refuse halfway through. A focus that is
        // refused must not restack a window behind the lock, activate an X11
        // window, or tell a layout that something is focused when nothing is.
        // Scripts (`sol.focus`), layouts' event handlers, xdg-activation and
        // `settle_focus` all arrive here, and all of them can while locked.
        if !self.may_focus(window) {
            tracing::debug!("refused to focus a window: the session is locked");
            return;
        }
        // Frames are drawn differently focused and unfocused, and restacking
        // changes what covers what. Both are the screen changing.
        self.redraw = true;
        // `true` restacks: a clicked window comes to the front.
        self.map_stacked(window.clone(), location, true);

        if self.seat.get_keyboard().is_some() {
            // The window's own surface, so this works for an X11 window as
            // well as an xdg one.
            let surface = window.wl_surface().map(|surface| surface.into_owned());
            self.give_keyboard(surface.clone(), serial);
            // X11 wants telling separately, in its own terms: a window that
            // has keyboard focus but was never activated draws itself
            // unfocused however much typing goes into it.
            crate::xwayland::activate(self, surface.as_ref());
        }

        // A layout may want to follow: a scroller brings the focused column
        // fully into view, which is the difference between clicking a window
        // half off the edge and being able to use it.
        if !self.focusing {
            self.focusing = true;
            self.trigger_focus(window);
            self.focusing = false;
        }
    }

    /// Place and animate a window the first time it has something to show.
    ///
    /// Both belong to this moment rather than to the map request: until the
    /// client has committed a buffer it has no size, and placing or animating
    /// a zero-sized window is placing nothing.
    fn show_if_new(&mut self, window: &Window) {
        // A client's first commit is typically empty — it commits to receive
        // the initial configure, then draws. Claiming the first-show moment on
        // that commit places and animates a zero-sized window, which lands it
        // at half the output away from where it belongs.
        let size = window.geometry().size;
        if size.w <= 0 || size.h <= 0 {
            return;
        }

        let Some(pane) = self.panes.id_of(window) else {
            return;
        };
        if !self.panes.get(pane).is_some_and(present::mark_shown) {
            return;
        }

        // An unmanaged pane is already where it belongs, and everything below
        // this line sizes, places or animates -- all three wrong for it.
        //
        // Override-redirect is X11 for "do not manage me": a menu, a tooltip,
        // a drag icon. `mapped_override_redirect_window` has already mapped it
        // at the position its client chose, and `take_unmanaged_pane` marked
        // the pane so. Marking it shown above is still right -- it is on
        // screen -- but it is the last thing this function may do to it.
        //
        // Issue #100, from the reporter's log opening a Steam context menu:
        //
        //     WARN could not size an X11 window err=UnsupportedForOverrideRedirect
        //
        // That warning is the harmless half. `size_window` asks smithay to
        // configure an override-redirect surface and is refused, which costs a
        // line in the log and nothing else. The damage is the next statement:
        // `initial_placement` picks a location and `map_element` *succeeds* at
        // moving the menu there, so it opens away from the pointer and the
        // layout treats it as a window.
        //
        // Hence the guard here and not inside `size_window`: the failing call
        // is not the one doing the harm, and a guard there would have silenced
        // the warning while leaving the menu misplaced.
        //
        // This is the second leak of its kind -- see the comment in
        // `xwayland.rs`'s `mapped_override_redirect_window`, where unmanaged
        // windows reached the list a layout reads and dragging a text
        // selection reflowed the desktop. Both were one path forgetting to
        // ask; if a third appears, the question belongs inside whatever those
        // paths call rather than at a fourth call site.
        //
        // Asked here, ahead of both branches below -- one places from a
        // remembered slot, the other from a fresh fit -- rather than inside
        // whichever branch a bug happened to surface in.
        if !self.panes.get(pane).is_some_and(Pane::managed) {
            return;
        }

        // A client that never negotiates still gets a frame.
        //
        // `Frame::Pending` reserves `TITLEBAR_HEIGHT` -- see `insets_for` --
        // on the understanding that a frame is on its way. Until this, the
        // only things that ever built one were `decorate`, driven by
        // `xdg_decoration`, and two special cases (a launch placeholder and
        // leaving fullscreen). So a client that never binds that protocol
        // reserved a titlebar for the life of its window and had none drawn.
        //
        // That is not a rare case. Firefox does not bind it, and no XWayland
        // client can -- Steam included. Issue #103, measured nested: exactly
        // 32 rows of unpainted space above Firefox's first painted row, beside
        // a real titlebar on a terminal in the same session.
        //
        // Server-side is already what `new_decoration` offers unasked, on the
        // grounds that the frame is part of the desktop's look. This extends
        // that to the clients that never ask: having no opinion gets the same
        // answer as not having expressed one yet. A client that later asks for
        // client-side is still obeyed -- `decorate` takes it to `Frame::None`
        // and removes this.
        //
        // `Pending` is the whole condition and it is exact: a negotiated
        // server-side frame is already `Styled`, a negotiated client-side one
        // is already `None`, and `insert` itself returns early on `Styled`.
        // What is left is only "nobody has decided", which is this.
        //
        // Ahead of the sizing below, because a frame changes the insets that
        // `fitted_size` and `initial_placement` both read.
        if self
            .panes
            .get(pane)
            .is_some_and(|pane| matches!(pane.frame(), crate::pane::Frame::Pending))
        {
            let real = self.real_geometry(window);
            let width = real.map_or(TITLEBAR_HEIGHT * 20, |real| real.size.w);
            let height = real.map_or(TITLEBAR_HEIGHT * 15, |real| real.size.h);
            self.decorations
                .insert(&mut self.panes, pane, width, height);
        }

        // A client that mapped into a window which was already open takes
        // that window's shape. The layout placed it before the application
        // existed and was told it opened then; doing either again would move a
        // window that is already where it belongs and announce it twice.
        if self.panes.get(pane).is_some_and(Pane::adopted) {
            let Some(slot) = self.panes.get(pane).map(Pane::slot) else {
                return;
            };
            // Sized again: adoption already asked for this, and a client that
            // negotiated its decorations in between has a different amount of
            // room than it was first told.
            size_window(window, slot);
            self.map_stacked(window.clone(), slot.loc, false);
            // Its place was decided at the launch, so the keyboard is decided
            // now: nothing was asked of a script on the way here.
            self.offer_keyboard(window, pane);
            return;
        }

        // Sized to fit before it is placed, because where a window goes
        // depends on how big it is.
        let size = self.fitted_size(window);
        let location = self.initial_placement(window, size);
        if size != window.geometry().size {
            size_window(window, Rectangle::new(location, size));
        }
        self.map_stacked(window.clone(), location, true);

        // How a window appears is a script's decision — that is what makes the
        // dock-icon genie a script rather than a feature. The built-in is only
        // a fallback for when nothing has an opinion; a window popping into
        // existence with no animation at all is worse than a plain one.
        let opened = self.trigger_open(pane);
        if !opened.handled
            && let Some(outer) = self.outer_geometry(window)
            && let Some(pane) = self.panes.get(pane)
        {
            present::open(pane, outer, self.clock.now());
        }
        // After `open`, which is where the layout says where the window goes,
        // and only if no script said where the keyboard goes.
        if !opened.focused {
            self.offer_keyboard(window, pane);
        }
    }

    /// Give a window the keyboard as it is first shown, if it is headed
    /// somewhere the user can see.
    ///
    /// Focus follows the newest window; #12 turns this into a policy. It used
    /// to happen in `new_toplevel`, which is too early to know where the window
    /// is going: a window opened by its application is told to the layout only
    /// here, at its first frame, so every new toplevel took the keyboard before
    /// anything had placed it -- and one a layout then parked on a workspace
    /// nobody is looking at kept it. A window launched with `sol.spawn` was
    /// placed before its application existed, and took the keyboard when the
    /// application arrived, wherever that was. With `follow_overflow = false`
    /// both are ordinary, and every key typed afterwards went to a window the
    /// user could not see (#134 review). So the grant waits for the window to
    /// have its place, and asks [`Self::on_stage`] -- the question the focus
    /// rules ask everywhere else -- first. Declined, the keyboard is left where
    /// it was. The two routes are
    /// `a_window_that_overflows_to_a_hidden_workspace_does_not_take_the_keyboard`
    /// and
    /// `a_launched_window_parked_on_a_hidden_workspace_does_not_take_the_keyboard_when_it_arrives`;
    /// a launched window that is on screen still takes it, in
    /// `a_launched_window_that_overflows_with_the_view_takes_the_keyboard_when_it_arrives`.
    ///
    /// **An xdg window and an X11 one alike, and through
    /// [`Self::focus_window`]**: the rule is [`first_focus`], which says why
    /// X11 windows are named in it. Through `focus_window` rather than a bare
    /// [`Self::give_keyboard`], because the grant is only part of focus. The
    /// scripts are told `focus`, as they are for every other way the keyboard
    /// moves, and X11 is told in its own terms (`xwayland::activate`, whose
    /// note says what an X11 window does when it is not). Before the second
    /// round of #134's review a new xdg window in floating or tiling got the
    /// bare grant and no `focus`, while in scrolling it got all of it through
    /// the strip's own `sol.focus`; the three modes now take one path, which
    /// `a_window_opening_while_floating_takes_the_keyboard_as_a_focus` and its
    /// `tiling` and `scrolling` twins pin.
    ///
    /// Through the gate, which refuses it while locked: this is a client
    /// opening a window of its own accord, with nobody at the machine, and
    /// until the gate existed it was the shortest way to the password.
    /// `focus_window` asks the gate before it does anything, so a refused
    /// window is not raised or activated either
    /// (`a_window_that_opens_while_locked_does_not_take_the_keyboard`).
    fn offer_keyboard(&mut self, window: &Window, pane: crate::pane::PaneId) {
        let landed = self.settling();
        let screens = self.screens();
        let offer = self.panes.get(pane).map_or(FirstFocus::Stay, |held| {
            first_focus(
                ClientKind::of(window),
                held.managed(),
                self.on_stage(held, &screens, landed),
            )
        });
        match offer {
            FirstFocus::Focus => self.focus_window(window, SERIAL_COUNTER.next_serial()),
            FirstFocus::Stay => tracing::debug!(
                pane = pane.get(),
                "a window opened where nobody can see it, and the keyboard stayed put"
            ),
        }
    }

    /// Offer a newly shown window to whatever script wants to animate it in.
    /// Read the configuration again and swap it in.
    ///
    /// The QML cache is cleared and every frame rebuilt too, so editing a
    /// decoration is the same one keystroke as editing a binding. A file that
    /// fails to load leaves the running configuration alone: a typo should
    /// cost a log line, not the session.
    ///
    /// ## A reload replaces the scripts, not the session
    ///
    /// The session is older than the scripts reading it. The windows, the
    /// monitors, the workspace in view and the layout in charge all outlive
    /// `super+shift+r`; the Lua state does not. So the second half of a reload
    /// is putting the new scripts back in touch with the session they have
    /// inherited, and it has two parts, both of which are the contract on
    /// [`Scripts::load_carrying`]:
    ///
    ///  * `Scripts::kept` and `load_carrying` hand back what the old scripts
    ///    asked to keep, *before* the new ones run, so a script's top level
    ///    sees its own state rather than its defaults;
    ///  * `restore`, `monitors` and `layout` re-announce the world, in that
    ///    order, so nothing has to be kept that could be recomputed.
    ///
    /// **The order is the same one a hotplug uses** — see
    /// [`Self::settle_monitors`] — with `restore` in front of it. That is not
    /// a coincidence to be tidied away later: "the screens are not the screens
    /// you knew" is exactly a new script set's position, and a layout that
    /// handles a monitor arriving already handles this.
    ///
    /// **This is what issue #116 was.** Only `layout` reached the new scripts,
    /// and only by accident: shipped `init.lua` calls `sol.monitors`, whose
    /// command happens to trigger a relayout. `workspaces.lua` regrouped every
    /// window onto desk 1 from that `layout` while desk 1 was still carried
    /// two screen-widths off-stage by the *previous* session's view, and
    /// nothing put it back — `super+1` did not, because the fresh Lua state
    /// believed workspace 1 was already showing.
    pub(crate) fn reload(&mut self) {
        let path = Scripts::config_path();
        // Collected before the new configuration is even read, because reading
        // it is what may fail, and the failure path has to leave the running
        // scripts -- and therefore their keep -- untouched.
        let carried = self.scripts.as_ref().map(Scripts::kept).unwrap_or_default();
        match Scripts::load_carrying(&path, carried) {
            Ok(scripts) => {
                crate::qml::clear_cache();
                let style = self.decorations.style().map(ToOwned::to_owned);
                // Twice, and to the same place it started, to defeat
                // `set_style`'s "nothing changed" guard. What the second call
                // does is *rebuild* every existing frame rather than drop it
                // -- see the comment on it, and the reason: dropping leaves
                // every open window bare until it is reopened -- so a window
                // that is framed before a reload is framed after it, by a
                // different `Decoration` built from the file as it now reads.
                self.decorations.set_style(&mut self.panes, None);
                self.decorations.set_style(&mut self.panes, style);
                self.start_scripts(Some(scripts));
                // The re-announcement, in the order the doc comment states.
                // Three dispatches and not one, each with its own snapshot,
                // because what `monitors` does changes what `layout` is
                // looking at -- `workspaces.lua` moves every desk in the first
                // and arranges the windows on the one in view in the second.
                self.trigger_restored();
                self.trigger_monitors_changed();
                self.trigger_relayout();
                self.redraw = true;
                tracing::info!(config = %path.display(), "configuration reloaded");
                // And then look at what that produced -- at where it *lands*,
                // not at this frame, which is still the previous session's.
                // See `everything_is_off_stage` for both halves: why the
                // question is asked here and nowhere else -- a reload is both
                // the keypress that lost the desktop and the keypress anybody
                // reaches for when it is gone, so it is the one moment where
                // the answer is worth having whichever way it comes out -- and
                // why asking it of the current frame answered about the wrong
                // session.
                if self.everything_is_off_stage() == Some(true) {
                    tracing::warn!(
                        "when the movement this reload started has landed, every window will be \
                         drawn outside every screen. If that is not simply a workspace with \
                         nothing on it, a selection is carrying the desktop off-stage and only \
                         something that names that selection can carry it back: switch workspace \
                         away and back again, which re-states where every desk sits"
                    );
                }
            }
            Err(err) => {
                tracing::error!(?err, config = %path.display(), "reload failed, keeping what was running");
            }
        }
    }

    /// Take the scripts, and act on whatever they asked for while loading.
    pub(crate) fn start_scripts(&mut self, scripts: Option<Scripts>) {
        // Before the scripts run, so `sol.keyboard()` answers truthfully even
        // in a configuration that never calls `sol.keyboard{…}` -- which is
        // the common case, since the useful default is whatever the session's
        // `XKB_DEFAULT_*` already said.
        self.keyboard = crate::keymap::describe(self);
        let Some(mut scripts) = scripts else {
            self.scripts = None;
            return;
        };
        let outcome = scripts.startup();
        self.scripts = Some(scripts);
        self.apply(outcome);
    }

    /// The Developer Tweaks panel, built on first use.
    ///
    /// A second shell surface rather than anything new: it is QML hosted in
    /// the compositor, which is a thing that already exists here. What it
    /// Declare a surface, or replace one of the same name.
    ///
    /// Re-declaring something identical keeps its rasterisations, because
    /// every reload re-runs the whole configuration and re-declares
    /// everything: without that check a `super+shift+r` that changed a gap
    /// would re-decode every wallpaper on every monitor.
    pub(crate) fn declare_surface(&mut self, declared: crate::scripted::Declaration) {
        if self.surfaces.declare(declared) {
            self.redraw = true;
        }
    }

    pub(crate) fn remove_surface(&mut self, name: &str) {
        if self.surfaces.remove(name) {
            self.redraw = true;
        }
    }

    /// Offer the pointer to the scripted surfaces above the windows, or below.
    ///
    /// Two calls rather than one, and the split is the same one
    /// wlr-layer-shell makes: a bar at `top` gets the click before the window
    /// under it, and a dock at `bottom` gets it only if no window wanted it.
    /// Without the split an interactive background would swallow every click
    /// on the desktop, and the symptom would be "windows stopped responding"
    /// rather than anything mentioning wallpapers.
    ///
    /// Returns whether one took it.
    pub(crate) fn surface_pointer(
        &mut self,
        above_windows: bool,
        location: Point<f64, Logical>,
        pressed: Option<bool>,
    ) -> bool {
        let Some((output, id, area)) = self.surface_claiming(above_windows, location) else {
            return false;
        };
        let Some(surface) = self.surfaces.get_mut(id) else {
            return false;
        };
        if !surface.pointer(&output, area, location.x, location.y, pressed) {
            // The area contained the point — `surface_claiming` said so — so
            // the only way back here is an instance that would not build, which
            // is a scene that failed to load and logged as much. It draws
            // nothing and it takes nothing.
            return false;
        }
        self.redraw = true;
        self.settle_surfaces();
        true
    }

    /// Which scripted surface, if any, claims `location` on its side of the
    /// windows.
    ///
    /// **Split out of [`Self::surface_pointer`] so the pointer can ask the
    /// question without answering it.** The cursor has to know whether a press
    /// here would be taken by a bar or a panel — that is the second half of
    /// this finding — and the one thing it must not do is *deliver* to find
    /// out: `surface_pointer` pushes hover into the scene and damages the
    /// screen, and [`Self::reassert_cursor`] runs every frame. A probe with
    /// those side effects would repaint the session continuously for as long as
    /// the pointer rested on a bar, and would run a scene's hover handlers on a
    /// pointer that never moved.
    ///
    /// So the geometry is here and the delivery is there, and there is still
    /// one predicate: `surface_pointer` reaches its surface through this and
    /// cannot pick a different one.
    fn surface_claiming(
        &self,
        above_windows: bool,
        location: Point<f64, Logical>,
    ) -> Option<(Output, crate::scripted::SurfaceId, Rectangle<i32, Logical>)> {
        if self.surfaces.iter().all(|surface| !surface.interactive()) {
            return None;
        }
        let output = monitor::at(&self.space, location)?;
        let geometry = self.space.output_geometry(&output)?;
        let primary = self.primary_output();

        // Topmost first, so a surface drawn over another gets the press.
        let order = if above_windows {
            [crate::scripted::Layer::Overlay, crate::scripted::Layer::Top]
        } else {
            [
                crate::scripted::Layer::Bottom,
                crate::scripted::Layer::Background,
            ]
        };

        for layer in order {
            // Where each of them is *drawn*, not merely where it was declared:
            // a surface carried off by a group is not under the pointer either,
            // which is the same rule a window follows. Without it, a wallpaper
            // that slid away with its workspace goes on eating clicks on the
            // workspace that replaced it.
            let claimed = self
                .surfaces
                .iter()
                .filter(|surface| surface.interactive() && surface.layer() == layer)
                .filter_map(|surface| {
                    let area = surface.area_on(&output, geometry, primary.as_ref())?;
                    Some((surface.id(), self.carried(surface.id(), &output, area)))
                })
                .find(|(_, area)| area.to_f64().contains(location));
            if let Some((id, area)) = claimed {
                return Some((output, id, area));
            }
        }
        None
    }

    /// Act on whatever a scripted surface asked for.
    ///
    /// The scene sets `action`, this takes it and hands it to whoever is
    /// listening, by surface name. A panel's buttons therefore live entirely
    /// in the script that declared it -- which is what turned the Developer
    /// Tweaks panel from a compositor feature into `lua/tweaks.lua`.
    pub(crate) fn settle_surfaces(&mut self) {
        let mut asked: Vec<(String, String)> = Vec::new();
        for surface in self.surfaces.iter_mut() {
            if let Some(action) = surface.taken_action() {
                asked.push((surface.name().to_owned(), action));
            }
        }
        for (name, action) in asked {
            let snapshot = self.snapshot();
            let Some(mut scripts) = self.scripts.take() else {
                return;
            };
            let outcome = scripts.surface_action(&name, &action, snapshot);
            self.scripts = Some(scripts);
            self.apply(outcome);
        }
    }

    /// Drop the rasterisations belonging to monitors that are no longer there.
    ///
    /// Each is a full-screen image held for a screen that has gone -- on a
    /// laptop docked and undocked all day that is a slow leak of exactly the
    /// largest thing the compositor allocates.
    fn prune_surfaces(&mut self) {
        let live: Vec<String> = self.space.outputs().map(Output::name).collect();
        for surface in self.surfaces.iter_mut() {
            surface.keep_only(&live);
        }
    }

    /// Tell scripts focus moved.
    fn trigger_focus(&mut self, window: &Window) {
        let id = self.window_id(window);
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return;
        };
        let outcome = scripts.focused(id, snapshot);
        self.scripts = Some(scripts);
        self.apply(outcome);
    }

    /// Offer a resize to scripts. Returns whether a layout took it.
    ///
    /// **`request.edge_at` is where the dragged edge should go, per axis.** Not
    /// a delta, and since #124 not the pointer either. This doc has been wrong
    /// about it twice: it called it "the delta" for the whole life of the event
    /// while `ResizeGrab::motion` recorded `event.location`, and #120 corrected
    /// that to "where the pointer is" — accurate about the code, and the code
    /// was the defect. A seam set from the pointer lands *under the cursor*, so
    /// a drag begun anywhere but exactly on the edge threw that edge to the
    /// cursor on its first frame.
    ///
    /// It is still a position rather than a delta, for the reason
    /// `ResizeRequest` gives: a seam set from a position is idempotent, and one
    /// accumulated from deltas feeds the layout's own response back in as its
    /// next input. It is nonetheless relative to the grab, because
    /// `crate::input::resize::dragged_edge` builds it from the pane's own
    /// laid-out edge and the drag's total movement rather than from the seat.
    ///
    /// It is in the layout's **outer** space — the space `sol.place` writes and
    /// `tree:layout` returns — because it comes from `Solium::pane_laid_out`,
    /// which is the rectangle `sol.place` was last handed for this pane. Not
    /// from `Solium::pane_outer`, which is that rectangle only until the client
    /// commits a size of its own.
    ///
    /// The pair after it is the side of the window being dragged on each axis
    /// — `"left"` or `"right"`, `"top"` or `"bottom"`, or nil for an axis that
    /// is not in play. Sides and not an axis pair, because which seam a tiled
    /// drag moves depends on which edge the hand is on; see
    /// [`crate::input::resize::sides`].
    pub(crate) fn trigger_resize(&mut self, request: &ResizeRequest) -> bool {
        let id = self.window_id(&request.window);
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return false;
        };
        let outcome = scripts.resized(
            id,
            request.edge_at,
            crate::input::resize::sides(request.edges),
            snapshot,
        );
        self.scripts = Some(scripts);
        let handled = outcome.handled && !outcome.commands.is_empty();
        self.apply(outcome);
        handled
    }

    pub(crate) fn trigger_monitors_changed(&mut self) {
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return;
        };
        let outcome = scripts.monitors_changed(snapshot);
        self.scripts = Some(scripts);
        self.apply(outcome);
    }

    /// Tell the scripts they have replaced a running session's, not started one.
    ///
    /// Called from [`Self::reload`] and from nowhere else: a cold start has
    /// nothing to restore, and firing it there would make the event mean
    /// "loaded", which is a thing a script's own top level already is.
    fn trigger_restored(&mut self) {
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return;
        };
        let outcome = scripts.restored(snapshot);
        self.scripts = Some(scripts);
        self.apply(outcome);
    }

    pub(crate) fn trigger_relayout(&mut self) {
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return;
        };
        let outcome = scripts.relayout(snapshot);
        self.scripts = Some(scripts);
        self.apply(outcome);
    }

    pub(crate) fn trigger_close(&mut self, pane: crate::pane::PaneId) {
        let id = pane.get();
        // **The close is over, so nothing may bring this window back** (#128).
        // The pane outlives this call: `Self::depart`, which is where a client
        // destroying its toplevel lands, turns it into what fades out for
        // `pane::LEAVING` when there is anything to fade, and otherwise it
        // lasts the rest of the frame -- every frame runs the Wayland dispatch,
        // then `settle`, and only then `sync_panes`, which retires it.
        // A client that went near its grace deadline was still `asked_at` in
        // that `settle`, so `settle_refused` gave the dead window back and the
        // layout was told `refused` after `close`: a leaf kept for a window
        // that no longer exists, for good. See
        // `a_client_that_goes_at_its_grace_deadline_is_not_refused_after_it_closed`.
        //
        // All three fields, because two routes lead back into `give_back`:
        // `settle_refused` on `asked_at`, and `settle_closing`'s retry on a due
        // `closing_at` with an answer owed. Only the first is driven by that
        // test; the second needs a declined give-back to reach, and the one way
        // this suite has to decline one, `present::jam_slot`, never lets go.
        //
        // **And the pane is marked gone, which is what keeps it leaving once
        // the timers are cleared** (#128's review, findings 5 and 7). Clearing
        // them alone made a window that no longer exists an ordinary window
        // for the rest of the frame: listed in `sol.windows()` as not leaving,
        // placed by a stateless layout and drawn at full opacity by
        // `move_pane`, closed a second time by `close_pane`, and put back into a
        // tree by an `adopt`. `Pane::leaving` answers yes for a gone pane, and
        // the snapshot leaves it out of every event but this one. See
        // `a_window_that_has_gone_is_neither_placed_nor_closed_again` and
        // `adopt_in_the_frame_a_window_went_keeps_no_leaf_for_it`.
        //
        // Before the dispatch, now that `gone` answers for it: a layout placing
        // the window in `close` itself -- a stateless one places every row it
        // is handed -- still finds it leaving and leaves alone the transform
        // holding it invisible. See
        // `a_layout_placing_a_closed_window_does_not_show_it_again`. And
        // whether or not there are scripts, because a session with none has a
        // deadline to disarm all the same.
        if let Some(pane) = self.panes.get_mut(pane) {
            pane.forget_asked();
            pane.stop_closing();
            pane.settled_answer();
            pane.went();
        }
        // Only for the snapshot, and the snapshot is the whole of what a script
        // sees. The pane is still here on two of the three routes in -- since
        // #126, `Self::depart` and `settle_closing`'s loading pane both call
        // this before the pane goes, and the first is
        // `a_client_that_disconnects_fades_out_and_is_told_gone_once`'s
        // `close N*` -- so a dialog waiting on this window would otherwise be
        // re-centred on the rect of the window that is leaving, and this pass
        // is the last one: nothing runs again to take it off the window that
        // moves into that space. See [`Self::parented`]. **Not on the third**:
        // the token-adoption merge in [`Self::claim_into`] removes the pane
        // first, because there both panes hold the same `Window` and this
        // snapshot would list it twice. Read in its code; no test drives it.
        self.closing = Some(pane);
        let snapshot = self.snapshot();
        self.closing = None;
        if let Some(mut scripts) = self.scripts.take() {
            let outcome = scripts.closed(id, snapshot);
            self.scripts = Some(scripts);
            self.apply(outcome);
        }
    }

    /// A window is going on its own: its client closed it, quit, crashed or was
    /// killed, its X11 window unmapped, or its application never arrived.
    /// Tell the scripts it has gone, and keep it on screen long enough to fade
    /// out as a window the compositor closes does (#126).
    ///
    /// **Every way a window goes on its own comes here**: `toplevel_destroyed`,
    /// `CompositorHandler::destroyed` for a window's own surface, an X11
    /// unmap, `settle_loading` and a failed spawn. A close the compositor asked
    /// for comes here too, when its client finally goes, and finds its fade
    /// already landed and nothing left to fade (`asked_at` below). Two ways
    /// out do not come here at all: a loading pane closed by hand, which
    /// `settle_closing` removes once its own fade has landed, and the
    /// token-adoption merge in [`Self::claim_into`], which is not a window
    /// leaving.
    ///
    /// **What it leaves is decided before anyone is told**, because telling
    /// is what moves things: `close` runs a layout, which grows the
    /// neighbours into this window's space, and a stateless layout places
    /// every row of `close`'s snapshot, this window's included. So the
    /// rectangle it is drawn at, the selections carrying it, its title and its
    /// picture are all read first, and its fade starts first, from where it
    /// stands. `move_pane` then declines to move a pane that has gone.
    ///
    /// **Then `close`, once, with the pane still here and still the window it
    /// was.** The same event a compositor's own close ends in and nothing
    /// else: no `closing`, because nobody asked for this close and there is
    /// no refusal to come back from. A layout reflows at `close`, and the
    /// window fades where it stood while its neighbours grow in, which is the
    /// picture #128 gives a close the compositor asked for. Its row in that
    /// snapshot says `leaving`, and it is in no snapshot after.
    /// `a_window_that_closes_itself_hands_its_space_over_as_it_fades`.
    ///
    /// **Then it becomes [`crate::pane::Content::Leaving`]** -- out of the
    /// space, and in the stack where it was, over the panes it was over and
    /// over the ones the layout grew into its space at `close`
    /// ([`crate::pane::Left::over`]), drawn from what it left
    /// ([`crate::pane::Remains`]) and from nothing else, and dropped by
    /// [`Self::settle_leaving`] when its fade is over. Or, with nothing to
    /// fade, it goes as it did before #126: a client's pane at the end of the
    /// frame in `sync_panes`, a loading pane at once.
    ///
    /// Asked at most once per window: most windows are heard going twice --
    /// a disconnecting client's surface and then its toplevel, an X11 window's
    /// surface and its unmap -- and `close` means gone exactly once.
    pub(crate) fn depart(&mut self, id: crate::pane::PaneId) {
        use crate::pane::{Left, Remains};
        enum Keep {
            Scene,
            Picture(crate::remains::Picture),
            Lost,
        }

        let now = self.clock.now();
        let Some(pane) = self.panes.get(id) else {
            return;
        };
        if pane.gone() || pane.ghost() {
            return;
        }
        let window = pane.client().cloned();
        let outer = self.pane_outer(pane);
        let geometry = self.pane_geometry(pane);
        let groups = if self.groups.is_empty() {
            Vec::new()
        } else {
            let monitor = self.named_monitor_of(outer);
            self.groups.holding_window(id.get(), monitor.as_deref())
        };
        let title = self.pane_title(id);
        let focused = self.looks_focused(id);
        // **On the fade it is already on, if a close the compositor started is
        // playing**: a client quitting inside those 190 ms leaves on that
        // close's schedule and from that close's transform, rather than
        // starting again from a window already half gone.
        // `a_client_that_quits_during_a_close_leaves_on_that_close`.
        let fading = pane.closing_at();
        let since = fading.map_or(now, |due| due.saturating_sub(present::CLOSING));
        // **Nothing left to fade** once that close has landed: the window has
        // been held at opacity zero since, waiting for its client to answer.
        let landed = pane.asked_at().is_some();
        let keep = if !pane.managed() || landed {
            // A menu, a tooltip or a drag icon goes the way a Wayland popup
            // does, at once.
            None
        } else if pane.has_standing_scene() {
            Some(Keep::Scene)
        } else {
            window.as_ref().and_then(|window| {
                let picture = crate::remains::Picture::of(window, self.textures.as_ref());
                if picture.drawable() {
                    Some(Keep::Picture(picture))
                } else if self.client_ready(window) {
                    Some(Keep::Lost)
                } else {
                    // Never painted, or unmapped itself before it went: there
                    // was nothing of it on screen to fade.
                    None
                }
            })
        };
        // Where it is in the stack, read before `close` lets a layout restack
        // anything. See `crate::pane::Left::over`.
        let mut over = if keep.is_some() {
            self.stacked_under(pane)
        } else {
            Vec::new()
        };
        if keep.is_some()
            && fading.is_none()
            && let Some(pane) = self.panes.get(id)
        {
            present::close(pane, outer, now);
        }
        // Where the layout has every other window, so what it grows into this
        // one's space at `close` can be put under the fade. Only when there
        // will be a fade.
        let laid: Vec<(crate::pane::PaneId, Rectangle<i32, Logical>)> = if keep.is_some() {
            self.panes
                .iter()
                .filter(|each| each.id() != id && !each.ghost())
                .map(|each| (each.id(), self.laid_out_at(each)))
                .collect()
        } else {
            Vec::new()
        };

        self.trigger_close(id);

        for (other, was) in laid {
            let grew_in = self
                .panes
                .get(other)
                .map(|each| self.laid_out_at(each))
                .is_some_and(|now| now != was && now.overlaps(outer));
            if grew_in && !over.contains(&other) {
                over.push(other);
            }
        }

        let Some(keep) = keep else {
            // A client's pane is retired by `sync_panes` once the space has
            // let go of its window, as it always was. A pane that never had a
            // client has no window for the space to let go of, and `sync_panes`
            // keeps every such pane -- so it goes here.
            if window.is_none() {
                self.panes.remove(id);
            }
            self.redraw = true;
            return;
        };
        // Out of the space now rather than at its next `refresh`: until then
        // `sync_panes` would find the element with no pane holding it, and
        // give it a new one.
        if let Some(window) = window.as_ref() {
            self.space.unmap_elem(window);
        }
        let Some(pane) = self.panes.get_mut(id) else {
            return;
        };
        let remains = match keep {
            Keep::Scene => pane
                .take_standing_scene()
                .map_or(Remains::Lost, Remains::Scene),
            Keep::Picture(picture) => Remains::Picture(picture),
            Keep::Lost => Remains::Lost,
        };
        tracing::debug!(
            pane = id.get(),
            remains = match &remains {
                Remains::Picture(_) => "picture",
                Remains::Scene(_) => "scene",
                Remains::Lost => "nothing",
            },
            "a window went on its own and fades out"
        );
        pane.leave(Left {
            since,
            outer,
            geometry,
            groups,
            over,
            title,
            focused,
            remains,
            fill: smithay::backend::renderer::element::Id::new(),
        });
        self.panes.changed();
        self.redraw = true;
    }

    /// End the fade of every window that went and is drawn from `surface`,
    /// if this commit gives `surface` a new buffer: its client kept the
    /// surface and is using it again, and the texture the fade holds is the
    /// one the new buffer will be uploaded into. See
    /// `crate::remains::Picture::holds`.
    ///
    /// The fade ends rather than going on without that surface: this is a
    /// window coming back -- a hide and a show -- and it is about to be drawn
    /// by a pane of its own.
    fn let_go_of_reused(&mut self, surface: &WlSurface) {
        if !self.panes.iter().any(Pane::ghost) {
            return;
        }
        let attached = with_states(surface, |states| {
            matches!(
                states
                    .cached_state
                    .get::<smithay::wayland::compositor::SurfaceAttributes>()
                    .current()
                    .buffer,
                Some(smithay::wayland::compositor::BufferAssignment::NewBuffer(_))
            )
        });
        if !attached {
            return;
        }
        let id = surface.id();
        let reused: Vec<crate::pane::PaneId> = self
            .panes
            .iter()
            .filter(|pane| {
                pane.left().is_some_and(|left| {
                    matches!(&left.remains, crate::pane::Remains::Picture(picture) if picture.holds(&id))
                })
            })
            .map(Pane::id)
            .collect();
        for pane in reused {
            tracing::debug!(
                pane = pane.get(),
                "a surface a window that went is drawn from was given a new buffer"
            );
            self.panes.remove(pane);
            self.redraw = true;
        }
    }

    /// Tell scripts a close has begun, so a layout can reflow now.
    ///
    /// See [`Scripts::closing`]. Fired from [`Self::close_pane`] alone.
    fn trigger_closing(&mut self, pane: crate::pane::PaneId) {
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return;
        };
        let outcome = scripts.closing(pane.get(), snapshot);
        self.scripts = Some(scripts);
        self.apply(outcome);
    }

    /// Tell scripts a close was refused and the window is back.
    ///
    /// See [`Scripts::refused`]. Fired from [`Self::give_back`] alone.
    fn trigger_refused(&mut self, pane: crate::pane::PaneId) {
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return;
        };
        let outcome = scripts.refused(pane.get(), snapshot);
        self.scripts = Some(scripts);
        self.apply(outcome);
    }

    /// Tell scripts a drag finished, so a layout can put the window back.
    pub(crate) fn trigger_drop(&mut self, window: &Window, x: f64, y: f64) {
        let id = self.window_id(window);
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return;
        };
        let outcome = scripts.dropped(id, x, y, snapshot);
        self.scripts = Some(scripts);
        self.apply(outcome);
    }

    /// Offer a modified wheel turn to scripts. Returns whether one took it.
    pub(crate) fn trigger_scroll(&mut self, dx: f64, dy: f64) -> bool {
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return false;
        };
        let outcome = scripts.scrolled(dx, dy, snapshot);
        self.scripts = Some(scripts);
        let handled = outcome.handled;
        self.apply(outcome);
        handled
    }

    fn trigger_open(&mut self, pane: crate::pane::PaneId) -> Opened {
        let id = pane.get();
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return Opened::default();
        };
        let outcome = scripts.opened(id, snapshot);
        self.scripts = Some(scripts);

        let opened = Opened {
            handled: outcome.handled && !outcome.commands.is_empty(),
            focused: outcome
                .commands
                .iter()
                .any(|command| matches!(command, Command::Focus { .. })),
        };
        self.apply(outcome);
        opened
    }

    /// Where a new window goes.
    ///
    /// Centred, then cascaded, so a second window is not hidden exactly behind
    /// the first. This is *not* a layout engine and is not trying to be one —
    /// E4 replaces it with floating, tiling and scrolling behind one interface.
    /// It exists because "every window at (0, 0)" is not a usable compositor.
    /// The size a window should open at, which is not always the one it asked
    /// for.
    ///
    /// A client picks its own first size and plenty pick one larger than the
    /// screen — Firefox and LibreOffice both do on a 1600x900 output. Nothing
    /// was bringing it down, and placement cannot help: a window wider than the
    /// display hangs off it wherever you put it. Firefox opened with its tab
    /// bar visible and everything below the fold past the bottom edge.
    ///
    /// A layout that claims the window overrides this a moment later. This is
    /// for the floating case, where nothing else has an opinion.
    fn fitted_size(&self, window: &Window) -> Size<i32, Logical> {
        let size = window.geometry().size;
        // A window that already has a place is measured against its own
        // monitor; a brand new one against the active one, which is where it
        // is about to be put.
        let area = self
            .real_geometry(window)
            .and_then(|real| self.work_area_of(real))
            .or_else(|| self.work_area());
        let Some(area) = area else {
            return size;
        };
        let insets = self.frame_insets(window);
        (
            size.w.min((area.size.w - insets.horizontal()).max(1)),
            size.h.min((area.size.h - insets.vertical()).max(1)),
        )
            .into()
    }

    fn initial_placement(&self, window: &Window, size: Size<i32, Logical>) -> Point<i32, Logical> {
        let Some(output) = self.work_area() else {
            return (0, 0).into();
        };

        const CASCADE: i32 = 44;
        const WRAP: usize = 6;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_possible_wrap,
            reason = "the index is taken modulo a small constant"
        )]
        let step = CASCADE * (self.panes.len() % WRAP) as i32;

        // The frame is above the client, so the client's own top edge starts
        // that far down: the pair has to fit in the work area, not just the
        // client.
        let insets = self.frame_insets(window);
        let outer_height = size.h + insets.vertical();
        let outer_width = size.w + insets.horizontal();

        let centred = |available: i32, window: i32| (available - window) / 2;
        let x = output.loc.x + centred(output.size.w, outer_width).max(0) + step + insets.left;
        let y = output.loc.y + centred(output.size.h, outer_height).max(0) + step + insets.top;

        // Kept on the output even if the cascade would walk a large window off
        // the bottom right.
        (
            x.min(output.loc.x + (output.size.w - size.w).max(0)),
            y.max(output.loc.y + insets.top)
                .min(output.loc.y + (output.size.h - outer_height).max(0) + insets.top),
        )
            .into()
    }

    /// The window owning a surface, if any.
    pub(crate) fn window_for(&self, surface: &WlSurface) -> Option<Window> {
        self.space
            .elements()
            .find(|window| window.wl_surface().as_deref() == Some(surface))
            .cloned()
    }
}

/// The pane a token was minted for, carried on the token itself.
///
/// This is the whole of the fix for launching through a wrapper: a token is a
/// thing we made and handed out, so whatever the program does to its processes,
/// the token that comes back is still the one we gave it.
struct LaunchedFor(crate::pane::PaneId);

#[cfg(test)]
mod tests;
