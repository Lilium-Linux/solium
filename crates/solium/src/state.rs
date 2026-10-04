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
                PopupSurface, PositionerState, SurfaceCachedState, ToplevelSurface,
                XdgShellHandler, XdgShellState, XdgToplevelSurfaceData,
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
    qml::hosted::{Asking, PointerKind, ScenePointer},
    script::{
        AnimationSpec, Command, Drawn, Outcome, Parentage, Rect, Scripts, Snapshot, WindowInfo,
    },
};

mod close;
mod commands;
mod handlers;
mod hit_test;
mod hosted;
mod keyboard;
mod lock_screen;
mod monitors;
mod open;
mod placement;
mod resize_bridge;
mod snapshot;
mod workspaces;
mod x11;

#[cfg(test)]
use handlers::{may_grab, popup_target};
pub(crate) use hit_test::{Chrome, owns};
#[cfg(test)]
use hit_test::{
    Claim, PaneHit, chrome_of, chrome_offered, claim_of, on_frame, pane_hit_of, shown_at,
    topmost_chrome,
};
pub(crate) use hosted::{GrabRoute, HostedGrab, HostedKeyboard, SceneRepeat};
#[cfg(test)]
use monitors::anywhere_on;
use open::Claimed;
#[cfg(test)]
use open::{ClientKind, FirstFocus, first_focus};
pub(crate) use placement::Standing;
use placement::{Change, outer_of};
#[cfg(test)]
use snapshot::to_rect;
pub(crate) use snapshot::{Limits, limits_of};
use workspaces::nothing_on_stage;
#[cfg(test)]
use workspaces::{SETTLED, put_away, staged};

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

/// A press a hosted scene took, and where: the pointer is that scene's until
/// every button is up (Ruling 7).
/// `tests::real_client::reflow_on_close::hosted::a_release_after_dragging_off_a_shell_button_reaches_the_scene`.
#[derive(Clone, Debug)]
pub(crate) struct ScenePress {
    pub(crate) surface: crate::scripted::SurfaceId,
    pub(crate) output: Output,
    pub(crate) area: Rectangle<i32, Logical>,
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
    /// Registers `zwlr_output_power_manager_v1`. See `power.rs`.
    #[expect(dead_code, reason = "holds the global; dropping it would remove it")]
    pub(crate) power_state: crate::power::PowerState,
    /// Which monitors are off, and which of those are dark. See `power.rs`.
    pub(crate) power: crate::power::Power,

    /// Everything a script has asked the compositor to draw in QML.
    ///
    /// The wallpaper is one of these and there is nothing in here that knows
    /// that. See `scripted.rs`.
    pub(crate) surfaces: crate::scripted::Surfaces,

    /// What Qt last took of each model hosted scenes read, so a frame sends
    /// only what changed since: `models::tests::a_batch_qt_cannot_take_is_sent_again_once_it_can`.
    pub(crate) published: crate::models::Published,

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
    /// The layout and the locks as the configuration and the scenes were last
    /// told them: `keyboard_change::tests::a_layout_switch_and_a_caps_toggle_by_key_are_told_once_each_with_russian_active`.
    pub(crate) keyboard_told: crate::keyboard_change::Told,
    /// Registers `zwp_text_input_manager_v3`, and holds every text field and
    /// its caret: `text_input::tests::an_enabled_field_has_its_caret_in_the_global_space`.
    pub(crate) text_inputs: crate::text_input::TextInputs,

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

    /// The active mode's name, as a script last reported it with `sol.status`
    /// (`input::tests::a_shifted_digit_fires_the_binding_that_names_the_digit`
    /// reads it back). The compositor does not know what modes exist; it keeps
    /// the name and logs it when it changes, and nothing draws it.
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
    /// The mouse buttons held, as Qt's `MouseButtons`, which every pointer
    /// event a hosted scene is told carries.
    /// `tests::real_client::reflow_on_close::hosted::a_right_press_on_a_scene_reaches_it_as_the_right_button_with_shift_held`.
    pub(crate) pointer_buttons: u32,
    /// A press a scene took, which holds the pointer for it until every
    /// button is up (Ruling 7).
    /// `tests::real_client::reflow_on_close::hosted::a_release_after_dragging_off_a_shell_button_reaches_the_scene`.
    pub(crate) scene_press: Option<ScenePress>,
    /// What became of the `sol.act`s the dispatch being applied ran, told to
    /// Lua once all of it is applied (Ruling 15).
    /// `tests::real_client::reflow_on_close::hosted::sol_act_answers_why_it_could_not`.
    pub(crate) settled_attempts: Vec<crate::script::Settled>,
    /// The actions `sol.act` was asked for that the compositor does not know,
    /// each logged the first time only.
    /// `tests::real_client::reflow_on_close::hosted::an_unknown_action_is_warned_of_the_first_time_only`.
    pub(crate) unknown_actions: std::collections::HashSet<String>,
    /// Whether the settled attempts are being told, so what a `done` asks
    /// for is told by that loop and not from inside it.
    /// `tests::real_client::reflow_on_close::hosted::a_done_that_acts_again_each_time_it_is_told_costs_rounds_not_the_session`.
    telling_attempts: bool,
    /// Whether a grab ended while a scene held a press, so the pointer goes
    /// back to what is under it at that press's release.
    /// `tests::real_client::reflow_on_close::hosted::a_popup_closed_during_a_press_inside_it_gives_the_pointer_back_at_the_release`.
    repoint_at_release: bool,
    /// The scene the pointer was last over, and the one this motion found.
    /// `tests::real_client::reflow_on_close::hosted::the_scene_hears_the_pointer_leave_when_it_moves_off_its_items`.
    pub(crate) scene_hovered: Option<(crate::scripted::SurfaceId, Output)>,
    pub(crate) scene_hover_seen: Option<(crate::scripted::SurfaceId, Output)>,
    /// Whether [`Self::settle_scenes`] is running, so nothing it runs runs
    /// it again (Ruling 11): the layout pass it runs declares surfaces in
    /// `tests::real_client::reflow_on_close::hosted::a_property_a_layout_handler_writes_reflows_the_windows_against_the_reserve_it_moved`.
    settling_scenes: bool,
    /// What hosted surfaces reserved when the layout last ran, which a
    /// change is measured against.
    /// `tests::real_client::reflow_on_close::hosted::a_scene_reserve_overrides_its_edge_and_reflows_the_layout_once`.
    laid_out_reserves: hosted::Reserves,
    /// Whether a declaration in the dispatch running now asks for the scenes
    /// to be settled once that dispatch is done.
    /// `tests::real_client::reflow_on_close::hosted::a_property_a_layout_handler_writes_reflows_the_windows_against_the_reserve_it_moved`.
    scenes_to_settle: bool,
    /// The one grab a hosted scene holds the pointer with (Ruling 12).
    /// `tests::real_client::reflow_on_close::hosted::while_a_grab_is_held_the_pointer_is_the_scenes`.
    pub(crate) hosted_grab: Option<HostedGrab>,
    /// The buttons whose press the compositor swallowed, by evdev code, so
    /// their release is swallowed too, wherever it lands; forgotten at the
    /// lock, behind which a release is the lock screen's.
    /// `tests::real_client::reflow_on_close::hosted::a_swallowed_unnamed_press_swallows_its_release_off_the_scene`,
    /// `tests::real_client::reflow_on_close::hosted::a_press_outside_a_grab_dismisses_it_and_is_swallowed_by_default`,
    /// `tests::real_client::reflow_on_close::hosted::the_lock_forgets_the_presses_a_shell_swallowed`.
    pub(crate) swallowed: std::collections::HashSet<u32>,
    /// The scene holding the keyboard, and the window it took the keyboard
    /// from (Ruling 14).
    /// `tests::real_client::reflow_on_close::hosted::the_window_gets_the_keyboard_back_when_the_shell_lets_go`.
    pub(crate) hosted_keyboard: Option<HostedKeyboard>,
    /// The keys whose press went to a scene, by xkb keycode, so their
    /// release does too and never reaches a window.
    /// `crate::input::tests::a_release_whose_press_went_to_a_scene_reaches_no_window`.
    pub(crate) keys_to_scene: std::collections::HashSet<u32>,
    /// The key held for the scene holding the keyboard, and when it repeats
    /// next. `crate::input::tests::a_held_key_repeats_into_the_scene_at_the_keyboards_rate`.
    pub(crate) scene_repeat: Option<SceneRepeat>,
    /// Every key told to the scene holding the keyboard, for the tests.
    #[cfg(test)]
    pub(crate) scene_keys: Vec<crate::qml::hosted::SceneKey>,
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
    /// What systemd and D-Bus activation have been told about this session,
    /// and the stop and unset it owes them on exit. See `session.rs`.
    pub(crate) session: crate::session::Session,
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

    /// Whose own size limits a floating drag is held to (#115). See
    /// [`crate::script::ClientSizes`].
    pub(crate) client_sizes: crate::script::ClientSizes,

    /// Whether `apply` is already inside the layout pass it runs to tell the
    /// scripts that a window's `cramped` changed (#115), so that pass does not
    /// run another. See `Solium::apply`.
    retelling_cramped: bool,

    /// Whether a `fullscreen` or `maximize` event is being told now, so that
    /// a change one of its listeners makes -- a `sol.toggle_fullscreen` in a
    /// `fullscreen` listener -- is made at once and not told again, which
    /// would be told again for ever. See `Solium::transition`.
    /// `a_listener_that_toggles_the_change_back_is_not_told_it_again`.
    telling_change: bool,

    /// Whether a `sol.monitors{}` was applied since the surfaces were last
    /// placed, so they are placed once the dispatch that applied it is done.
    /// `tests::real_client::a_runtime_primary_change_drops_the_old_primarys_scene`.
    monitors_rearranged: bool,

    /// How deep the dispatches running now are: an `apply` inside another, as
    /// the layout pass a `sol.monitors{}` runs is, or inside a reload or a
    /// hotplug, which places the surfaces itself once every handler it runs
    /// has run.
    /// `tests::real_client::a_binding_that_moves_the_primary_and_its_surface_together_keeps_the_scene`,
    /// `tests::real_client::a_reload_that_moves_a_monitor_keeps_the_scene_its_handler_declares_there`,
    /// `tests::real_client::a_hotplug_whose_handler_rearranges_the_monitors_places_the_surfaces_once`.
    dispatching: u32,

    /// How many times the surfaces were placed on the monitors, for the tests
    /// that say when they are:
    /// `tests::real_client::a_hotplug_whose_handler_rearranges_the_monitors_places_the_surfaces_once`.
    #[cfg(test)]
    instances_synced: u32,

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

    /// What a fullscreen window covers: `fullscreen.covers`. See
    /// [`crate::stack::Covers`].
    pub(crate) fullscreen_covers: crate::stack::Covers,

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
    /// Held to its client's own size limits since #115, unless the user has
    /// said not to believe them: see [`crate::input::resize::drag_rect`].
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
            pointer_buttons: 0,
            scene_press: None,
            settled_attempts: Vec::new(),
            unknown_actions: std::collections::HashSet::new(),
            telling_attempts: false,
            repoint_at_release: false,
            scene_hovered: None,
            scene_hover_seen: None,
            settling_scenes: false,
            laid_out_reserves: hosted::Reserves::new(),
            scenes_to_settle: false,
            hosted_grab: None,
            swallowed: std::collections::HashSet::new(),
            hosted_keyboard: None,
            keys_to_scene: std::collections::HashSet::new(),
            scene_repeat: None,
            #[cfg(test)]
            scene_keys: Vec::new(),
            reported_at: std::time::Duration::ZERO,
            xwm: None,
            x11_display: None,
            session: crate::session::Session::off(),
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
            power_state: crate::power::PowerState::new::<Self>(&display_handle),
            power: crate::power::Power::default(),
            surfaces: crate::scripted::Surfaces::default(),
            published: crate::models::Published::default(),
            groups: crate::group::Groups::default(),
            keymap: None,
            keyboard: crate::keymap::State::initial(),
            keyboard_told: crate::keyboard_change::Told::default(),
            text_inputs: crate::text_input::TextInputs::new(&display_handle),
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
            focusing: false,
            closing: None,
            pending_drop: None,
            pending_resize: None,
            retelling_cramped: false,
            telling_change: false,
            monitors_rearranged: false,
            dispatching: 0,
            #[cfg(test)]
            instances_synced: 0,
            client_sizes: crate::script::ClientSizes::default(),
            resize_hold: None,
            resize_bridge: None,
            resize_gesture: None,
            resize_ended: None,
            resizing: crate::resizing::Settings::default(),
            fullscreen_covers: crate::stack::Covers::default(),
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
    /// pass [`Self::settling`]. [`workspaces::SETTLED`] states it and names the tests on
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

    /// Drop a queued capture whose frame has gone away.
    pub(crate) fn forget_capture(
        &mut self,
        frame: &smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
    ) {
        self.pending_captures
            .retain(|capture| &capture.frame != frame);
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
        // What the frame's own tick changed in a scene, read one pass later
        // (Ruling 11):
        // `tests::real_client::reflow_on_close::hosted::a_reserve_a_scene_changes_on_its_own_is_read_after_the_frame`.
        self.settle_scenes();
        self.animating = animating;
        animating
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
    ///
    /// An inhibitor taken over D-Bus (`screensaver.rs`) has no surface to ask
    /// about, so it counts for as long as it is held, and like the rest not
    /// behind the lock:
    /// `a_dbus_inhibit_holds_the_idle_blank_off_and_uninhibit_lets_it_happen` and
    /// `a_dbus_inhibitor_holds_nothing_behind_the_lock_screen`.
    pub(crate) fn idle_inhibited(&self) -> bool {
        if self.lock.is_some() {
            return false;
        }
        // Collected first: `inhibiting` borrows `self.idle` and the visibility
        // test borrows the rest of `self`.
        self.idle.dbus_inhibiting()
            || self
                .idle
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

    /// Declare a surface, or change the one of the same name in place; only a
    /// new scene file replaces it
    /// (`scripted::tests::a_redeclared_property_is_written_into_the_live_scene`,
    /// `scripted::tests::a_new_scene_path_rebuilds_the_surface`).
    ///
    /// Re-declaring something identical keeps its rasterisations, because
    /// every reload re-runs the whole configuration and re-declares
    /// everything: without that check a `super+shift+r` that changed a gap
    /// would re-decode every wallpaper on every monitor. Any other declaration
    /// syncs the surface's instances there and then: a scene on every monitor
    /// it is now on, and none on the monitors it left
    /// (`tests::real_client::a_surface_redeclared_onto_another_monitor_drops_its_scene_on_the_first`).
    ///
    /// Only that surface's: another surface a handler has yet to declare
    /// again may still name where a monitor was before a hotplug, and judged
    /// by that it would lose a scene its own declaration is about to keep
    /// (`tests::real_client::one_handler_declaring_two_surfaces_again_keeps_both_their_scenes`).
    pub(crate) fn declare_surface(&mut self, declared: crate::scripted::Declaration) {
        let name = declared.name.clone();
        if self.surfaces.declare(declared) != crate::scripted::Declared::Same {
            let (outputs, primary) = (self.monitor_rects(), self.primary_output());
            if let Some(surface) = self
                .surfaces
                .named(&name)
                .and_then(|id| self.surfaces.get_mut(id))
            {
                surface.sync(&outputs, primary.as_ref());
            }
            self.redraw = true;
            self.settle_scenes_once_dispatched();
        }
    }

    pub(crate) fn remove_surface(&mut self, name: &str) {
        if self.surfaces.remove(name) {
            self.redraw = true;
            self.settle_scenes_once_dispatched();
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
    /// A surface is offered only the points its scene's items claim, for what
    /// the event asks: motion where an item takes hover or presses, a button
    /// or the wheel where one takes presses (Ruling 6;
    /// `tests::real_client::reflow_on_close::hosted::a_press_where_the_shell_draws_nothing_reaches_the_window_under_it`,
    /// `tests::real_client::reflow_on_close::hosted::a_hover_strip_hears_the_motion_and_leaves_the_window_its_press`).
    /// A press a scene took holds the pointer for it, wherever the pointer
    /// goes, until every button is up (Ruling 7;
    /// `tests::real_client::reflow_on_close::hosted::a_release_after_dragging_off_a_shell_button_reaches_the_scene`),
    /// and what the scene asked for on the way is acted on once the input
    /// dispatch is done, by [`Self::settle_scenes`]
    /// (`tests::real_client::a_click_on_a_hosted_button_is_acted_on_at_its_release`).
    ///
    /// `event` is `None` for a button Qt has no name for: a scene that takes
    /// a press where it is swallows it, and is not told of it (Ruling 9;
    /// `tests::real_client::reflow_on_close::hosted::a_button_qt_has_no_name_for_is_swallowed_where_a_shell_takes_a_press`).
    ///
    /// Returns whether one took it. None does while the session is locked:
    /// the pointer is the lock screen's, and nothing of the session's may
    /// notice it going past
    /// (`tests::real_client::lock_focus::the_wheel_over_a_hosted_scene_is_not_the_scenes_while_locked`).
    pub(crate) fn surface_pointer(
        &mut self,
        above_windows: bool,
        location: Point<f64, Logical>,
        event: Option<ScenePointer>,
    ) -> bool {
        if self.lock.is_some() {
            return false;
        }
        if let Some(held) = self.scene_press.clone() {
            if let Some(event) = event {
                if let Some(surface) = self.surfaces.get_mut(held.surface) {
                    surface.deliver(&held.output, held.area, location, event);
                }
                if matches!(event.kind, PointerKind::Release(_)) && event.buttons == 0 {
                    self.scene_press = None;
                    // A grab let go of during the press gave the pointer to
                    // no client; with no grab held now, the window under it
                    // has it back.
                    // `tests::real_client::reflow_on_close::hosted::a_popup_closed_during_a_press_inside_it_gives_the_pointer_back_at_the_release`.
                    if std::mem::take(&mut self.repoint_at_release) && self.hosted_grab.is_none() {
                        self.repoint_clients();
                    }
                }
            }
            self.redraw = true;
            return true;
        }
        let asking = event.map_or(Asking::Press, |event| Asking::of(event.kind));
        let Some((output, id, area)) = self.surface_claiming(above_windows, location, asking)
        else {
            return false;
        };
        // A button Qt has no name for is not told to a scene (Ruling 9), and
        // a scene that takes a press there still keeps it from what is under
        // it.
        // `tests::real_client::reflow_on_close::hosted::a_button_qt_has_no_name_for_is_swallowed_where_a_shell_takes_a_press`.
        let Some(event) = event else {
            return true;
        };
        let Some(surface) = self.surfaces.get_mut(id) else {
            return false;
        };
        if !surface.deliver(&output, area, location, event) {
            // A defence only: the claim already leaves out a monitor the
            // surface has no instance on, where `Surface::hit` answers
            // nothing
            // (`scripted::tests::a_surface_with_no_instance_on_a_monitor_claims_nothing_there`),
            // so a surface that claimed the point has an instance to take it.
            // Nothing is drawn where there is none, so nothing is taken.
            return false;
        }
        match event.kind {
            PointerKind::Motion => self.scene_hover_seen = Some((id, output)),
            PointerKind::Press(_) => {
                // The scene pressed is the one hovered from its press on: one
                // that came under a still pointer hears the motion that leaves
                // it after the release, and a scene hovered until then hears
                // the pointer leave it now.
                // `tests::real_client::reflow_on_close::hosted::a_scene_pressed_under_a_still_pointer_hears_it_leave`,
                // `tests::real_client::reflow_on_close::hosted::a_scene_pressed_over_another_hovered_one_takes_the_hover`.
                let pressed = Some((id, output.name()));
                if self
                    .scene_hovered
                    .as_ref()
                    .map(|(hovered, on)| (*hovered, on.name()))
                    != pressed
                {
                    let before = self.scene_hovered.replace((id, output.clone()));
                    if let Some((hovered, on)) = before
                        && let Some(surface) = self.surfaces.get_mut(hovered)
                    {
                        surface.leave(&on);
                    }
                }
                self.scene_press = Some(ScenePress {
                    surface: id,
                    output,
                    area,
                });
            }
            PointerKind::Release(_) | PointerKind::Wheel { .. } => {}
        }
        self.redraw = true;
        true
    }

    /// After a motion was offered to the scenes: tell the one the pointer
    /// left.
    /// `tests::real_client::reflow_on_close::hosted::the_scene_hears_the_pointer_leave_when_it_moves_off_its_items`.
    pub(crate) fn finish_scene_motion(&mut self) {
        let now = self.scene_hover_seen.take();
        if self.scene_press.is_some() {
            return;
        }
        let key = |hovered: &Option<(crate::scripted::SurfaceId, Output)>| {
            hovered.as_ref().map(|(id, output)| (*id, output.name()))
        };
        if key(&now) != key(&self.scene_hovered) {
            if let Some((id, output)) = self.scene_hovered.take()
                && let Some(surface) = self.surfaces.get_mut(id)
            {
                surface.leave(&output);
            }
            self.scene_hovered = now;
        }
    }

    /// Which scripted surface, if any, claims `location` on its side of the
    /// windows, for what `asking` asks of it.
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
        asking: Asking,
    ) -> Option<(Output, crate::scripted::SurfaceId, Rectangle<i32, Logical>)> {
        if self.surfaces.iter().all(|surface| !surface.interactive()) {
            return None;
        }

        // Above the windows, a script's surface claims a point only where it
        // is what is on top there, in `crate::stack`'s order: not under a
        // client's surface at its own layer or above, nor under the window
        // lifted over the bars.
        let above = self.topmost_above(location, Some(asking));
        if above_windows {
            return match above? {
                hit_test::Above::Script(output, id, area) => Some((output, id, area)),
                hit_test::Above::Client(..) | hit_test::Above::Lifted => None,
            };
        }

        // Below them, the same order walked on down, and the same rule: only
        // where nothing above the windows has the point, no window has it,
        // and no client's surface at a layer over the script's has it. A
        // client's surfaces there are not offered the pointer, and never were,
        // but one drawn over a script's surface is still over it.
        // `a_background_mapped_after_a_bottom_surface_stays_under_it` and
        // `a_window_over_a_scripted_dock_keeps_the_press`.
        if above.is_some() || self.windows_have(location) {
            return None;
        }
        let output = monitor::at(&self.space, location)?;
        let geometry = self.space.output_geometry(&output)?;
        for band in crate::stack::below() {
            match band {
                crate::stack::Band::Layer(layer, crate::stack::Owner::Script) => {
                    if let Some((id, area)) =
                        self.script_at(&output, geometry, layer, location, asking)
                    {
                        return Some((output, id, area));
                    }
                }
                crate::stack::Band::Layer(layer, crate::stack::Owner::Client) => {
                    // A layer map's geometry is in its own output's coordinates.
                    let local = location - geometry.loc.to_f64();
                    if layer::surface_under(&output, layer, local).is_some() {
                        return None;
                    }
                }
                // Neither is below the windows: `stack::below` never yields
                // them (`above_and_below_split_the_order_at_the_windows`).
                crate::stack::Band::Fullscreen | crate::stack::Band::Windows => {}
            }
        }
        None
    }

    /// The script's interactive surface at one layer whose scene claims
    /// `location` for what `asking` asks, first declared first -- the order
    /// `render::stacked` draws them in. A surface whose items take nothing
    /// there is not in the way of what is under it
    /// (`tests::real_client::reflow_on_close::hosted::a_press_on_a_rounded_corners_transparent_part_falls_through`).
    ///
    /// Where each of them is *drawn*, not merely where it was declared: a
    /// surface carried off by a group is not under the pointer either, which
    /// is the same rule a window follows. Without it, a wallpaper that slid
    /// away with its workspace goes on eating clicks on the workspace that
    /// replaced it.
    /// `a_scripted_overlay_is_drawn_over_a_scripted_bar_and_takes_the_press`.
    fn script_at(
        &self,
        output: &Output,
        geometry: Rectangle<i32, Logical>,
        layer: crate::scripted::Layer,
        location: Point<f64, Logical>,
        asking: Asking,
    ) -> Option<(crate::scripted::SurfaceId, Rectangle<i32, Logical>)> {
        let primary = self.primary_output();
        self.surfaces
            .iter()
            .filter(|surface| surface.interactive() && surface.layer() == layer)
            .filter_map(|surface| {
                let area = surface.area_on(output, geometry, primary.as_ref())?;
                Some((surface, self.carried(surface.id(), output, area)))
            })
            .find(|(surface, area)| surface.hit(output, *area, location).claims(asking))
            .map(|(surface, area)| (surface.id(), area))
    }

    /// Give every surface an instance on each monitor it is on, and no other:
    /// one on a monitor that arrived, and none on a monitor that has gone or
    /// that its placement or the primary monitor has moved off
    /// (`scripted::tests::a_surface_on_every_monitor_has_one_live_scene_per_monitor`,
    /// `tests::real_client::an_unplugged_monitor_still_loses_its_scene`,
    /// `tests::real_client::a_reload_that_moves_the_primary_drops_the_old_primarys_scene`).
    ///
    /// Called once the scripts have answered the change, never before: after
    /// both the `monitors` and the `layout` handlers. A surface either of them
    /// declares over a monitor's rectangle names where that monitor was until
    /// the handler runs, and judged by that it lost the scene the handler was
    /// about to keep
    /// (`tests::real_client::a_monitor_an_unplug_moves_keeps_the_scene_its_handler_declares_there`,
    /// `tests::real_client::a_layout_declared_strip_keeps_its_scene_through_an_unplug`),
    /// and once, even when a `monitors` handler rearranges the monitors
    /// (`tests::real_client::a_hotplug_whose_handler_rearranges_the_monitors_places_the_surfaces_once`).
    pub(crate) fn sync_instances(&mut self) {
        #[cfg(test)]
        {
            self.instances_synced += 1;
        }
        self.monitors_rearranged = false;
        let (outputs, primary) = (self.monitor_rects(), self.primary_output());
        for surface in self.surfaces.iter_mut() {
            surface.sync(&outputs, primary.as_ref());
        }
        // A scene built just now says what it reserves, and that is read in
        // this dispatch, not at the next frame:
        // `tests::real_client::a_scene_built_on_a_monitor_that_arrives_reserves_in_the_hotplugs_dispatch`.
        self.settle_scenes_once_dispatched();
    }

    /// Every monitor and its rectangle, as `Surface::sync` takes them.
    fn monitor_rects(&self) -> Vec<(Output, Rectangle<i32, Logical>)> {
        self.space
            .outputs()
            .filter_map(|output| Some((output.clone(), self.space.output_geometry(output)?)))
            .collect()
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
