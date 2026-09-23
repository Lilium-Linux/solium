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
    delegate_compositor, delegate_data_device, delegate_dmabuf, delegate_layer_shell,
    delegate_output, delegate_seat, delegate_shm, delegate_xdg_decoration, delegate_xdg_shell,
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
    script::{AnimationSpec, Command, Outcome, Parentage, Rect, Scripts, Snapshot, WindowInfo},
};

/// Whether `rect` lands on any of `screens`.
///
/// A free function over plain rectangles rather than a method, for one reason:
/// `Solium` cannot be built in a unit test — it needs a `Display` — and
/// neither can a `Space` with monitors mapped into it. This is the half that
/// decides, so this is the half that is testable, and [`Solium::on_any_output`]
/// is the two-line adapter that feeds it `space.outputs()`. Same trick, same
/// reason, as `offscreen::Scratch` being generic over what it keeps.
///
/// Exclusive, through `Rectangle::overlaps`, and deliberately the same call
/// `render::elements` makes when it culls a pane against one screen: a window
/// whose right edge is exactly the monitor's left edge has no pixel on it.
fn anywhere_on(
    rect: Rectangle<i32, Logical>,
    screens: impl IntoIterator<Item = Rectangle<i32, Logical>>,
) -> bool {
    screens.into_iter().any(|screen| screen.overlaps(rect))
}

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

/// Whether none of these drawn rectangles reaches any screen.
///
/// `None` when there is nothing to ask about — no screens, or no windows. The
/// free function over plain rectangles, for the same reason [`anywhere_on`] is
/// one: `Solium` needs a `Display` and cannot be built in a unit test, so the
/// half that decides is the half kept testable. The instant the rectangles were
/// measured at is the caller's, and is the other half — see
/// [`Solium::everything_is_off_stage`], which got it wrong.
fn nothing_on_stage(
    drawn: impl IntoIterator<Item = Rectangle<f64, Logical>>,
    screens: &[Rectangle<i32, Logical>],
) -> Option<bool> {
    if screens.is_empty() {
        return None;
    }
    let mut any = false;
    for rect in drawn {
        any = true;
        if screens.iter().any(|screen| screen.to_f64().overlaps(rect)) {
            return Some(false);
        }
    }
    any.then_some(true)
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

fn to_rect(rectangle: Rectangle<i32, Logical>) -> Rect {
    Rect {
        x: f64::from(rectangle.loc.x),
        y: f64::from(rectangle.loc.y),
        w: f64::from(rectangle.size.w),
        h: f64::from(rectangle.size.h),
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
    /// window whichever mode is running.
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
        // `pane_laid_out` describes and must go on seeing. `render::elements`
        // cuts the surfaces to the same rectangle. See `Self::shown_size`.
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

    /// A committed size, as this pane shows it: cut per axis to its tile.
    ///
    /// The committed size itself for a pane in no tile, and on any axis the
    /// client is already inside. Shared by `pane_geometry`, which is what the
    /// frame, the hit test and every transform read, and by the two places
    /// that draw a client's pixels -- `render::elements` cuts the surfaces to
    /// it, and `offscreen::capture_client` captures a masked client at it --
    /// so none of the three can hold the client to a different rectangle.
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
    /// Falls back to `pane_outer` for a pane no layout holds in a tile. That is
    /// a floating window — where nothing reads this, because no layout claims
    /// the drag — a dialog a layout centred without tiling it, a maximised or
    /// fullscreen window, or the frames between a window mapping and the first
    /// sweep, where the pane's own rectangle is the only answer there is.
    ///
    /// **A pane that was laid out and is not any more answers `pane_outer`
    /// too, and until #133 it did not.** It answered where the last layout
    /// left it, deliberately: the only reader was this edge, and clearing the
    /// field meant teaching every mode that stops placing a window to say so.
    /// #133 made `Pane::placed` the tile a client is held inside as well, and
    /// a stale one of those is a window cut down to where it used to be — so
    /// every way out of a tile clears it now, and `modes.use` is taught to say
    /// so with `sol.unplace`. What this function answers for a pane that *is*
    /// tiled is unchanged: `a_client_that_rounds_its_size_does_not_move_the_seam`
    /// still drives it, and `a_restored_window_goes_back_into_its_tile` covers
    /// the one way back that has to put the layout's rectangle back as well.
    pub(crate) fn pane_laid_out(&self, window: &Window) -> Option<crate::input::resize::LaidOut> {
        let pane = self.panes.get(self.panes.id_of(window)?)?;
        Some(crate::input::resize::LaidOut(
            pane.placed().unwrap_or_else(|| self.pane_outer(pane)),
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
        let frame = present::frame(pane, real, now);
        if self.groups.is_empty() {
            return frame;
        }
        // Only worked out when a selection has actually named a screen: this is
        // a geometric search over the outputs, per pane, per frame.
        let monitor = self
            .groups
            .names_monitors()
            .then(|| self.output_of(real).map(|output| output.name()))
            .flatten();
        self.groups
            .on_window(pane.id().get(), monitor.as_deref(), now)
            .apply(frame)
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
            pane.client().map_or_else(
                || pane.program().unwrap_or_default().to_owned(),
                |window| self.window_title(window),
            )
        })
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

    /// The monitor the user is working on.
    ///
    /// **The one the pointer is on.** One rule, and it needs no state: a new
    /// window opens where you are looking, `sol.monitor()` means the screen in
    /// front of you, and there is nothing to get out of step.
    ///
    /// The alternative — the focused window's monitor — sounds more careful and
    /// is worse in the case that actually happens: move the pointer to the
    /// second screen, click the empty desktop, open a terminal. Nothing was
    /// focused, so nothing changed, and the terminal appears on the screen you
    /// just looked away from. It is also inconsistent with the layout policy
    /// this project already has, where a new window splits *the window under
    /// the pointer*.
    ///
    /// A window is still maximised and fitted against the monitor **it** is on,
    /// not this one. Where a window goes and where a window is are different
    /// questions.
    pub(crate) fn active_output(&self) -> Option<Output> {
        let at = self
            .seat
            .get_pointer()
            .map(|pointer| pointer.current_location());
        match at {
            Some(at) => monitor::at(&self.space, at).or_else(|| monitor::nearest(&self.space, at)),
            // No pointer yet, which is the moment before the first input
            // device is reported. The primary monitor is the stable answer.
            None => self.primary_output(),
        }
    }

    /// The output whose place in the global space is exactly this rectangle.
    ///
    /// How the render loop gets from "the screen I am drawing" back to the
    /// output that owns the layer surfaces on it. Matched on geometry rather
    /// than carried through, so there is one fewer thing to keep in step.
    pub(crate) fn output_for(&self, screen: Rectangle<i32, Logical>) -> Option<Output> {
        self.space
            .outputs()
            .find(|output| self.space.output_geometry(output) == Some(screen))
            .cloned()
    }

    /// The monitor things belonging to *one* screen go on.
    ///
    /// A dock, a bar, a layer surface that named no output. Not the active
    /// monitor: a dock connects once, at startup, and pinning it to whichever
    /// screen the pointer happened to be over at that moment means it appears
    /// on a different monitor depending on where the mouse was left — which
    /// looks like the compositor placing it at random, because it is.
    ///
    /// `primary = true` in the configuration decides it. Otherwise the first
    /// monitor, which is at least stable across sessions.
    pub(crate) fn primary_output(&self) -> Option<Output> {
        let named = self.arrangement.primary();
        named
            .and_then(|name| {
                self.space
                    .outputs()
                    .find(|output| output.name() == name)
                    .cloned()
            })
            .or_else(|| self.space.outputs().next().cloned())
    }

    /// The monitor covering a point, or the nearest one to it.
    ///
    /// Never `None` while any output is mapped, on purpose. The callers are
    /// asking in order to place or size something, and "no monitor" is not an
    /// answer they can do anything with — an L-shaped arrangement has a hole in
    /// it, and a window whose centre lands in the hole still has to go
    /// somewhere.
    pub(crate) fn output_at(&self, point: Point<i32, Logical>) -> Option<Output> {
        let point = point.to_f64();
        monitor::at(&self.space, point).or_else(|| monitor::nearest(&self.space, point))
    }

    /// The monitor a rectangle is on, judged by its centre.
    ///
    /// Derived rather than stored, and that is the point: a window dragged to
    /// the next screen belongs to it the moment it is more than half way
    /// there, with no bookkeeping to keep in step and nothing to go stale.
    pub(crate) fn output_of(&self, rect: Rectangle<i32, Logical>) -> Option<Output> {
        self.output_at((rect.loc.x + rect.size.w / 2, rect.loc.y + rect.size.h / 2).into())
    }

    /// Whether a rectangle is on any monitor at all.
    ///
    /// **Not [`Self::output_of`], which never answers `None`**: that one falls
    /// back to the *nearest* monitor, because a window being dragged has to
    /// belong to something. This is the other question -- is any of this
    /// rectangle on a screen -- and a workspace that is hidden by being parked
    /// a screen away is precisely the case where the two answers differ.
    ///
    /// Asked by `render::prepare`, which runs before any output is bound and
    /// so has no one screen to test against; `render::elements` asks the same
    /// thing one monitor at a time and needs no such helper.
    pub(crate) fn on_any_output(&self, rect: Rectangle<i32, Logical>) -> bool {
        anywhere_on(
            rect,
            self.space
                .outputs()
                .filter_map(|output| self.space.output_geometry(output)),
        )
    }

    /// Every monitor's rectangle, for a caller asking about more than one pane.
    ///
    /// Collected once rather than per pane: [`Self::on_stage`] is asked in a
    /// walk, and re-deriving the screens inside it would make a question about
    /// one pane cost a pass over the outputs.
    fn screens(&self) -> Vec<Rectangle<i32, Logical>> {
        self.space
            .outputs()
            .filter_map(|output| self.space.output_geometry(output))
            .collect()
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
    /// **No screens is not "invisible".** `nothing_on_stage` answers `None`
    /// when there is nothing to measure against, and a compositor with no
    /// output bound yet must not decide that every window is unreachable — the
    /// caller would then refuse to focus anything at all. "Not known to be off
    /// stage" is the honest reading and the safe one.
    fn on_stage(&self, pane: &Pane, screens: &[Rectangle<i32, Logical>], landed: Duration) -> bool {
        let frame = self.drawn_at(pane, self.pane_outer(pane), landed);
        frame.shows() && nothing_on_stage([frame.rect], screens) != Some(true)
    }

    /// The monitor a surface is on, for telling it what to draw itself like.
    ///
    /// A window's own monitor when it has one, and the active one otherwise —
    /// which covers a surface that has committed but is not placed yet, and is
    /// the monitor it is about to be on.
    fn output_for_surface(&self, surface: &WlSurface) -> Option<Output> {
        self.window_for(surface)
            .and_then(|window| {
                self.real_geometry(&window)
                    .filter(|real| real.size.w > 0 && real.size.h > 0)
                    .and_then(|real| self.output_of(real))
            })
            .or_else(|| self.active_output())
    }

    /// A monitor's size in its own logical coordinates.
    ///
    /// The mode divided by the scale, which is what every protocol that
    /// positions something against an output speaks in.
    pub(crate) fn output_logical_size(
        &self,
        output: &Output,
    ) -> smithay::utils::Size<i32, Logical> {
        self.space
            .output_geometry(output)
            .map(|geometry| geometry.size)
            .unwrap_or_default()
    }

    /// Drop a queued capture whose frame has gone away.
    pub(crate) fn forget_capture(
        &mut self,
        frame: &smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
    ) {
        self.pending_captures
            .retain(|capture| &capture.frame != frame);
    }

    /// How many device pixels to a logical one, on a rectangle's own monitor.
    ///
    /// What everything the compositor draws itself has to rasterise at. One
    /// frame can span monitors at different scales, so this is asked per
    /// window rather than once for the frame.
    pub(crate) fn scale_of(&self, rect: Rectangle<i32, Logical>) -> f64 {
        self.output_of(rect)
            .map_or(1.0, |output| output.current_scale().fractional_scale())
    }

    /// The area windows may use on the monitor the user is working on.
    ///
    /// Whatever is left once every anchored surface has taken its exclusive
    /// zone — a number the *shell* chooses and may change at runtime, not a
    /// constant here. Every placement decision reads this rather than the raw
    /// output.
    pub(crate) fn work_area(&self) -> Option<Rectangle<i32, Logical>> {
        self.work_area_on(&self.active_output()?)
    }

    /// The same, for a monitor you already have.
    pub(crate) fn work_area_on(&self, output: &Output) -> Option<Rectangle<i32, Logical>> {
        // The layer map's zone is in the output's own coordinates; every rect
        // the compositor works in is global. Without this offset a bar on the
        // second monitor reserves its strip from the *first* one, which looks
        // like the exclusive zone being applied to the wrong screen because it
        // is.
        let geometry = self.space.output_geometry(output)?;
        let mut area = layer::work_area(output);
        area.loc += geometry.loc;
        Some(area)
    }

    /// The area a rectangle's own monitor offers it.
    pub(crate) fn work_area_of(
        &self,
        rect: Rectangle<i32, Logical>,
    ) -> Option<Rectangle<i32, Logical>> {
        self.work_area_on(&self.output_of(rect)?)
    }

    /// Put every mapped output where the arrangement says.
    ///
    /// Called when an output appears or goes away and when the configuration is
    /// read again, and it is the only place an output's position is decided.
    /// Re-running it with nothing changed is harmless and cheap, which is what
    /// makes it safe to call from a reload.
    pub(crate) fn place_outputs(&mut self) {
        // Scale first, because it decides each monitor's *logical* size and
        // the positions are laid out in logical space.
        //
        // Here and not where the outputs are created, which is where it was:
        // the nested backend loads its scripts after making its outputs, so
        // the arrangement was empty and every scale read as automatic. Doing
        // it in the one place both backends already call also means
        // `super+shift+r` can change a scale without ending the session.
        self.scale_outputs();

        let monitors: Vec<_> = self
            .space
            .outputs()
            .map(|output| {
                let size = output
                    .current_mode()
                    .map(|mode| mode.size.to_logical(1))
                    .unwrap_or_default();
                (output.name(), size)
            })
            .collect();
        if monitors.is_empty() {
            return;
        }

        for name in self.arrangement.unmatched(&monitors) {
            // Warned rather than ignored: a connector name that does not exist
            // on this machine is the usual reason a monitor configuration
            // appears to do nothing at all, and it is invisible otherwise.
            tracing::warn!(
                monitor = name,
                "no connector by that name -- run `solium --probe` for the ones this machine has"
            );
        }

        let layout = self.arrangement.place(&monitors);
        for name in &layout.unresolved {
            // Named a neighbour that is not here, or two monitors named each
            // other. Placed to the right of everything rather than dropped,
            // and said out loud: the position it ends up at is the one thing
            // that will not look like the configuration was read.
            tracing::warn!(
                monitor = name,
                "could not be placed beside what it names -- put it at the right-hand end"
            );
        }
        let outputs: Vec<Output> = self.space.outputs().cloned().collect();
        for (output, at) in outputs.iter().zip(layout.at) {
            // Only when it actually moved. Remapping an output resets its
            // damage memory, so re-placing everything on every reload would
            // throw away damage tracking to achieve nothing -- and would log a
            // line per monitor per reload, which is how a log stops being read.
            if self
                .space
                .output_geometry(output)
                .map(|geometry| geometry.loc)
                == Some(at)
            {
                continue;
            }
            self.space.map_output(output, at);
            tracing::info!(monitor = output.name(), x = at.x, y = at.y, "placed");
        }
        self.arrange_layers();
    }

    /// Give every monitor the scale it asked for, or the one its size implies.
    fn scale_outputs(&mut self) {
        for output in self.space.outputs().cloned().collect::<Vec<_>>() {
            let name = output.name();
            let physical = output.physical_properties().size;
            let Some(mode) = output.current_mode() else {
                continue;
            };
            let scale = match self.arrangement.scale(&name) {
                monitor::Scaling::Fixed(scale) => scale,
                // A window has no physical size, so a nested output reports
                // 0x0 and lands on 1x: a scale there has to be asked for.
                monitor::Scaling::Auto => monitor::automatic(physical, mode.size),
            };
            let current = output.current_scale().fractional_scale();
            if (current - scale).abs() < f64::EPSILON {
                continue;
            }
            if physical.w > 0 {
                #[expect(clippy::cast_possible_truncation, reason = "reported, not measured")]
                let dpi = (f64::from(mode.size.w) / (f64::from(physical.w) / 25.4)).round() as i32;
                tracing::info!(monitor = name, dpi, scale, "scale");
            } else {
                tracing::info!(monitor = name, scale, "scale");
            }
            output.change_current_state(None, None, Some(Scale::Fractional(scale)), None);

            // `wl_surface.preferred_buffer_scale` reaches an existing client
            // for free on its next commit (`commit`, below, sends it on every
            // one). `wp_fractional_scale_v1` does not: `new_fractional_scale`
            // answers it once, when a client first asks, and nothing calls it
            // again on its own. Without this, a window opened before a
            // `super+shift+r` rescale keeps drawing at the scale it had at
            // startup, upscaled by the compositor -- issue #99. Only reached
            // when the scale actually changed, by the `continue` above.
            self.resend_fractional_scale(&output);
        }
    }

    /// Re-tell every surface on `output` the fractional scale it should draw
    /// at, once `scale_outputs` has actually changed that output's scale.
    ///
    /// Per window, not per output: `fractional_scale_for` reads each
    /// surface's *own* output rather than being handed this one, so a window
    /// on some other monitor is never touched even though this function only
    /// runs for the monitor that changed, and one straddling two monitors at
    /// different scales is told the one it actually reads its scale from.
    ///
    /// Walks every window's full surface tree -- subsurfaces and popups, not
    /// just the toplevel -- the same way `send_frame` and
    /// `take_presentation_feedback` already do elsewhere in this file.
    ///
    /// Writes through the `SurfaceData` `with_surfaces` already hands its
    /// callback, rather than looking it up again with `with_states`: that
    /// lookup takes the same per-surface lock `with_surfaces` is already
    /// holding while it calls this closure, and a second, nested attempt on
    /// it from the same thread is a self-deadlock, not a wait -- found by
    /// this function's own test hanging instead of failing.
    fn resend_fractional_scale(&self, output: &Output) {
        for window in self.space.elements_for_output(output) {
            window.with_surfaces(|surface, states| {
                let scale = self.fractional_scale_for(surface);
                with_fractional_scale(states, |fractional| {
                    fractional.set_preferred_scale(scale);
                });
            });
        }
    }

    /// The fractional scale a surface should draw itself at: its own
    /// window's own output, or the active one for a surface not placed yet.
    ///
    /// Shared by `new_fractional_scale`, which answers a client's first ask,
    /// and `resend_fractional_scale`, which repeats the answer when an
    /// output's scale changes after that -- one copy of "what scale is this
    /// surface drawn at" rather than two that can drift apart. Deliberately
    /// just the computation: how the answer gets written back to the surface
    /// differs between the two callers, and that part is not shared -- see
    /// `resend_fractional_scale`'s own doc comment for why.
    fn fractional_scale_for(&self, surface: &WlSurface) -> f64 {
        self.window_for(surface)
            .and_then(|window| self.space.outputs_for_element(&window).first().cloned())
            .or_else(|| self.active_output())
            .map_or(1.0, |output| output.current_scale().fractional_scale())
    }

    /// Arrange anchored surfaces on every monitor.
    ///
    /// Returns whether anything moved, because a changed exclusive zone changes
    /// a work area and the windows placed against the old one are now wrong.
    pub(crate) fn arrange_layers(&mut self) -> bool {
        let outputs: Vec<Output> = self.space.outputs().cloned().collect();
        // A plain loop rather than `any`, deliberately: `any` short-circuits,
        // and the first output reporting a change would leave every one after
        // it unarranged — a bar on the second monitor placed against nothing.
        let mut moved = false;
        for output in &outputs {
            if layer::arrange(output) {
                moved = true;
            }
        }
        moved
    }

    /// A window's application id, as the client set it.
    ///
    /// The shell tells its own surfaces from application windows by this, so
    /// an empty answer is better than a wrong one.
    pub(crate) fn window_app_id(&self, window: &Window) -> String {
        window
            .toplevel()
            .map(ToplevelSurface::wl_surface)
            .and_then(|surface| {
                with_states(surface, |states| {
                    states
                        .data_map
                        .get::<XdgToplevelSurfaceData>()
                        .and_then(|data| data.lock().ok())
                        .and_then(|attributes| attributes.app_id.clone())
                })
            })
            .unwrap_or_default()
    }

    /// A window's title, as the client set it.
    pub(crate) fn window_title(&self, window: &Window) -> String {
        window
            .toplevel()
            .map(ToplevelSurface::wl_surface)
            .and_then(|surface| {
                with_states(surface, |states| {
                    states
                        .data_map
                        .get::<XdgToplevelSurfaceData>()
                        // A poisoned lock means another thread panicked while
                        // holding it. Showing no title beats propagating that.
                        .and_then(|data| data.lock().ok())
                        .and_then(|attributes| attributes.title.clone())
                })
            })
            .unwrap_or_default()
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

    /// A window's script-facing identity: the id of the pane it is inside.
    ///
    /// It belongs to the pane rather than to the surface, which is what lets it
    /// exist before the surface does — a script told about a window while its
    /// application was still starting is still talking about the same window
    /// once the application arrives, because nothing was replaced.
    ///
    /// Zero means a window the compositor is not tracking. Every window it maps
    /// gets a pane on the same line, so in practice this is a window Smithay
    /// put in the space behind our back, and a script can do nothing with it
    /// anyway.
    pub(crate) fn window_id(&self, window: &Window) -> u64 {
        self.panes.id_of(window).map_or(0, crate::pane::PaneId::get)
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
    /// Returns whether it went anywhere, so an unrecognised token can fall
    /// through to being an ordinary request for focus.
    fn claim_into(&mut self, pane: crate::pane::PaneId, surface: &WlSurface) -> bool {
        // Still waiting, or already given up on.
        if !self.panes.get(pane).is_some_and(Pane::is_loading) {
            return false;
        }
        let Some(window) = self.window_for(surface) else {
            return false;
        };
        let Some(wrong) = self.panes.id_of(&window) else {
            return false;
        };
        if wrong == pane {
            return true;
        }

        if let Some(held) = self.panes.get_mut(pane) {
            held.adopt(window.clone());
        }
        // The pane it opened in goes, and with it the frame and the id nothing
        // should have learned. Retired rather than left empty: `sync_panes`
        // would drop it anyway, and the layout is told now rather than a frame
        // late.
        self.panes.remove(wrong);
        self.trigger_close(wrong);
        tracing::debug!(
            pane = pane.get(),
            was = wrong.get(),
            "an application arrived in its window, by token"
        );
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

    /// What the compositor looks like right now, as a script sees it.
    ///
    /// Built fresh per dispatch and handed over by value: a script holding a
    /// stale view of the windows is the mirror-of-state bug that cost this
    /// project a week in its previous life.
    pub(crate) fn snapshot(&self) -> Snapshot {
        let now = self.clock.now();
        let focused = self.focused_window();
        let cursor = self
            .seat
            .get_pointer()
            .map(|pointer| pointer.current_location())
            .unwrap_or_default();

        // Topmost first, which is the order a hit test wants.
        //
        // Built from panes, not from the space: this is the list scripts place,
        // so a window that exists but has no client yet has to be in it or the
        // layout will never give it anywhere to be.
        let windows = self
            .panes
            .iter()
            .rev()
            .filter_map(|pane| {
                // A pane whose application has not arrived is in this list --
                // that is what makes the layout reserve its place before there
                // is anything to put in it. Unless it was asked not to: a
                // window that takes no slot until it is really there is a
                // setting, because which of the two reads better is taste.
                if pane.client().is_none() && !self.loading.reserves_a_slot {
                    return None;
                }
                // A menu, a tooltip, a drag icon. On screen and under the
                // pointer, but not a window: a layout given one reserves a
                // slot for it and reflows the desktop around something that
                // will be gone in a moment.
                if !pane.managed() {
                    return None;
                }
                let outer = self.pane_outer(pane);
                let drawn = self.drawn_at(pane, outer, now);
                Some(WindowInfo {
                    id: pane.id().get(),
                    rect: to_rect(outer),
                    drawn: Rect {
                        x: drawn.rect.loc.x,
                        y: drawn.rect.loc.y,
                        w: drawn.rect.size.w,
                        h: drawn.rect.size.h,
                    },
                    // What the user asked for, until the client has an opinion.
                    title: pane.client().map_or_else(
                        || pane.program().unwrap_or_default().to_owned(),
                        |window| self.window_title(window),
                    ),
                    focused: pane.client().is_some() && focused.as_ref() == pane.client(),
                    monitor: self
                        .output_of(outer)
                        .map(|output| output.name())
                        .unwrap_or_default(),
                    // A pane with no client yet is a reserved slot, and a
                    // reserved slot has no client to have said either of these
                    // things -- so it is an ordinary window until one arrives,
                    // and the `modal_changed` that arrives with it re-runs the
                    // layout.
                    modal: pane.client().is_some_and(|window| self.is_modal(window)),
                    parent: pane
                        .client()
                        .map_or(Parentage::None, |window| self.parent_of(window)),
                })
            })
            .collect();

        let active = self.active_output();
        let primary = self.primary_output();
        let monitors = self
            .space
            .outputs()
            .map(|output| crate::script::MonitorInfo {
                name: output.name(),
                area: self.work_area_on(output).map(to_rect).unwrap_or_default(),
                whole: self
                    .space
                    .output_geometry(output)
                    .map(to_rect)
                    .unwrap_or_default(),
                scale: output.current_scale().fractional_scale(),
                focused: active.as_ref() == Some(output),
                primary: primary.as_ref() == Some(output),
                transform: format!("{:?}", output.current_transform()).to_lowercase(),
            })
            .collect();

        Snapshot {
            windows,
            monitors,
            keyboard: self.keyboard.clone(),
            work_area: self.work_area().map(to_rect).unwrap_or_default(),
            cursor: (cursor.x, cursor.y),
        }
    }

    /// Whether a layout should float this window over the one waiting on it.
    ///
    /// Two protocols, one question, and they are not symmetrical: Wayland has a
    /// flag that means exactly this, and X11 does not, so the X11 side reads
    /// the window type instead. The whole of that argument is in
    /// `xwayland::floats_over_its_parent`; what is here is only the lookup.
    fn is_modal(&self, window: &Window) -> bool {
        if let Some(toplevel) = window.toplevel() {
            return with_states(toplevel.wl_surface(), |states| {
                states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    // A poisoned lock is a panic somewhere else in this
                    // process, and the honest answer to "is this modal" at that
                    // point is "no" -- an ordinary window, laid out the
                    // ordinary way. `unwrap` here would turn one panic into
                    // two, in a compositor with nothing to restart it.
                    .and_then(|data| data.lock().ok())
                    .is_some_and(|attributes| attributes.modal)
            });
        }
        window
            .x11_surface()
            .is_some_and(|surface| crate::xwayland::floats_over_its_parent(surface.window_type()))
    }

    /// A parent that was found, as a script sees it.
    ///
    /// `Unknown` for a pane that is on its way out, which is not a special case
    /// so much as the honest reading of it: [`Self::trigger_close`] runs while
    /// the dying pane is still in `panes`, because a script has to be able to
    /// ask which window it was. Answer `Window` there and the dialog it was
    /// waiting on is centred on the rect of a window that is going away — and
    /// the close pass is the last one, so nothing runs again to move it off.
    /// "Named, and cannot be pointed at" is exactly what `Unknown` means.
    fn parented(&self, pane: crate::pane::PaneId) -> Parentage {
        if self.closing == Some(pane) {
            return Parentage::Unknown;
        }
        Parentage::Window(pane.get())
    }

    /// Which window this one belongs to, as far as this compositor can tell.
    ///
    /// The distinction [`Parentage`] exists for is made here and only here: a
    /// parent that was named and cannot be found is `Unknown`, and a parent
    /// that was never named is `None`. Both end up as "no rect to centre on" in
    /// a layout, but only one of them means something has gone missing.
    fn parent_of(&self, window: &Window) -> Parentage {
        if let Some(toplevel) = window.toplevel() {
            let Some(parent) = toplevel.parent() else {
                return Parentage::None;
            };
            return self
                .window_for(&parent)
                .and_then(|window| self.panes.id_of(&window))
                .map_or(Parentage::Unknown, |pane| self.parented(pane));
        }

        let Some(surface) = window.x11_surface() else {
            return Parentage::None;
        };
        // `WM_TRANSIENT_FOR`, which smithay reads at `CreateNotify` and again
        // on every property change. It holds an X11 window id rather than a
        // surface, so the match is against the id side -- and a client that
        // points it at the root window, which is a common way of saying "I am
        // transient for the session", names an id no element here has and comes
        // out `Unknown`. That is the right answer: there is no window to centre
        // on.
        let Some(parent) = surface.is_transient_for() else {
            return Parentage::None;
        };
        self.space
            .elements()
            .find(|element| {
                element
                    .x11_surface()
                    .is_some_and(|surface| surface.window_id() == parent)
            })
            .and_then(|element| self.panes.id_of(element))
            .map_or(Parentage::Unknown, |pane| self.parented(pane))
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
                    let target = Frame {
                        matrix: matrix.unwrap_or(crate::mat4::Mat4::IDENTITY),
                        rect: rect.map_or_else(
                            || outer.to_f64(),
                            |rect| present::logical((rect.x, rect.y), (rect.w, rect.h)),
                        ),
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
                    let start = Frame {
                        matrix: crate::mat4::Mat4::IDENTITY,
                        rect: present::logical((rect.x, rect.y), (rect.w, rect.h)),
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
            self.panes
                .iter()
                .filter(|pane| pane.managed())
                .map(|pane| self.drawn_at(pane, self.pane_outer(pane), landed).rect),
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

        // `rev` because `panes` is in stacking order, bottom-first, and the
        // rule is topmost-first -- which is now load-bearing in a way it was
        // not before, since the first pane to cover the point ends the walk,
        // and a halo is only kept until a lower pane is found drawing under it.
        topmost_chrome(
            self.panes
                .iter()
                .rev()
                .map(|pane| self.pane_chrome(pane, location, now)),
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
    ) -> PaneHit<Under> {
        let outer = self.pane_outer(pane);
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
            // `false`: laying out must not restack. A tiling arrangement that
            // reordered windows every time it ran would fight the user's focus.
            self.map_stacked(window, client.loc, false);
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
                Standing::Kept => {
                    if held.placed().is_some() {
                        held.set_placed(outer);
                    }
                }
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
        // The cost is that a closing window can overlap the one moving into its
        // space for up to 190ms. It is shrinking and fading throughout, so what
        // that reads as is the window dissolving with its replacement arriving
        // underneath — against a full-opacity snap followed by nothing, which
        // is what shipped.
        //
        // **"Underneath" is not guaranteed, and the decision is to accept
        // that** (#127's third review, finding 4). `map_stacked` above is let
        // through for a leaving pane, and `Space::map_element` removes and
        // re-inserts on top whatever the `false` says — that flag decides only
        // who is told they are focused, which `map_stacked`'s own doc is where
        // this is written down. So a sweep that touches the dying pane after it
        // touches the replacement draws the dying one above.
        //
        // Three things make that the cheaper side, and they are worth stating
        // because the paragraph above reads like a promise otherwise.
        //
        // * **The z-order after any sweep is the sweep's order, for every pane
        //   it moves and not just this one.** Pinning the leaving pane alone
        //   would be a guarantee standing on a background that offers none, and
        //   the honest fix — deciding the whole stack a layout sweep produces —
        //   is a change about stacking rather than about closing.
        // * **It costs pixels and not input.** The dying pane is at opacity
        //   zero from the end of `CLOSING` onwards, and `Frame::covers` gates
        //   every hit test on `shows()` — so a pane drawn on top owns no pixel
        //   the moment it stops being visible, and while it *is* visible
        //   keeping its clicks is that predicate's whole argument.
        // * **It is bounded by the fade.** 190ms of a window that is shrinking
        //   and going transparent, over one that is arriving. Restacking it
        //   mid-close would itself be a visible reorder on the one path with
        //   the least to gain from one.
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
        // Note what needs no guard. `present::rebase` — the group path — is
        // safe for a leaving pane by construction: it preserves both the
        // destination and the release flag, so a closing transform rebased by a
        // workspace slide is still a closing transform. It is this function's
        // unconditional `from` that was the exception.
        if let Some(held) = self.panes.get(pane).filter(|_| !leaving) {
            present::from(
                held,
                outer,
                present::Frame::real(was),
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
        let token = {
            let data = XdgActivationTokenData::default();
            data.user_data.insert_if_missing(|| LaunchedFor(pane));
            let (token, _) = self.activation_state.create_external_token(data);
            token.as_str().to_owned()
        };
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
                // otherwise. The layout is told, and closes the gap.
                if self.panes.remove(pane) {
                    self.trigger_close(pane);
                }
                self.redraw = true;
                tracing::warn!(?err, program, "could not spawn");
            }
        }
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
        for pane in self.panes.iter().rev() {
            let outer = self.pane_outer(pane);
            // `covers`, not `rect.contains`: a pane drawn at opacity zero is
            // not on screen and owns no pixel, however solid the rectangle it
            // would be drawn at. See [`present::Frame::covers`], and #127's
            // review finding 1 -- this walk ends in `focus_window` through
            // click-to-focus, so an invisible pane winning it took the
            // keyboard as well as the click.
            if !self.drawn_at(pane, outer, now).covers(location) {
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

        for pane in self.panes.iter().rev() {
            let outer = self.pane_outer(pane);
            let frame = self.drawn_at(pane, outer, now);
            // Invisible is not covered. Same rule and same reason as
            // [`Self::window_under`]: this walk is what delivers motion,
            // buttons and — through the focus a press sets — keystrokes, so a
            // pane held at opacity zero across a close winning it is where the
            // typing went. [`present::Frame::covers`] argues the predicate.
            if !frame.covers(location) {
                continue;
            }
            // A window whose application has not arrived has no surface to
            // give the pointer -- but it is on screen and it is under the
            // cursor, so nothing behind it may have the click either. Falling
            // through would type into whatever the window is covering.
            let window = pane.client()?;

            // Mapped through the *outer* rect, then offset into the client's
            // own space. A point in the titlebar lands above the client and
            // finds no surface, which is what should happen: the frame is the
            // compositor's, not the client's.
            let insets = self.frame_insets(window);
            let inset: Point<f64, Logical> = (f64::from(insets.left), f64::from(insets.top)).into();
            let in_outer = present::to_window_space(frame, outer, location);
            let in_window = in_outer - outer.loc.to_f64() - inset;

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

            if let Some((surface, surface_offset)) =
                window.surface_under(in_buffer, WindowSurfaceType::ALL)
            {
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
    /// binding, a script. A client that exits on its own still vanishes
    /// instantly: by the time we hear about it its surface is gone, and
    /// animating it would mean holding a snapshot of every window on the
    /// chance that it might be the next to leave. That is issue #126.
    ///
    /// **Asked at most once per window.** See [`Pane::leaving`] for the three
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
        self.redraw = true;
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
                // is entirely ours to decide.
                if self.panes.remove(id) {
                    self.trigger_close(id);
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
        // And one asked to close that is still here comes back.
        animating |= self.settle_refused(now);
        self.animating = animating;
        animating
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

    /// An X11 client has copied something. Offer it to Wayland clients.
    ///
    /// Offered as the compositor's own selection rather than any client's,
    /// which is what `set_data_device_selection` is for: from a Wayland
    /// client's side there is simply a selection available in these formats,
    /// and it never learns that the thing holding it does not speak Wayland.
    pub(crate) fn take_x11_selection(&mut self, ty: SelectionTarget, mimes: Vec<String>) {
        let display = self.display_handle.clone();
        match ty {
            SelectionTarget::Clipboard => {
                set_data_device_selection(&display, &self.seat, mimes, ());
            }
            SelectionTarget::Primary => {
                set_primary_selection(&display, &self.seat, mimes, ());
            }
        }
    }

    /// The X11 client that owned a selection has let it go.
    pub(crate) fn drop_x11_selection(&mut self, ty: SelectionTarget) {
        let display = self.display_handle.clone();
        match ty {
            SelectionTarget::Clipboard => clear_data_device_selection(&display, &self.seat),
            SelectionTarget::Primary => clear_primary_selection(&display, &self.seat),
        }
    }

    /// An X11 client wants to read a selection a Wayland client owns.
    ///
    /// Asked of whichever client owns it, which writes into the descriptor X11
    /// gave us. Nothing is copied through the compositor.
    pub(crate) fn serve_x11_selection(
        &mut self,
        ty: SelectionTarget,
        mime_type: String,
        fd: std::os::fd::OwnedFd,
    ) {
        // Two calls rather than one `match` producing a result: the clipboard
        // and the primary selection fail with different error types, and
        // flattening them would mean stringifying one to match the other.
        match ty {
            SelectionTarget::Clipboard => {
                if let Err(err) = request_data_device_client_selection(&self.seat, mime_type, fd) {
                    tracing::warn!(?err, "no Wayland client would serve the clipboard to X11");
                }
            }
            SelectionTarget::Primary => {
                if let Err(err) = request_primary_client_selection(&self.seat, mime_type, fd) {
                    tracing::warn!(?err, "no Wayland client would serve the primary to X11");
                }
            }
        }
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
        let Some(pane) = self.panes.get(id) else {
            // No pane, nothing to give back and nothing left waiting: a window
            // that went while this ran answered the close after all.
            return true;
        };
        let outer = self.pane_outer(pane);
        if !present::clear(
            pane,
            outer,
            now,
            std::time::Duration::from_millis(150),
            solium_animation::Curve::OutCubic,
        ) {
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

    /// Where a window drawn at `outer` goes if it is on no screen at all:
    /// onto the nearest one, keeping its size where that fits.
    ///
    /// `None` when any of `outer` is on a screen already -- one hanging half
    /// off an edge is a normal thing to have arranged on purpose -- and when
    /// there is no screen to put it on. With no screens every window is off
    /// every screen, and leaving it where it was means it is still there when
    /// a monitor comes back, which is the best available answer.
    fn rescued(&self, outer: Rectangle<i32, Logical>) -> Option<Rectangle<i32, Logical>> {
        if self.on_any_output(outer) {
            return None;
        }
        let centre = (
            f64::from(outer.loc.x) + f64::from(outer.size.w) / 2.0,
            f64::from(outer.loc.y) + f64::from(outer.size.h) / 2.0,
        );
        let screen = monitor::nearest(&self.space, centre.into())
            .and_then(|output| self.space.output_geometry(&output))?;
        // Onto the nearest screen, keeping its size, clamped so the whole
        // window is on it when it fits. Not centred: a window that was in the
        // top-left of the monitor that went should still feel like the window
        // that was in the top-left.
        let size = (
            outer.size.w.min(screen.size.w),
            outer.size.h.min(screen.size.h),
        );
        let x = outer
            .loc
            .x
            .clamp(screen.loc.x, screen.loc.x + screen.size.w - size.0);
        let y = outer
            .loc
            .y
            .clamp(screen.loc.y, screen.loc.y + screen.size.h - size.1);
        Some(Rectangle::new((x, y).into(), (size.0, size.1).into()))
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
        self.panes.iter().rev().find_map(|pane| {
            // Only a built frame is listening. There is no scene to tell about
            // the pointer until there is one.
            pane.decoration()?;
            let outer = self.pane_outer(pane);
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
            .filter(|pane| pane.expired(now, patience))
            .map(|pane| (pane.id(), pane.program().unwrap_or_default().to_owned()))
            .collect();
        if gone.is_empty() {
            return false;
        }
        for (id, program) in gone {
            tracing::info!(program, "gave up on an application that never arrived");
            // Forgotten first, then reported: a layout hearing that a window
            // closed will lay out immediately, and it should not be laying out
            // around a window that is already gone. Its frame and its timers
            // go here with it -- fields of the pane rather than entries in a
            // table waiting for the next `sync_panes` to sweep them.
            self.panes.remove(id);
            self.trigger_close(id);
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
                        let drawn = self.drawn_at(pane, self.pane_outer(pane), now).rect;
                        self.space.outputs().any(|output| {
                            self.space
                                .output_geometry(output)
                                .is_some_and(|geometry| geometry.to_f64().overlaps(drawn))
                        })
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
        if !self.trigger_open(pane)
            && let Some(outer) = self.outer_geometry(window)
            && let Some(pane) = self.panes.get(pane)
        {
            present::open(pane, outer, self.clock.now());
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

    /// Tell the shell what windows exist.
    ///
    /// Sent when the list changes rather than every frame: the shell rebinds
    /// on it, and a bar that re-evaluates sixty times a second because nothing
    /// happened is a bar that costs something to look at.
    pub(crate) fn publish_windows(&mut self) {
        // Nobody to tell, nothing to say. The window list is serialised for the
        // shell, and building it walks every window, asks each for its title
        // and app id, and allocates a string per window -- every frame, once
        // something is animating. With no shell hosted that is pure waste, and
        // the ordinary case is no shell hosted.
        // Only when a foreign shell is hosted. The list is for the Quickshell
        // compatibility layer -- `ToplevelManager.toplevels` and friends -- and
        // building it walks every window, asks each for its title and app id,
        // and allocates a string per window, every time anything changes.
        //
        // This used to test whether the in-process shell existed, which stopped
        // meaning anything the moment the shell became an ordinary scripted
        // surface: a wallpaper is one of those, and there is always a
        // wallpaper.
        if std::env::var_os("SOLIUM_SHELL_SCENE").is_none() {
            return;
        }
        let focused = self.focused_window();
        let mut windows = String::from("{\"windows\":[");
        let mut active = String::from("null");
        for (index, pane) in self.panes.iter().rev().enumerate() {
            let Some(window) = pane.client() else {
                continue;
            };
            let id = pane.id().get();
            let title = self.window_title(window).replace('"', "'");
            let app_id = self.window_app_id(window).replace('"', "'");
            let is_active = focused.as_ref() == Some(window);
            let entry = format!(
                "{{\"id\":{id},\"title\":\"{title}\",\"appId\":\"{app_id}\",\"activated\":{is_active}}}"
            );
            if index > 0 {
                windows.push(',');
            }
            windows.push_str(&entry);
            if is_active {
                active = entry;
            }
        }
        windows.push_str("],\"active\":");
        windows.push_str(&active);
        windows.push('}');

        if windows != self.published_windows {
            crate::qml::set_windows(&windows);
            self.published_windows = windows;
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

    /// Tell scripts a window has gone, so a layout can forget it.
    /// Tell the layout the space windows get has changed.
    /// Tell the scripts the set of monitors is not what it was.
    ///
    /// Fired before the relayout rather than instead of it: re-homing the
    /// windows and arranging them are two steps, and a mode that does the
    /// first is still expecting the second.
    /// Everything a change in the set of monitors has to do, in one call.
    ///
    /// Both backends call this and nothing else, so the nested one and the
    /// hardware one cannot drift -- which matters more here than usual,
    /// because the hardware path needs a cable to exercise and the nested one
    /// does not.
    /// The screens are known: tell the scripts, once, at startup.
    ///
    /// Scripts load before the monitors exist -- on the hardware backend they
    /// load before the GPU is even opened -- so a script that computes where
    /// to put something computes it against nothing. The Developer Tweaks
    /// panel did exactly that and came out 200x0 pixels, which is a panel that
    /// is there and invisible.
    ///
    /// The same call a hotplug makes, deliberately: "the monitors are not what
    /// you last knew" covers both, and having one event rather than a
    /// `startup` and a `changed` means a script cannot handle one and forget
    /// the other.
    pub(crate) fn monitors_ready(&mut self) {
        self.settle_monitors();
    }

    pub(crate) fn settle_monitors(&mut self) {
        self.place_outputs();
        self.prune_surfaces();
        self.rescue_offscreen();
        self.trigger_monitors_changed();
        self.trigger_relayout();
        self.redraw = true;
    }

    /// Bring back any window that is no longer on any screen.
    ///
    /// This is the compositor's job and not a layout's, which took two
    /// hardware reports and a nested reproduction to establish. The obvious
    /// place for it is the layout -- a monitor went, so re-run the layout and
    /// it will put everything somewhere -- and that is wrong twice over. A
    /// tiling layout only walks the monitors that *exist*, so a window in a
    /// departed monitor's tree is in a tree nothing iterates. And the default
    /// mode is floating, where no layout runs at all: `tiling.active` and
    /// `scrolling.active` both start false, so on a stock configuration there
    /// is nobody to ask.
    ///
    /// A window nobody can reach is not a layout preference, it is a window
    /// the user has lost. So it is an invariant the compositor keeps, and a
    /// script is free to move it again afterwards -- `trigger_relayout` runs
    /// straight after this.
    ///
    /// Only windows that are *entirely* off every screen are touched. One
    /// hanging half off an edge is a normal thing to have arranged on purpose.
    fn rescue_offscreen(&mut self) {
        // Which windows are stranded and where each goes is `rescued`'s,
        // shared with the maximise and fullscreen way back so the two cannot
        // disagree about where a window on no screen belongs.
        let stranded: Vec<_> = self
            .panes
            .iter()
            .filter_map(|pane| {
                let outer = self.pane_outer(pane);
                Some((pane.id(), outer, self.rescued(outer)?))
            })
            .collect();

        for (pane, outer, moved) in stranded {
            // Through the same move every layout uses. Setting the slot alone
            // looks like it works and does not: the space still holds the old
            // position and writes it back the next frame.
            //
            // Animated from where it was, which is off screen -- so it flies
            // in from the edge the monitor was on rather than appearing. That
            // is worth the two lines: a window that teleports is one the user
            // has to find again.
            //
            // `Kept`, because a rescue is not a layout's opinion: a tiled pane
            // is still tiled, at the rectangle it was brought back to, and a
            // floating one must not become held inside a tile nobody gave it.
            let now = self.clock.now();
            self.move_pane(
                pane,
                moved,
                outer,
                AnimationSpec::default(),
                now,
                Standing::Kept,
            );
            tracing::info!(
                from = ?outer.loc,
                to = ?moved.loc,
                "a window was left on no screen and has been brought back"
            );
        }
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
        // Only for the snapshot, and the snapshot is the whole of what a script
        // sees. The pane is still here -- it is retired in `sync_panes`, a frame
        // from now -- so a dialog waiting on this window would otherwise be
        // re-centred on the rect of the window that is leaving, and this pass is
        // the last one: nothing runs again to take it off the window that moves
        // into that space. See [`Self::parented`].
        self.closing = Some(pane);
        let snapshot = self.snapshot();
        self.closing = None;
        let Some(mut scripts) = self.scripts.take() else {
            return;
        };
        let outcome = scripts.closed(id, snapshot);
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

    fn trigger_open(&mut self, pane: crate::pane::PaneId) -> bool {
        let id = pane.get();
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return false;
        };
        let outcome = scripts.opened(id, snapshot);
        self.scripts = Some(scripts);

        let handled = outcome.handled && !outcome.commands.is_empty();
        self.apply(outcome);
        handled
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

impl CompositorHandler for Solium {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        match client.get_data::<ClientState>() {
            Some(state) => &state.compositor_state,
            // Smithay only calls this for clients we created with ClientState,
            // so this is unreachable in practice -- but panicking here would
            // take the session down, so leak a default instead.
            None => Box::leak(Box::new(CompositorClientState::default())),
        }
    }

    fn commit(&mut self, surface: &WlSurface) {
        // Imports the client's attached buffer into renderer-visible state.
        // Without it every surface is silently empty: the window maps, the
        // client draws, and the compositor renders nothing.
        on_commit_buffer_handler::<Self>(surface);

        // A client committing is the screen changing. Nothing else says so --
        // and on the hardware, where drawing waits to be asked, nothing else
        // was asking: a terminal's own output only reached the screen when
        // some unrelated thing happened to want a frame. Which frame it is
        // and how much of it changed are the damage tracker's business; that
        // it changed at all is this.
        self.redraw = true;

        // What scale and rotation a surface should draw itself at, for clients
        // that never bind `wp_fractional_scale_v1`. That protocol is answered
        // too — see `new_fractional_scale` — but it is the newer one, and a
        // client that only knows `wl_surface.preferred_buffer_scale` would
        // otherwise draw at 1x on a 2x screen and be scaled up.
        if let Some(output) = self.output_for_surface(surface) {
            let scale = output.current_scale().integer_scale();
            let transform = output.current_transform();
            with_states(surface, |states| {
                smithay::wayland::compositor::send_surface_state(surface, states, scale, transform);
            });
        }

        // Sub-surfaces commit through their root; only the root needs handling.
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self.window_for(&root) {
                window.on_commit();
                self.show_if_new(&window);
            }
        }
        self.popups.commit(surface);
        // After `popups.commit`, which is what moves a popup from the unmapped
        // list into its tree on its first commit. Ordering is not load-bearing
        // — `find_popup` searches both lists — but the configure answers the
        // commit that has just been applied, so it reads in the right order.
        self.configure_popup(surface);
        self.configure_layer(surface);
    }

    /// A surface is going. Only the lock screen's need anything doing: see
    /// `Solium::lock_surface_destroyed`.
    fn destroyed(&mut self, surface: &WlSurface) {
        self.lock_surface_destroyed(surface);
    }
}

impl Solium {
    /// Send a layer surface its first configure, so it can draw.
    ///
    /// The protocol says the initial configure goes out in response to the
    /// surface's first commit, and Smithay is deliberate about not sending it
    /// from `arrange` — a client is allowed to set its size *before*
    /// committing, and a configure sent earlier would carry the wrong one.
    /// That leaves it to the compositor, and nothing here was doing it.
    ///
    /// So a bar mapped, took its exclusive zone, and was never told what size
    /// to be — and a client may not attach a buffer until it has been
    /// configured once. Every layer surface was invisible, which means the
    /// claim in `layer.rs` that any existing panel works was untrue for the
    /// whole time it has been written down. Found by `wl-probe` anchoring one
    /// and waiting, which is the entire reason that program exists.
    fn configure_layer(&mut self, surface: &WlSurface) {
        let outputs: Vec<Output> = self.space.outputs().cloned().collect();
        for output in outputs {
            let map = layer_map_for_output(&output);
            let Some(layer) = map
                .layers()
                .find(|layer| layer.layer_surface().wl_surface() == surface)
                .cloned()
            else {
                continue;
            };
            // The map is dropped before arranging: `arrange` takes it again,
            // and the lock is not reentrant.
            drop(map);
            let sent = with_states(surface, |states| {
                states
                    .data_map
                    .get::<LayerSurfaceData>()
                    .and_then(|data| data.lock().ok())
                    .is_some_and(|attributes| attributes.initial_configure_sent)
            });
            if !sent {
                // Arranged first, so the size it is told is the one it will
                // actually be given rather than a guess to be corrected.
                layer::arrange(&output);
                layer.layer_surface().send_configure();
                self.relayout_for_layers();
            }
            return;
        }
    }

    /// A layer surface changed the room windows get, so the layout is re-run.
    fn relayout_for_layers(&mut self) {
        self.trigger_relayout();
        self.redraw = true;
    }
}

impl BufferHandler for Solium {
    fn buffer_destroyed(
        &mut self,
        _buffer: &smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer,
    ) {
    }
}

impl ShmHandler for Solium {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl XdgShellHandler for Solium {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: smithay::wayland::shell::xdg::ToplevelSurface) {
        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Activated);
        });
        // A client may not attach a buffer until it has been configured once.
        surface.send_configure();

        let window = Window::new_wayland_window(surface.clone());
        self.map_stacked(window.clone(), (0, 0), true);
        // On the same line as the map, so nothing can observe a mapped window
        // that has no pane -- `trigger_open` is about to ask for its id.
        self.adopt_or_open(window);

        // Focus follows the newest window. #12 turns this into a policy.
        //
        // Through the gate, which refuses it while locked: this is a client
        // opening a window of its own accord, with nobody at the machine, and
        // until the gate existed it was the shortest way to the password.
        self.give_keyboard(
            Some(surface.wl_surface().clone()),
            SERIAL_COUNTER.next_serial(),
        );
    }

    /// `xdg_toplevel.set_parent` — a window saying which window it belongs to.
    ///
    /// Re-run the layout, for the same reason `modal_changed` does: a modal
    /// dialog is centred on its parent, so the answer to "where does it go"
    /// just changed. It matters more than it looks, because the order is not
    /// the one you would guess. GTK4 creates the toplevel, maps it, and calls
    /// `set_parent` and `set_modal` in whichever order the widget tree settles
    /// in -- so a dialog can easily be laid out once while its parent is still
    /// `Parentage::None`, land in the middle of the screen, and never move
    /// again. Without this, that is the last word.
    ///
    /// Cheap enough not to need a guard: the layout runs off a snapshot, and a
    /// window whose place has not changed is placed where it already is.
    fn parent_changed(&mut self, surface: ToplevelSurface) {
        // **The moment a client says which window its new one is about**, which
        // is the evidence `Solium::refused_with_a_dialog` acts on. Hooked here
        // rather than at the child's first commit because this fires whichever
        // order the client chooses: a toolkit that calls `set_parent` during
        // window setup and one that calls it after mapping both arrive here,
        // and only one of them has committed a buffer by now.
        if let Some(window) = self.window_for(surface.wl_surface()) {
            self.refused_with_a_dialog(&window);
        }
        self.trigger_relayout();
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        // Told before the window is forgotten, so a script can still ask which
        // one it was. Bound before the call, so the borrow of `space` ends
        // here rather than lasting across it.
        let going = self
            .space
            .elements()
            .find(|window| window.toplevel().is_some_and(|top| *top == surface))
            .cloned();
        if let Some(pane) = going.as_ref().and_then(|window| self.panes.id_of(window)) {
            self.trigger_close(pane);
        }

        // Nothing to tidy up here. The frame and the pending close belong to
        // the pane, and the pane is retired in `sync_panes` -- which is also
        // the only notice we get for a client that died without destroying
        // anything, so the tidying has to live there or happen twice.
    }

    /// A menu has gone. If it was the last of the chain `popup_grab` holds,
    /// that grab is over and is let go of here.
    ///
    /// Nothing else ever cleared it. A menu that closed normally stayed
    /// recorded as "the chain holding the seat's grabs", keeping its window's
    /// surface with it, until the next menu replaced it or the next lock
    /// dismissed a chain that had ended long before -- which is harmless today
    /// only because dismissing an ended chain does nothing, and is not what
    /// the field says it is. `has_ended` reads the chain as live until the
    /// popup manager has tidied the dead popup out of it, which is what the
    /// `cleanup` first is for: the same call both backends make every frame.
    fn popup_destroyed(&mut self, _surface: PopupSurface) {
        self.popups.cleanup();
        if self.popup_grab.as_ref().is_some_and(PopupGrab::has_ended) {
            self.popup_grab = None;
        }
    }

    /// A client is opening a menu, a tooltip or a combo-box list.
    ///
    /// The positioner is the client's entire description of *where*: an anchor
    /// rectangle in its parent's coordinates, an edge of that rectangle to
    /// hang from, a direction to hang in, and — the part this handler exists
    /// for — the set of adjustments it permits us to make if the result would
    /// not fit on the screen. Until #100 the argument was named `_positioner`
    /// and dropped, which left Smithay's own initial geometry standing: the
    /// raw `get_geometry()` set in `xdg_surface::GetPopup`, which honours the
    /// anchor and the gravity and nothing else. A menu opened near an edge was
    /// drawn partly off the screen, and the part that was missing was the part
    /// with the entries in it.
    ///
    /// Placed before the popup is tracked so that the first geometry the popup
    /// tree — and therefore the renderer — ever reads is the constrained one;
    /// there is no frame in which the wrong position is on screen.
    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        self.place_popup(&surface, positioner);
        // Tracking failure here is not fatal: the popup simply will not be
        // positioned, which is better than ending the session.
        if let Err(err) = self.popups.track_popup(surface.into()) {
            tracing::warn!(?err, "failed to track popup");
        }
    }

    /// `xdg_popup.grab` — a client asking that a menu own input until it is
    /// dismissed.
    ///
    /// This is what makes a menu behave like a menu. The client asks for the
    /// grab; in return the compositor promises three things, none of which
    /// this did while it was an empty stub: a press anywhere outside the
    /// popup's own client dismisses the whole chain rather than reaching what
    /// it landed on, keyboard focus follows the chain so arrow keys and Escape
    /// go to the menu instead of the document behind it, and when the chain
    /// ends both are handed back to the surface the menu came from. Firefox
    /// asks for this for every context menu, so without it a menu opened and
    /// then could not be closed, dismissed or driven.
    ///
    /// **What releases it, because a grab that is never released leaves a
    /// session in which nothing can be clicked.** There are three exits and
    /// all of them are Smithay's, which is the argument for using its grabs
    /// rather than writing our own:
    ///
    /// * A press outside the grabbing client. `PopupPointerGrab::button`
    ///   compares the client of the surface under the pointer with the client
    ///   of the current grab, dismisses every popup in the chain, and calls
    ///   `handle.unset_grab`. Unsetting a pointer grab runs its `unset`, and
    ///   `PopupPointerGrab::unset` is what takes the keyboard grab off too —
    ///   so the click that closes the menu releases both devices.
    /// * The client destroying the popup. `PopupGrab::has_ended` then answers
    ///   true, and the next pointer motion or key press through either grab
    ///   unsets it. Motion arrives constantly whenever the pointer is in use,
    ///   and `popups.cleanup()` runs every frame from both backends, so this
    ///   is not a path that waits on the client for anything.
    /// * The root toplevel dying. `has_ended` covers that as well: it is
    ///   `!self.root.alive() || !self.toplevel_grab.active()`.
    ///
    /// A refused grab releases nothing because it takes nothing: every early
    /// return below happens before `set_grab` is called, except the one that
    /// has already called `ungrab` to undo what `grab_popup` recorded.
    fn grab(&mut self, surface: PopupSurface, seat: WlSeat, serial: Serial) {
        let Some(seat) = Seat::<Self>::from_resource(&seat) else {
            return;
        };
        let popup = PopupKind::Xdg(surface);

        // The root is computed here rather than left to `grab_popup`, and this
        // is not tidiness. `PopupManager::grab_popup` opens with
        // `assert_eq!(root.wl_surface(), find_popup_root_surface(&popup)?)` —
        // an assertion in a library we cannot annotate, in a compositor with
        // no supervisor to restart it. Deriving the focus we pass from the
        // same function it checks against is the only way to know the two
        // agree. A popup whose parent chain is already dead answers `Err` and
        // is refused here, before the assertion can be reached.
        //
        // Passing the root surface itself works because Solium's
        // `SeatHandler::KeyboardFocus` is a bare `WlSurface` — see the
        // `SeatHandler` impl. A compositor with a richer focus target would
        // have to look the window up; we do not, and that also means a menu
        // rooted in a layer surface (a bar's own menu) is grabbable on the
        // same path as one rooted in a window.
        let Ok(root) = find_popup_root_surface(&popup) else {
            tracing::debug!("refused a popup grab: the popup has no live root");
            return;
        };

        // A grab is the keyboard by another name -- see `grab_keyboard` in
        // `focus.rs` -- so it answers to the same rule, asked here because
        // this is before `grab_popup` has recorded anything that would then
        // have to be undone. A popup's root is a window or a layer surface,
        // never a lock surface, so while locked every grab stops here. Before
        // this line, a client that merely opened a menu behind the lock took
        // the keyboard from the lock screen and kept it.
        if !self.may_hold_keyboard(&root) {
            // And the client is told. The protocol's word for a denied grab is
            // a dismissed popup, and a menu left waiting on a grab that is not
            // coming would still be open when the session unlocked. Sent to
            // the popup directly: it has not been committed yet, so it is in
            // no tree that `PopupManager::dismiss_popup` could find it in.
            if let PopupKind::Xdg(surface) = &popup {
                surface.send_popup_done();
            }
            tracing::debug!("refused a popup grab: the session is locked");
            return;
        }

        // A stale serial is a refusal, not a crash. `grab_popup` returns
        // `Err` for a popup that is already mapped, one whose parent was
        // dismissed, and one that is not the topmost — and posts the protocol
        // error itself where the protocol calls for one, so there is nothing
        // to do here but decline and say so.
        let mut grab = match self.popups.grab_popup(root.clone(), popup, &seat, serial) {
            Ok(grab) => grab,
            Err(err) => {
                tracing::debug!(?err, "refused a popup grab");
                return;
            }
        };

        let keyboard = seat.get_keyboard();
        let pointer = seat.get_pointer();
        // `previous_serial` is the serial of the parent popup's grab, so a
        // submenu opening inside its parent's grab is recognised as the same
        // chain rather than as a stranger trying to steal the device.
        let chain = grab.previous_serial().unwrap_or_else(|| grab.serial());

        // Both devices are tested before either is taken. Anvil checks them
        // one at a time and calls `ungrab` from the middle, which can leave a
        // keyboard grab already installed for a chain that was then dismissed;
        // it recovers on the next key, but there is no reason to enter that
        // state. The case this refuses in practice is a client asking for a
        // menu grab while one of Solium's own grabs is running — a window
        // being dragged by `MoveGrab` or resized by `ResizeGrab` — where
        // handing the pointer to a popup would abandon the drag mid-motion.
        let keyboard_free = keyboard.as_ref().is_none_or(|keyboard| {
            may_grab(
                keyboard.is_grabbed(),
                keyboard.has_grab(serial),
                keyboard.has_grab(chain),
            )
        });
        let pointer_free = pointer.as_ref().is_none_or(|pointer| {
            may_grab(
                pointer.is_grabbed(),
                pointer.has_grab(serial),
                pointer.has_grab(chain),
            )
        });
        if !(keyboard_free && pointer_free) {
            // `grab_popup` has already recorded this popup in the seat's grab
            // chain, so declining now means undoing that — otherwise the next
            // popup would be told its parent holds a grab that nothing is
            // servicing. `All` rather than `Topmost` because the chain this
            // one was appended to is being abandoned with it.
            grab.ungrab(PopupUngrabStrategy::All);
            tracing::debug!("refused a popup grab: a device is grabbed by something else");
            return;
        }

        if keyboard.is_some() {
            // Keyboard before pointer, and the order matters. Installing the
            // pointer grab runs the *previous* pointer grab's `unset`, which
            // for a parent popup's `PopupPointerGrab` tries to take the
            // keyboard grab off again. It only does so if the keyboard grab's
            // serial is the parent's, so setting ours first is what makes a
            // submenu keep the keyboard instead of handing it back to the
            // window while its menu is still open.
            //
            // `give_keyboard` moves the selection focus with the keyboard, as
            // it does everywhere, and a menu opened from an unfocused window
            // is exactly the case where the two would otherwise part company:
            // the popup would take the keyboard while the clipboard still
            // answered to whoever had it before. Both are per-client, so for
            // the ordinary case of a menu in the already-focused window this
            // changes nothing.
            let focus = grab.current_grab();
            self.give_keyboard(focus, serial);
            self.grab_keyboard(&root, PopupKeyboardGrab::new(&grab), serial);
        }
        if let Some(pointer) = pointer {
            // `Focus::Keep`, not `Focus::Clear` as Solium's move and resize
            // grabs use: those want the pointer to stop pointing at anything
            // for the duration, whereas a menu is being pointed *at* and must
            // keep receiving enter/motion so its entries highlight.
            pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
        }
        // Kept, so that locking can dismiss the chain: see `release_grabs`.
        self.popup_grab = Some(grab);
    }

    /// A window asking for the whole screen.
    ///
    /// Not the same as maximised, and the difference is the whole point: a
    /// maximised window fills the *work area* and keeps its frame, a
    /// fullscreen one covers the monitor edge to edge with no frame and no
    /// bar over it. A video player, a game, a presentation. Without this the
    /// request was ignored entirely — the client had asked, been told nothing,
    /// and drew its own idea of fullscreen inside a titlebar.
    ///
    /// The monitor the window is on, not the active one: a video sent
    /// fullscreen on the second screen must not jump to whichever screen the
    /// pointer is over.
    fn fullscreen_request(
        &mut self,
        surface: ToplevelSurface,
        wl_output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
    ) {
        let Some(window) = self.window_for(surface.wl_surface()) else {
            return;
        };
        let Some(id) = self.panes.id_of(&window) else {
            return;
        };
        let output = wl_output
            .as_ref()
            .and_then(Output::from_resource)
            .or_else(|| {
                self.real_geometry(&window)
                    .and_then(|real| self.output_of(real))
            })
            .or_else(|| self.active_output());
        let Some(screen) = output.and_then(|output| self.space.output_geometry(&output)) else {
            return;
        };

        // Where to come back to, kept before anything moves. The same slot a
        // maximise keeps, and for the same reason: a rect that was stored is a
        // rect that comes back exactly, where one recomputed afterwards is a
        // guess.
        //
        // Not by a window that is fullscreen already: a client may ask a
        // second time, and the rect it has by then is the monitor's. Asked of
        // the xdg state rather than of whether a rect is kept, because a
        // window that went fullscreen before it had drawn has none kept (see
        // below), and its second request would otherwise keep the monitor.
        //
        // Not over a rect kept already, either. A window maximised and then
        // sent fullscreen keeps the rect from before the maximise, and it stays
        // there while `unfullscreen_request` puts the window back to maximised,
        // for the un-maximise after that to take.
        //
        // And not a rect of no size. `new_toplevel` maps a window at 0,0 before
        // it has a buffer, so one asking for fullscreen before its first
        // commit -- a player started with `--fs` -- is there with no size, and
        // a rect that describes nothing is no way back. With none kept,
        // leaving fullscreen lets the client pick its own size.
        let already = surface
            .with_pending_state(|state| state.states.contains(xdg_toplevel::State::Fullscreen));
        if !already
            && let Some(real) = self.real_geometry(&window)
            && !real.is_empty()
            && let Some(pane) = self.panes.get_mut(id)
            && pane.restore().is_none()
        {
            pane.set_restore(Some(real));
        }
        // Out of its tile, for `toggle_maximize`'s reason: a fullscreen window
        // cut down to the tile it came from is a video playing in a corner of
        // the monitor. Unconditionally rather than beside the rect above,
        // because a window that is already fullscreen, or was maximised first,
        // may have been put back in a tile by a sweep since -- and
        // `leave_tile` keeps an older way back when there is no tile to take.
        if let Some(pane) = self.panes.get_mut(id) {
            pane.leave_tile();
        }

        // The whole monitor, and no frame over it. The frame is dropped and
        // the pane marked bare, and leaving fullscreen builds a new one from
        // the style that is current then.
        //
        // Which is why the way back is kept on the pane and not on the frame:
        // it was kept on the frame until #92, and `remove` below dropped it
        // with the frame, so leaving fullscreen never had a rect to put any
        // window back at.
        self.decorations.remove(&mut self.panes, id);
        self.decorations.set_bare(&mut self.panes, id);

        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Fullscreen);
            state.size = Some(screen.size);
        });
        if surface.is_initial_configure_sent() {
            surface.send_pending_configure();
        }
        if let Some(pane) = self.panes.get_mut(id) {
            pane.set_slot(screen);
        }
        self.map_stacked(window, screen.loc, true);
        self.redraw = true;
        tracing::debug!(?screen, "a window went fullscreen");
    }

    /// And asking for it back.
    ///
    /// Back to maximised if the window was maximised when it went fullscreen,
    /// and otherwise to the rect it had before. Maximise and fullscreen keep
    /// their way back in the one slot on the pane, and that is what the two
    /// checks below are about.
    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        let Some(window) = self.window_for(surface.wl_surface()) else {
            return;
        };
        let Some(id) = self.panes.id_of(&window) else {
            return;
        };

        // Only a window that is fullscreen has anything to leave. A client may
        // send this whenever it likes, and one that sent it while merely
        // maximised had the maximise's way back spent on it: the window jumped
        // to its pre-maximise rect still marked maximised, and the next toggle
        // maximised it again rather than restoring it.
        let (fullscreen, maximized) = surface.with_pending_state(|state| {
            (
                state.states.contains(xdg_toplevel::State::Fullscreen),
                state.states.contains(xdg_toplevel::State::Maximized),
            )
        });
        if !fullscreen {
            return;
        }

        // The frame comes back unless the client draws its own, which is what
        // `is_bare` cannot tell us on its own -- so the decoration mode is
        // asked again rather than assumed.
        let client_side =
            surface.with_pending_state(|state| state.decoration_mode) == Some(Mode::ClientSide);
        if !client_side {
            self.decorations.unset_bare(&mut self.panes, id);
            let size = self
                .real_geometry(&window)
                .map_or((TITLEBAR_HEIGHT * 20, TITLEBAR_HEIGHT * 15), |real| {
                    (real.size.w, real.size.h)
                });
            self.decorations.insert(&mut self.panes, id, size.0, size.1);
        }

        // Where it goes, decided after the frame is back -- a maximised
        // window's share of the work area depends on it -- and before the
        // client is told anything, so that it is told once. It used to be two
        // configures, no size and then the size to go back to, and a client
        // that acts on every configure it reads, a terminal reflowing its grid,
        // resized twice.
        let back = if maximized {
            // Still maximised: nothing has un-maximised it. So it fills the
            // work area of the monitor it is on now, and the rect from before
            // the maximise stays kept for the un-maximise to take. Taking it
            // here placed and sized the window un-maximised while its state
            // still said `Maximized`, and left the next toggle nothing to
            // restore.
            self.real_geometry(&window)
                .and_then(|real| self.maximised(&window, real))
        } else {
            // Back into the tile it left as well, with the rect that goes with
            // it; see `toggle_maximize`. A window still maximised stays out of
            // one, which is the arm above.
            let kept = self.panes.get_mut(id).and_then(|pane| {
                pane.return_to_tile();
                pane.take_restore()
            });
            kept.map(|kept| self.back_on_a_screen(&window, kept))
        };

        surface.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Fullscreen);
            // No rect, no size: the client picks its own.
            state.size = back.map(|back| back.size);
        });
        if surface.is_initial_configure_sent() {
            surface.send_pending_configure();
        }
        if let Some(back) = back {
            if let Some(pane) = self.panes.get_mut(id) {
                pane.set_slot(back);
            }
            self.map_stacked(window, back.loc, true);
        }
        self.trigger_relayout();
        self.redraw = true;
        tracing::debug!("a window left fullscreen");
    }

    /// A client asking to be dragged — what client-side decorations send when
    /// their own titlebar is grabbed.
    fn move_request(&mut self, surface: ToplevelSurface, seat: WlSeat, serial: Serial) {
        let Some(seat) = Seat::<Self>::from_resource(&seat) else {
            return;
        };
        let Some(start_data) = self.drag_start_data(&seat, surface.wl_surface(), serial) else {
            return;
        };
        let Some(window) = self.window_for(surface.wl_surface()) else {
            return;
        };
        let Some(location) = self.space.element_location(&window) else {
            return;
        };
        let Some(pointer) = seat.get_pointer() else {
            return;
        };

        pointer.set_grab(
            self,
            MoveGrab::new(start_data, window, location),
            serial,
            Focus::Clear,
        );
    }

    /// A popup asking to be moved — a submenu re-anchoring as the pointer
    /// walks down its parent, or a reactive popup whose window has moved.
    ///
    /// Through `place_popup` for the same reason `new_popup` is: this used to
    /// take the positioner's raw geometry, so a submenu that opened inside the
    /// screen and then repositioned towards an edge was pushed off it by the
    /// very request meant to keep it visible.
    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        self.place_popup(&surface, positioner);
        surface.send_repositioned(token);
    }
}

/// Whether a popup grab may take a device that something already holds.
///
/// The three arguments are what the seat can answer about one device: whether
/// it is grabbed at all, whether the grab's serial is the one this popup is
/// being grabbed with, and whether it is the serial of the parent popup's
/// grab. A free device is takeable; so is one already held by this chain,
/// which is the submenu case and the common one. Anything else belongs to
/// somebody — one of Solium's own move or resize grabs, or another client's
/// menu — and the request is declined.
///
/// Booleans rather than the handles themselves so the rule can be tested
/// without a seat, a client and a live popup, none of which exist in a unit
/// test. See `a_grab_held_by_a_stranger_is_refused`.
const fn may_grab(grabbed: bool, this_popup: bool, its_parent: bool) -> bool {
    !grabbed || this_popup || its_parent
}

/// The rectangle a popup has to stay inside, in the coordinates its positioner
/// speaks.
///
/// A positioner's geometry is relative to the *parent surface's* window
/// geometry, and the screen is in the compositor's coordinates, so the two
/// have to be brought together before `get_unconstrained_geometry` can compare
/// them. Two translations separate them: where the root toplevel's window
/// geometry sits on the desktop, and — for a submenu — how far down the chain
/// of parent popups this one hangs.
///
/// Expressed as a subtraction from the screen rather than an addition to the
/// popup because the popup's position is the unknown: it is what the
/// positioner is about to work out.
fn popup_target(
    screen: Rectangle<i32, Logical>,
    root: Point<i32, Logical>,
    parents: Point<i32, Logical>,
) -> Rectangle<i32, Logical> {
    Rectangle::new(screen.loc - root - parents, screen.size)
}

impl Solium {
    /// Work out where a popup goes and put it in the pending state.
    ///
    /// The positioner is stored alongside the geometry because a *reactive*
    /// popup is re-constrained later, when its window moves or the screen
    /// changes, and the rules to re-run it with are the ones the client sent
    /// with the original request.
    ///
    /// The unconstrained geometry when there is a screen to constrain
    /// against, and the client's own placement when there is not — a popup
    /// rooted in something that is not a mapped window, which today means a
    /// layer surface's menu. That fallback is the behaviour this whole path
    /// replaces, so the worst case is what every popup used to get.
    fn place_popup(&self, surface: &PopupSurface, positioner: PositionerState) {
        let geometry = match self.popup_screen(surface, positioner) {
            Some(target) => positioner.get_unconstrained_geometry(target),
            None => positioner.get_geometry(),
        };
        surface.with_pending_state(|state| {
            state.positioner = positioner;
            state.geometry = geometry;
        });
    }

    /// The screen a popup must fit on, in its positioner's coordinates.
    ///
    /// The monitor under the popup's *anchor point* rather than the one its
    /// window is mostly on. They differ exactly where it matters: a window
    /// straddling two screens has a right-click menu that belongs to whichever
    /// screen the pointer was over, and constraining it to the other one would
    /// shove it back across the seam it was opened on.
    fn popup_screen(
        &self,
        surface: &PopupSurface,
        positioner: PositionerState,
    ) -> Option<Rectangle<i32, Logical>> {
        let popup = PopupKind::Xdg(surface.clone());
        let root = find_popup_root_surface(&popup).ok()?;
        let window = self.window_for(&root)?;
        // `element_location` is the window *geometry* origin, which is the
        // origin a positioner measures from — not the buffer origin, which for
        // a client with its own shadows is a couple of dozen pixels up and
        // left of it. `real_geometry` is that pairing, and `render.rs` places
        // popups against the same point.
        let real = self.real_geometry(&window)?;
        let parents = get_popup_toplevel_coords(&popup);
        let anchor = real.loc + parents + positioner.get_anchor_point();
        let output = self.output_at(anchor)?;
        let screen = self.space.output_geometry(&output)?;
        Some(popup_target(screen, real.loc, parents))
    }

    /// Send a popup its first configure, so it can draw.
    ///
    /// The same omission `configure_layer` was written for, and with the same
    /// consequence: xdg-shell forbids a client to attach a buffer before it
    /// has been configured once, Smithay deliberately leaves the initial
    /// configure to the compositor, and nothing here was sending one. A menu
    /// was created, tracked, and then waited forever for an event that was
    /// never coming — which is why Firefox's context menus did not merely
    /// appear in the wrong place, they did not appear.
    ///
    /// It also matters to the placement above. `PopupKind::location`, which is
    /// what the renderer positions a popup by, reads the *current* geometry,
    /// and `current` is only taken from the client's ack — so until a
    /// configure goes out, every popup's position stays at the default of
    /// (0, 0) no matter what `place_popup` computed.
    ///
    /// On commit rather than at `new_popup` because that is what the protocol
    /// says: the configure answers the surface's first commit. Sending one
    /// earlier would carry a size the client had not finished asking for.
    fn configure_popup(&mut self, surface: &WlSurface) {
        // Only xdg popups. An input-method popup is positioned by the
        // text-input protocol and has no configure of this kind.
        let Some(PopupKind::Xdg(popup)) = self.popups.find_popup(surface) else {
            return;
        };
        if popup.is_initial_configure_sent() {
            return;
        }
        if let Err(err) = popup.send_configure() {
            // Not fatal, and not ours to retry: the two failures Smithay
            // reports here are a client too old to be re-configured and a
            // non-reactive positioner, both of which mean the popup keeps the
            // geometry it already has.
            tracing::warn!(?err, "could not configure a popup");
        }
    }

    /// Validate a client's request to start a drag.
    ///
    /// A client may only be dragged from a press it actually received: the
    /// serial has to match the live grab, and the surface that was pressed has
    /// to belong to the same client as the one asking. Without both checks any
    /// client could start a drag of any window at any time.
    fn drag_start_data(
        &self,
        seat: &Seat<Self>,
        surface: &WlSurface,
        serial: Serial,
    ) -> Option<GrabStartData<Self>> {
        use smithay::reexports::wayland_server::Resource;

        let pointer = seat.get_pointer()?;
        if !pointer.has_grab(serial) {
            return None;
        }
        let start_data = pointer.grab_start_data()?;
        let (focused, _) = start_data.focus.as_ref()?;
        if !focused.id().same_client_as(&surface.id()) {
            return None;
        }
        Some(start_data)
    }
}

impl WlrLayerShellHandler for Solium {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        wl_output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        // A surface may name an output or leave the choice to us; a shell that
        // puts a bar on each screen names one per bar, and that is the request
        // that has to be honoured for the second screen to get a bar at all.
        //
        // The primary monitor when it names none, and not the active one: a
        // dock connects at startup, and where the pointer happened to be then
        // is not a decision anybody made. See `primary_output`.
        let output = wl_output
            .as_ref()
            .and_then(Output::from_resource)
            .or_else(|| self.primary_output());
        let Some(output) = output else {
            tracing::warn!(
                namespace,
                "a layer surface arrived with no output to put it on"
            );
            return;
        };

        // The protocol object becomes a desktop surface, which is what carries
        // the geometry and can be arranged.
        let surface = LayerSurface::new(surface, namespace.clone());
        if let Err(err) = layer_map_for_output(&output).map_layer(&surface) {
            tracing::warn!(?err, namespace, "could not map a layer surface");
            return;
        }
        // Arranging assigns the size and position the client is waiting to be
        // told; it must happen before the client can draw anything.
        layer::arrange(&output);
        tracing::info!(namespace, monitor = output.name(), "layer surface mapped");
    }

    fn ack_configure(&mut self, _surface: WlSurface, _configure: LayerSurfaceConfigure) {}

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        // Unmapped *and* rearranged: the exclusive zone it held is now free,
        // and the work area is wrong until someone recomputes it.
        for output in self.space.outputs().cloned().collect::<Vec<_>>() {
            let mut map = layer_map_for_output(&output);
            let found = map
                .layers()
                .find(|layer| layer.layer_surface() == &surface)
                .cloned();
            if let Some(layer) = found {
                map.unmap_layer(&layer);
                drop(map);
                layer::arrange(&output);
            }
        }
        tracing::info!("layer surface gone");
    }
}

impl XdgDecorationHandler for Solium {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        // Server-side is offered without being asked: the frame is part of the
        // desktop's look, and a client drawing its own would be a second
        // titlebar with different rules.
        self.decorate(&toplevel, Mode::ServerSide);
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: Mode) {
        // A client that insists on drawing its own frame gets to: overriding it
        // means two frames or none, depending on who gives way.
        self.decorate(&toplevel, mode);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        self.decorate(&toplevel, Mode::ServerSide);
    }
}

impl Solium {
    /// Agree a decoration mode with a client and act on it.
    fn decorate(&mut self, toplevel: &ToplevelSurface, mode: Mode) {
        let server_side = mode != Mode::ClientSide;

        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(if server_side {
                Mode::ServerSide
            } else {
                Mode::ClientSide
            });
        });

        // The frame belongs to the pane, so a client with no pane gets none.
        // It has still been told its mode, which is the part it is waiting on.
        let window = self.window_for(toplevel.wl_surface());
        let Some(id) = window.as_ref().and_then(|window| self.panes.id_of(window)) else {
            tracing::debug!(server_side, "decoration agreed for a window with no pane");
            if toplevel.is_initial_configure_sent() {
                toplevel.send_pending_configure();
            }
            return;
        };

        if server_side {
            let real = window.and_then(|window| self.real_geometry(&window));
            let width = real.map_or(TITLEBAR_HEIGHT * 20, |real| real.size.w);
            let height = real.map_or(TITLEBAR_HEIGHT * 15, |real| real.size.h);
            self.decorations.insert(&mut self.panes, id, width, height);
        } else {
            // Bare on purpose, not merely undecorated: the difference is
            // whether `insets_of` still reserves room for a frame that is
            // coming. For a client drawing its own, none is.
            self.decorations.remove(&mut self.panes, id);
            self.decorations.set_bare(&mut self.panes, id);
        }

        // The client has to learn its mode before it draws, or it decides for
        // itself and draws a frame we then draw over.
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
        tracing::debug!(server_side, "decoration mode agreed");
    }
}

/// The pane a token was minted for, carried on the token itself.
///
/// This is the whole of the fix for launching through a wrapper: a token is a
/// thing we made and handed out, so whatever the program does to its processes,
/// the token that comes back is still the one we gave it.
struct LaunchedFor(crate::pane::PaneId);

impl XdgActivationHandler for Solium {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.activation_state
    }

    /// A window asking to be brought forward.
    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        // Ours, from a launch: the window belongs in the one we opened for it.
        if let Some(pane) = data.user_data.get::<LaunchedFor>().map(|it| it.0)
            && self.claim_into(pane, &surface)
        {
            self.activation_state.remove_token(&token);
            return;
        }

        // Anyone else's: a window asking for focus, which is what the protocol
        // is for. Honoured because the token is proof the request came from
        // something the user was actually using -- a client cannot mint one for
        // itself out of nothing, which is the difference between this and a
        // window simply demanding focus.
        if let Some(window) = self.window_for(&surface) {
            tracing::debug!("a window asked to be brought forward");
            self.focus_window(&window, SERIAL_COUNTER.next_serial());
        }
        self.activation_state.remove_token(&token);
    }
}
smithay::delegate_xdg_activation!(Solium);

impl FractionalScaleHandler for Solium {
    /// A client has asked what scale it is really drawn at.
    ///
    /// `fractional_scale_for` has the answer, and how it is worked out; this
    /// is only the protocol's first-ask moment. The other one -- an output's
    /// scale changing later, after a client already asked -- is
    /// `resend_fractional_scale`, called from `scale_outputs`.
    fn new_fractional_scale(
        &mut self,
        surface: smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) {
        let scale = self.fractional_scale_for(&surface);
        with_states(&surface, |states| {
            with_fractional_scale(states, |fractional| {
                fractional.set_preferred_scale(scale);
            });
        });
    }
}
smithay::delegate_fractional_scale!(Solium);
// No `delegate_screencopy!`: Smithay has no handler for it, so `screencopy.rs`
// writes the `Dispatch` impls itself and there is nothing to delegate to.

smithay::delegate_viewporter!(Solium);
smithay::delegate_presentation!(Solium);

impl PointerConstraintsHandler for Solium {
    /// A client has asked for the pointer to be held still or kept inside a
    /// region.
    ///
    /// Granted straight away when the surface already has the pointer. A
    /// constraint is a request from a window that believes it is being used —
    /// a game entering mouse-look — and the honest test of that is whether the
    /// pointer is over it. One that is not is left inactive; the protocol
    /// expects it to be activated later, and the pointer arriving is when.
    fn new_constraint(
        &mut self,
        _surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
        _pointer: &smithay::input::pointer::PointerHandle<Self>,
    ) {
        // Deliberately not activated here.
        //
        // This runs inside the client's own `lock_pointer` request, and
        // activating sends `locked()` straight back down the same dispatch --
        // before the client has finished setting up. Firefox creates its
        // relative pointer six microseconds after asking for the lock and
        // attaches its handlers after that; `locked()` arriving in between was
        // discarded, and a lock the client never saw confirmed is a lock it
        // does not act on. The relative motion was delivered perfectly and the
        // page ignored every event of it.
        //
        // So it is activated on the next pointer motion instead, in `held`,
        // which is both a later dispatch and the first moment the answer
        // actually matters.
        tracing::debug!("a window asked for the pointer");
    }

    /// A locked pointer's client saying where it would like the cursor left.
    ///
    /// Taken as advice and acted on when the lock ends, which is what the hint
    /// is for: a game that locked the pointer in the middle of its window wants
    /// it back in the middle, not wherever it happened to be when the lock was
    /// taken.
    fn cursor_position_hint(
        &mut self,
        surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
        pointer: &smithay::input::pointer::PointerHandle<Self>,
        location: Point<f64, Logical>,
    ) {
        let active = with_pointer_constraint(surface, pointer, |constraint| {
            constraint.is_some_and(|constraint| constraint.is_active())
        });
        if !active {
            return;
        }
        if let Some(origin) = self
            .window_for(surface)
            .and_then(|window| self.real_geometry(&window))
        {
            self.constraint_hint = Some(origin.loc.to_f64() + location);
        }
    }
}
smithay::delegate_pointer_constraints!(Solium);
smithay::delegate_relative_pointer!(Solium);

impl SeatHandler for Solium {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    /// Both client-side cursor sources arrive here, and which one it was is
    /// readable off the variant.
    ///
    /// `Surface` and `Hidden` are `wl_pointer.set_cursor`: the client
    /// rasterised a cursor itself and we draw its pixels. `Named` is
    /// `wp_cursor_shape_v1.set_shape` — the only thing that can produce one,
    /// since `set_cursor` carries a surface or nothing — and it means the
    /// client named a shape and left the picture to us. Dropping either on the
    /// floor leaves every application with our arrow, which for the named case
    /// is exactly what #24 was: a text field that never showed an I-beam.
    ///
    /// Through `show` rather than assigned, so that a client alternating
    /// between its own two mechanisms cannot leave a fragment of the other
    /// behind. See `cursor::Pointer::show`, which is the only writer and where
    /// the precedence between all three sources is set out.
    ///
    /// **And it damages the screen, which is not optional.** Both backends draw
    /// only when something has changed — `tty.rs`'s loop and `winit.rs`'s make
    /// the same test, each with a comment arguing for it — and a client's reply
    /// to `wl_pointer.enter` arrives a round trip *after* the motion that
    /// provoked it, by which time the frame that motion caused has already been
    /// drawn. Without this, moving onto a text field and stopping leaves the
    /// old arrow sitting there until something unrelated damages the screen,
    /// and jiggling the mouse is the only way to see the I-beam.
    ///
    /// It cost nothing before #24 only because `Named(_)` was discarded, so the
    /// picture genuinely did not change. It is now the main visible path of the
    /// whole feature, and a feature that only works while the mouse is moving
    /// reads as a broken one.
    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.pointer.show(image);
        self.redraw = true;
    }
    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&WlSurface>) {}
}

/// Required by smithay's `wp_cursor_shape_v1` dispatch, and empty on purpose.
///
/// The protocol hands out a shape device for a `wl_pointer` *or* for a
/// `zwp_tablet_tool_v2`, so its `Dispatch` impl is bound on `TabletSeatHandler`
/// whether or not the compositor has tablets — see
/// `wayland/cursor_shape.rs:240`. Solium advertises no tablet manager, so no
/// client can ever hold a `zwp_tablet_tool_v2` to ask for one, and
/// `tablet_tool_image` is unreachable rather than unimplemented. The default
/// body discards the image, which is the right thing for a cursor that has no
/// device to be drawn for.
impl smithay::wayland::tablet_manager::TabletSeatHandler for Solium {}

impl DmabufHandler for Solium {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    /// A client has built a buffer out of GPU memory and wants to know whether
    /// it is usable.
    ///
    /// Accepted on the strength of the format list the global was created with
    /// — the renderer's own — rather than by importing here, because the
    /// renderer belongs to the backend and this does not. The real import
    /// happens when the buffer is committed, and says so in the log if it
    /// fails. The honest cost: a client whose buffer we cannot import is told
    /// "yes" and then shows nothing, instead of being told "no" and falling
    /// back to shared memory.
    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        _dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        if let Err(err) = notifier.successful::<Self>() {
            tracing::warn!(?err, "could not accept a client's dmabuf");
        }
    }
}
delegate_dmabuf!(Solium);

impl SelectionHandler for Solium {
    type SelectionUserData = ();

    /// A Wayland client has copied something. Tell the X11 side it exists.
    ///
    /// Only that it exists, and in which formats — the data itself is not moved
    /// anywhere. X11 selections are the same idea: the owner advertises types
    /// and hands over bytes when someone asks. Copying a megabyte in one
    /// toolkit and pasting nothing in the other should cost nothing.
    fn new_selection(
        &mut self,
        ty: SelectionTarget,
        source: Option<SelectionSource>,
        _seat: Seat<Self>,
    ) {
        let Some(xwm) = self.xwm.as_mut() else {
            return;
        };
        let mimes = source.map(|source| source.mime_types());
        if let Err(err) = xwm.new_selection(ty, mimes) {
            tracing::warn!(?err, ?ty, "could not offer a selection to X11");
            return;
        }

        // Make the X server actually hear about it.
        //
        // `new_selection` issues `SetSelectionOwner` and does not flush, and
        // x11rb buffers requests -- so the ownership change sits in the output
        // buffer until something unrelated forces a flush, which is the next X
        // event to arrive. If none does, the X server still believes nobody
        // owns the selection: a client asking gets nothing, and the compositor
        // is never even consulted. Copying in a Wayland app and pasting in an
        // X11 one worked or did not depending on whether anything else
        // happened to be talking to X, which is as good as a coin toss.
        //
        // There is no public `flush` on `X11Wm`. This is a read-only query
        // that round-trips, and a round-trip has to flush the output buffer
        // before it can wait for the reply. The answer is discarded; the flush
        // is the point.
        let _ = xwm.get_randr_primary_output();
    }

    /// A Wayland client wants to read a selection an X11 client owns.
    ///
    /// Recorded here and carried out by the backend: see `pending_selection`.
    fn send_selection(
        &mut self,
        ty: SelectionTarget,
        mime_type: String,
        fd: std::os::fd::OwnedFd,
        _seat: Seat<Self>,
        (): &(),
    ) {
        self.pending_selection = Some((ty, mime_type, fd));
    }
}

impl OutputHandler for Solium {}

impl DataDeviceHandler for Solium {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}
impl PrimarySelectionHandler for Solium {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}

impl ClientDndGrabHandler for Solium {
    /// A client has started a drag, and this is the only moment its icon is
    /// offered. See [`Solium::dnd_icon`] for why nothing can ask for it later.
    ///
    /// `icon` is `None` for a drag the client chose not to illustrate, which is
    /// ordinary and not a failure — a text selection dragged inside one window
    /// often has no icon at all. Stored as-is: the surface already carries the
    /// `dnd_icon` role, which smithay gave it in `data_device::device.rs`
    /// before this is called and which is what stops the same surface being a
    /// cursor or a toplevel at the same time.
    ///
    /// No cursor assertion is made here, and that is deliberate. A drag begun
    /// with the pointer installs a `DnDGrab` on it, so `pointer.is_grabbed()`
    /// is true for the whole gesture, and that one test holds *both* halves of
    /// the pointer: [`Solium::assert_cursor`] declines to recompute `chrome`,
    /// and [`crate::input::release_cursor`] declines to clear `status` over the
    /// gaps the drag crosses. The drag owns the pointer until it ends, exactly
    /// as a resize drag does, and it needs no flag of its own to say so.
    ///
    /// So the compositor says nothing about the shape for the length of the
    /// drag — which is not the same as the shape being frozen, and the
    /// difference matters. The drag's own client may still change it, and is
    /// meant to: `wl_pointer.set_cursor` is accepted from the holder of a grab
    /// (`wayland/seat/pointer.rs:521`, and its comment names drag and drop),
    /// so a toolkit swapping between `dnd-copy` and `dnd-no-drop` as it crosses
    /// drop targets reaches [`SeatHandler::cursor_image`] mid-drag and is
    /// obeyed. What is held is the compositor's hands off it.
    ///
    /// **The grab is the pointer's only for a pointer-initiated drag.**
    /// `start_drag` installs it on whichever device the start serial came from:
    /// a pointer serial takes `PointerHandle::set_grab`, a touch serial takes
    /// `TouchHandle::set_grab` and returns before the pointer branch is ever
    /// reached — `selection/data_device/device.rs:95`. A drag begun with a
    /// finger therefore leaves `pointer.is_grabbed()` false for its whole
    /// length, and neither guard above holds anything. That is reachable rather
    /// than hypothetical: [`Solium::new`] calls `seat.add_touch()`, and
    /// `input::handle` routes `TouchDown` into it.
    ///
    /// It costs nothing today because a finger moves no pointer. Both guarded
    /// writes sit on the pointer motion paths, so a touch drag reaches neither
    /// unless a mouse is moved alongside the finger — and at that point the
    /// pointer honestly is not the thing dragging, so describing what is under
    /// it is the right answer rather than a missed one. What a touch drag does
    /// get wrong is the icon, which `render::elements` puts at the pointer
    /// because the pointer is the only position it has; re-deriving that from
    /// the touch grab is the work touch support will bring, and the cursor
    /// rules here are the pointer's and stay the pointer's.
    fn started(
        &mut self,
        _source: Option<WlDataSource>,
        icon: Option<WlSurface>,
        _seat: Seat<Self>,
    ) {
        self.dnd_icon = icon;
        // The icon appears at the pointer on the next frame and nothing else
        // on screen has changed, so without this a drag started without moving
        // the mouse would draw nothing until something unrelated redrew.
        self.redraw = true;
    }

    /// The buttons came up. Whether the drop was accepted or refused, the icon
    /// stops being drawn now.
    ///
    /// **This is the reliable end of a drag, and the only one.** `DnDGrab`
    /// implements `PointerGrab::unset` (and `TouchGrab::unset`) as a call to
    /// its own `drop`, and `drop` calls this — so a grab taken away by
    /// something else, a cancelled touch, and an ordinary button release all
    /// arrive here. Clearing anywhere else would leave the icon painted over
    /// the session after the gesture that owned it was over.
    fn dropped(&mut self, _target: Option<WlSurface>, _validated: bool, _seat: Seat<Self>) {
        self.dnd_icon = None;
        // The icon was drawn last frame and will not be this one. Nothing else
        // damages that region, so a drop onto a still window would otherwise
        // leave the icon on screen until the next unrelated frame.
        self.redraw = true;
    }
}

impl ServerDndGrabHandler for Solium {}

/// `xdg_dialog_v1` -- a client saying a toplevel is a modal dialog.
///
/// The protocol is one flag on an object hung off a toplevel, and smithay
/// already keeps it: `set_modal`/`unset_modal` write
/// `XdgToplevelSurfaceRoleAttributes::modal`, and this handler is told only
/// when the value *changes* (`wayland/shell/xdg/dialog.rs` returns early when
/// it does not). So there is no state to mirror here; there is only the fact
/// that the layout's input just changed, and something has to say so.
///
/// ## What a non-modal dialog gets, and why
///
/// The protocol makes modality a flag on a dialog object rather than the
/// meaning of the object, so a client may create an `xdg_dialog_v1` for a
/// toplevel and never call `set_modal`. **Such a window gets the ordinary
/// treatment here: it is laid out like any other, with a share of the screen.**
///
/// It is worth being plain that this is not a free choice: with smithay 0.7 it
/// is the only one that can be implemented. Nothing reaches this compositor
/// when a dialog object is created. `XdgDialogHandler` has exactly one method,
/// the `modal_changed` below, and creating the object changes no flag; the
/// object itself is stored in `XdgShellSurfaceUserData::dialog`, which is
/// `pub(crate)` to smithay and has no accessor (`shell/xdg/handlers/surface.rs`
/// -- read the source, not the docs). So "this toplevel is a dialog but not a
/// modal one" is a state Solium cannot observe at all. Taking the other branch
/// would mean dispatching `xdg_wm_dialog_v1` ourselves and keeping a second
/// copy of state smithay already holds, which is how two answers to one
/// question get out of step.
///
/// That said, it is also the answer this would pick with the field in hand, and
/// that matters more than which one is cheap. What the protocol actually
/// *defines* for a non-modal dialog is nothing: `set_modal` is described as the
/// hint that the window must be addressed before its parent can be used again,
/// and the dialog object without it carries no stated behaviour, only the
/// possibility of future hints. The whole argument for lifting a window out of
/// the arrangement is that it is blocking the window underneath it and will be
/// gone in a moment. A dialog that blocks nothing has neither half of that: a
/// non-modal find bar or a colour picker is a window somebody keeps open beside
/// their document, and floating it in the middle of the screen, over the
/// document, is a worse answer than tiling it. Compare the X11 side, where the
/// same question is decided the same way for the same reason:
/// `xwayland::floats_over_its_parent` floats `Dialog` and not `Utility`.
///
/// If a toolkit is ever found creating dialog objects for prompts and leaving
/// `set_modal` unsent, this is the paragraph to revisit -- and the revision
/// would start with smithay, not here.
impl XdgDialogHandler for Solium {
    /// Re-run the layout, because a window just left the arrangement or
    /// rejoined it.
    ///
    /// `trigger_relayout` and nothing else. The alternative -- placing the
    /// dialog from here -- would put a second opinion about where a window goes
    /// next to the layout scripts' one, and the two would disagree the first
    /// time somebody wrote their own `tiling.lua`. Where a modal dialog goes is
    /// a layout question; that it *is* one is the only thing the compositor
    /// knows and the only thing it says.
    ///
    /// This is also what makes `unset_modal` work at all. Without it, a dialog
    /// that stopped being modal would sit floating until some unrelated event
    /// happened to re-run the layout, which on a quiet desktop is never.
    fn modal_changed(&mut self, toplevel: ToplevelSurface, is_modal: bool) {
        let id = self
            .window_for(toplevel.wl_surface())
            .and_then(|window| self.panes.id_of(&window))
            .map(crate::pane::PaneId::get);
        tracing::debug!(?id, is_modal, "a toplevel changed its modal hint");
        self.trigger_relayout();
    }
}

delegate_compositor!(Solium);
delegate_shm!(Solium);
delegate_xdg_shell!(Solium);
delegate_xdg_decoration!(Solium);
smithay::delegate_xdg_dialog!(Solium);
delegate_layer_shell!(Solium);
delegate_seat!(Solium);
// Routes `wp_cursor_shape_manager_v1` and the per-pointer device it hands out.
// smithay's dispatch turns a `set_shape` into `SeatHandler::cursor_image` with
// a `CursorImageStatus::Named`, so the handler for this protocol is the seat
// handler above rather than a trait of its own.
smithay::delegate_cursor_shape!(Solium);
delegate_output!(Solium);
delegate_data_device!(Solium);
smithay::delegate_primary_selection!(Solium);
smithay::delegate_xwayland_shell!(Solium);

#[cfg(test)]
mod tests {
    use super::*;

    /// Two monitors side by side, 1920 wide each, as `space.output_geometry`
    /// would report them.
    fn two_monitors() -> [Rectangle<i32, Logical>; 2] {
        [
            Rectangle::new((0, 0).into(), (1920, 1080).into()),
            Rectangle::new((1920, 0).into(), (1920, 1080).into()),
        ]
    }

    fn at(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    /// **The cull `render::prepare` runs before it captures anything.**
    ///
    /// A hidden workspace is not unmapped -- it is drawn a screen away -- so
    /// "is any of this rectangle on a monitor" is the question that separates
    /// a window worth capturing from one nothing will ever draw. Getting it
    /// wrong in the cheap direction costs a permanent offscreen pass per
    /// hidden window; getting it wrong in the other direction stops a visible
    /// window being captured, which is a blank corner. So both directions are
    /// asserted, and each line names an implementation it rules out.
    #[test]
    fn a_pane_is_on_a_monitor_only_if_some_monitor_covers_part_of_it() {
        let screens = two_monitors();

        assert!(
            anywhere_on(at(100, 100, 800, 600), screens),
            "a window in the middle of the first monitor is on it"
        );
        // Rules out `all(..)` in place of `any(..)`: this one touches only the
        // second screen, and with two monitors mapped that is the common case.
        assert!(
            anywhere_on(at(2000, 100, 800, 600), screens),
            "and one on the second monitor is on that"
        );
        assert!(
            anywhere_on(at(1800, 100, 400, 600), screens),
            "a window dragged across the bezel is on both"
        );

        // The case the cull exists for: a workspace hidden by being parked one
        // screen to the left of the desk. Rules out `|_| true`.
        assert!(
            !anywhere_on(at(-1920, 0, 1920, 1080), screens),
            "a workspace parked a screen away is on no monitor, which is how a \
             workspace switch hides one"
        );
        assert!(
            !anywhere_on(at(0, -2000, 800, 600), screens),
            "and so is one parked above the desk"
        );

        // Exclusive, matching `render::elements`. Rules out
        // `overlaps_or_touches`, which differs from `overlaps` only here.
        assert!(
            !anywhere_on(at(-800, 0, 800, 1080), screens),
            "a window whose right edge is exactly the monitor's left edge has \
             no pixel on it"
        );

        // Rules out a constant `true`, and covers the moment between a monitor
        // going away and the session noticing.
        assert!(!anywhere_on(at(100, 100, 800, 600), []));
    }

    /// **What the notice at the end of a reload is looking at.**
    ///
    /// The recovery half of #116 shipped with no test at all, which is how the
    /// inversion below went out with it. This is the decision itself: three
    /// answers, and the third is the one that matters — a *lost* desktop and an
    /// *empty* workspace are the same picture, so `None` and `Some(true)` are
    /// different claims and the empty cases must not come back as "everything
    /// is off stage".
    #[test]
    fn a_desktop_is_off_stage_only_when_there_is_a_desktop_and_it_is_off() {
        let screens = two_monitors().to_vec();
        let on = at(100, 100, 800, 600).to_f64();
        let off = at(-4000, 0, 1920, 1080).to_f64();

        assert_eq!(
            nothing_on_stage([off], &screens),
            Some(true),
            "a window carried clear of every screen is the picture this warns about"
        );
        // Rules out `all(..)` for `any(..)` twice over: one window still on a
        // screen is a desktop that is there, whichever order they come in.
        assert_eq!(
            nothing_on_stage([off, on], &screens),
            Some(false),
            "one window still on a screen means the desktop is not lost"
        );
        assert_eq!(
            nothing_on_stage([on, off], &screens),
            Some(false),
            "and the answer does not depend on which window is looked at first"
        );

        // Neither of these is a question with an answer, and neither may come
        // back as `Some(true)`: the caller warns on exactly that, and a session
        // with nothing open would then be told its desktop had been carried
        // away.
        assert_eq!(
            nothing_on_stage([], &screens),
            None,
            "no windows is not an off-stage desktop"
        );
        assert_eq!(
            nothing_on_stage([on], &[]),
            None,
            "and neither is no screens, which is every window off every one of \
             them by arithmetic"
        );
    }

    /// **And when it looks, which is the whole of the fault.**
    ///
    /// `everything_is_off_stage` ran at `self.clock.now()`, immediately after
    /// the reload's three dispatches. A workspace slide is an animation, and at
    /// its own start instant it has moved nothing — so the notice described the
    /// session that had just been thrown away. A reload that *rescued* an
    /// off-stage desktop warned about it, and one that carried the desktop off
    /// said nothing at all.
    ///
    /// Asked of a real [`Groups`] carrying a real [`present::Transform`],
    /// through the same `Shift::apply` that [`Solium::drawn_at`] ends in. Both
    /// instants are asserted, because "sampled later" is only worth anything if
    /// the earlier sample really does give the other answer — otherwise this
    /// would pass against the code it is pinned against.
    ///
    /// **The honest limit.** This pins [`SETTLED`] and what it is for; it
    /// cannot see `everything_is_off_stage` choosing to sample somewhere else,
    /// because that method needs a `Display` and a mapped `Space` and so cannot
    /// be called here at all. Setting `SETTLED` back to `ZERO` — which is what
    /// the code did — fails the second assertion.
    #[test]
    fn a_slide_that_has_not_started_yet_is_not_where_the_desks_end_up() {
        let start = Duration::from_secs(10);
        let screens = two_monitors().to_vec();
        let real = at(100, 100, 800, 600);

        let mut groups = crate::group::Groups::default();
        groups.declare(
            "desk-2",
            crate::group::Selection {
                members: vec![crate::group::Member::Window(7)],
                on: None,
            },
            start,
        );
        // A screen and a bit to the left, over 300ms: `workspaces.lua`'s own
        // numbers, and what a reload onto another workspace asks for.
        groups.present(
            "desk-2",
            crate::group::Shift {
                dx: -1920.0 * 1.06,
                ..crate::group::Shift::NONE
            },
            start,
            Duration::from_millis(300),
            present::Curve::OutCubic,
        );

        let where_it_is = |now| groups.on_window(7, None, now).apply(Frame::real(real)).rect;

        assert_eq!(
            nothing_on_stage([where_it_is(start)], &screens),
            Some(false),
            "at the instant the reload finishes, the slide it started has moved \
             nothing -- so this is the session before the reload, and reporting it \
             is reporting the wrong one"
        );
        assert_eq!(
            nothing_on_stage([where_it_is(start + SETTLED)], &screens),
            Some(true),
            "SETTLED is not far enough ahead for the transforms this reload started \
             to have landed, so the notice still describes the previous session"
        );
    }

    #[test]
    fn a_frame_reserves_what_it_always_reserved() {
        // The three answers `insets_of` used to assemble from two tables,
        // now read off one value.
        assert_eq!(
            insets_for(&crate::pane::Frame::Pending),
            Insets {
                top: TITLEBAR_HEIGHT,
                ..Insets::NONE
            },
            "a frame that has not been built yet still reserves room for one, \
             or the window changes shape the moment it arrives"
        );
        assert_eq!(
            insets_for(&crate::pane::Frame::None),
            Insets::NONE,
            "a pane that will never have a frame reserves nothing -- a \
             titlebar's worth of blank space with no titlebar in it is what \
             an Electron application looked like here"
        );
        // The third answer -- that a built frame reserves what its decoration
        // asked for, on every side, so that a bar along the left and a border
        // are the same mechanism -- was asserted here against a `Styled` arm
        // carrying a plain `Insets`. That arm carries the `Decoration` itself
        // now, and `Decoration::new` is private to `decoration.rs`, so the case
        // cannot be written here.
        //
        // It *can* be written there, and is: `decoration.rs`'s tests build real
        // frames. The reason this file does not reach over and do the same is
        // not that a frame needs a GPU and a display -- it does not, and this
        // comment said so for a while. It is that `solium_qml_start` assigns
        // the one `QGuiApplication` without a lock, so the tests that bring Qt
        // up share a mutex, and that mutex is in the module where they live.
        // A second, unsynchronised starter in another module is a data race.
    }

    /// Drawing is unclipped; input is not. A spike reaching over the next
    /// window must not eat that window's clicks — the failure mode is a
    /// neighbour that has silently stopped responding, with nothing on screen
    /// to explain it.
    ///
    /// **It passed the moment it was written, and that is the point.** Every
    /// hit-test in this file — `chrome_under`, `decorated_under`,
    /// `window_under` and `surface_under` — reaches its pane through
    /// [`Solium::pane_outer`], and the two that own a decoration gate on
    /// `drawn.rect.contains(location)` before a layer is asked anything at all.
    /// So the invariant holds by construction and nothing *states* it: the
    /// canvas is a rectangle that exists, is larger, is right there in
    /// `decoration.rs`, and is exactly what someone fixing "my glow does not
    /// take clicks" would reach for.
    ///
    /// **What it is and is not.** It is the two rectangles' relationship, in
    /// one place, with the reason written down; it is not a guard on the
    /// hit-tests, and it adds no machine-checked coverage that `decoration.rs`
    /// did not already have. Both controls below were run, and between them
    /// they are the honest limit of this test: the one it fails is pinned
    /// twice over elsewhere, and the regression it is named for is pinned
    /// nowhere.
    ///
    /// | control | measured |
    /// |---|---|
    /// | `canvas` returning `outer` — the state before Task 5 | fails on the first assertion, but so do `decoration::tests::a_canvas_is_the_pane_grown_by_its_bleed` and `no_bleed_means_the_canvas_is_the_pane`, which pin it already |
    /// | the frame band **and** `decorated_under` switched to the canvas, through `decoration::spread` | the whole suite still passes, 201 of 201 |
    ///
    /// The regression is instead caught with a real pointer, which is what the
    /// task's step 4 is for: two panes side by side under `bleedy`, a press
    /// 60px into the first one's bleed and 20px inside the second, and the
    /// second takes focus. Run against the canvas-hit-test build above, the
    /// *first* window takes it and the second is left unfocused — a neighbour
    /// that has silently stopped responding, exactly as described.
    ///
    /// A point 30px to the *left* of the pane: inside a canvas that bled 50, and
    /// outside the pane on the only axis that matters.
    #[test]
    fn a_point_in_the_bleed_is_not_in_the_pane() {
        let outer = Rectangle::<i32, Logical>::new((100, 100).into(), (200, 200).into());
        let bleed = crate::style::Bleed {
            top: 50,
            right: 50,
            bottom: 50,
            left: 50,
        };
        let canvas = crate::decoration::canvas(outer, bleed);
        let in_bleed = smithay::utils::Point::<f64, smithay::utils::Logical>::from((70.0, 120.0));
        assert!(canvas.to_f64().contains(in_bleed));
        assert!(!outer.to_f64().contains(in_bleed));
    }

    /// An ordinary framed window: 400x300 at (100, 100) under a titlebar and
    /// nothing else reserved, which is the shape both of #108's symptoms were
    /// reported on.
    fn framed() -> (Rectangle<i32, Logical>, Insets) {
        (
            Rectangle::new((100, 100).into(), (400, 300).into()),
            Insets {
                top: TITLEBAR_HEIGHT,
                ..Insets::NONE
            },
        )
    }

    /// What [`Solium::pane_chrome`] makes of a point of that window, with the
    /// pane drawn where the layout put it — no transform, so a screen point
    /// and a pane-local point differ only by the pane's corner.
    fn chrome_at(point: (f64, f64)) -> Option<Chrome> {
        let (outer, insets) = framed();
        let location = Point::<f64, Logical>::from(point);
        chrome_of(
            on_frame(outer.size, insets, location - outer.loc.to_f64()),
            resize::border_edges(outer, location),
        )
    }

    /// The whole of [`claim_of`] at a point of that window, with the two links
    /// above the chrome supplied by the caller: `surface` is an interactive
    /// scripted surface above the windows claiming the point, `mode` is a
    /// script grab held — `sol.grab(true)`, which is overview.
    fn claim_at(point: (f64, f64), surface: bool, mode: bool) -> Claim {
        claim_of(surface, mode, chrome_at(point))
    }

    /// **Issue #108, and the assertion the fix is actually for.**
    ///
    /// The cursor and the press must be reading the same answer, because the
    /// bug was that they were not. The frame's band and the resize border
    /// genuinely overlap — the top `RESIZE_BORDER` pixels of a titlebar are
    /// within reach of the window's top edge — and the press had always
    /// resolved that in the frame's favour while the pointer resolved it not
    /// at all, leaving whatever a CSD client had set for its own shadow's
    /// resize affordance. So in that band the pointer drew a resize arrow and
    /// a drag moved the window.
    ///
    /// What is pinned here is that no point can be claimed by both: whatever
    /// [`claim_of`] answers is *the* answer, and the cursor is a function of it
    /// rather than of a second hit test. A test that checked only the
    /// edge-to-icon mapping would pass with the second symptom still in place,
    /// which is why the sweep below is the body of this test and the named
    /// points are only the landmarks.
    ///
    /// **The chrome is one link of three, and the first version of this test
    /// swept only that one.** It was green while the pointer still promised a
    /// resize over an overview thumbnail and over the bottom edge of a scripted
    /// bar, because it never held a script grab and never put a surface over
    /// the point — so it exercised exactly the world in which the bug does not
    /// appear. The sweep is now run in all four worlds, and the deferring ones
    /// are checked against the count of pixels the chrome *would* have claimed,
    /// so a chain that quietly stopped deferring could not leave this passing.
    #[test]
    fn the_pointer_and_the_press_cannot_claim_different_things() {
        let (outer, insets) = framed();

        // The overlap is real, not theoretical -- without this the sweep's
        // "never both" would be vacuously true and would stay true if somebody
        // shrank the titlebar to nothing.
        let contested = Point::<f64, Logical>::from((300.0, 104.0));
        assert!(
            on_frame(outer.size, insets, contested - outer.loc.to_f64()),
            "four pixels below the top edge is inside a {TITLEBAR_HEIGHT}px \
             titlebar"
        );
        assert_ne!(
            resize::border_edges(outer, contested),
            ResizeEdge::None,
            "and inside the top resize border, which is what the two disagreed \
             about"
        );
        // The frame takes it, because the frame is what a press there does.
        assert_eq!(chrome_at((300.0, 104.0)), Some(Chrome::Frame));
        assert_eq!(
            claim_at((300.0, 104.0), false, false).cursor(),
            Some(CursorIcon::Default),
            "the band that moves the window must not draw a resize cursor: \
             that is #108's second symptom"
        );

        // Four pixels *above* the top edge is outside the window, so the frame
        // has no claim on it and the border does. This is where dragging the
        // top edge still works, and it now says so.
        assert_eq!(
            chrome_at((300.0, 96.0)),
            Some(Chrome::Resize(ResizeEdge::Top))
        );

        // The first symptom: the bottom-right corner resizes, and now looks
        // like it. Nothing is reserved along the bottom or the right, so the
        // corner is the client's pixels and the border's claim alone.
        let corner = (497.0, 397.0);
        assert_eq!(
            chrome_at(corner),
            Some(Chrome::Resize(ResizeEdge::BottomRight))
        );
        assert_eq!(
            claim_at(corner, false, false).cursor(),
            Some(CursorIcon::NwseResize)
        );

        // The middle of the client is nobody's chrome, which is what leaves a
        // client free to name its own cursor over its own window.
        assert_eq!(chrome_at((300.0, 250.0)), None);
        assert_eq!(chrome_at((900.0, 900.0)), None);

        // **A mode holds the grab: overview.** `lua/overview.lua` sets it and
        // hit-tests its own thumbnails, and `pointer_button` hands every press
        // to the mode before it looks at any chrome -- so a press on that same
        // corner focuses the window and leaves overview. Offering
        // `NwseResize` there is the pointer describing an action that will not
        // happen, which is the whole of #108, on `super+space`.
        assert_eq!(claim_at(corner, false, true), Claim::Mode);
        assert_eq!(
            claim_at(corner, false, true).cursor(),
            None,
            "in a mode the press is the mode's, so the pointer promises nothing"
        );
        assert_eq!(claim_at((300.0, 104.0), false, true).cursor(), None);

        // **A scripted surface takes the press.** The bottom eight pixels of a
        // `layer = "top"` bar are within reach of a maximised window's top
        // edge, and the tweaks panel is a full-height overlay down the right of
        // one. The press goes to the panel; the border cursor would have said
        // it resized the window.
        assert_eq!(claim_at(corner, true, false), Claim::Surface);
        assert_eq!(
            claim_at(corner, true, false).cursor(),
            None,
            "the press is the surface's, so the pointer leaves the shape to it"
        );
        assert_eq!(claim_at((300.0, 104.0), true, false).cursor(), None);

        // And the order between the two, which is the order `pointer_button`
        // asks them in: a surface above the windows is offered the press before
        // the mode is consulted.
        assert_eq!(claim_of(true, true, None), Claim::Surface);
        assert_eq!(claim_of(false, false, None), Claim::Nothing);
        assert_eq!(claim_of(false, false, None).cursor(), None);

        // And the whole neighbourhood of the window, a pixel at a time, in
        // each of the four worlds the two links above the chrome make.
        let mut chrome_pixels = 0_u32;
        let mut deferred_pixels = 0_u32;
        for (surface, mode) in [(false, false), (true, false), (false, true), (true, true)] {
            for y in 80..=420 {
                for x in 80..=520 {
                    let location = Point::<f64, Logical>::from((f64::from(x), f64::from(y)));
                    let framed_here = on_frame(outer.size, insets, location - outer.loc.to_f64());
                    let edges = resize::border_edges(outer, location);
                    let chrome = chrome_of(framed_here, edges);
                    if !surface && !mode && chrome.is_some() {
                        chrome_pixels += 1;
                    }
                    if (surface || mode) && chrome.is_some() {
                        deferred_pixels += 1;
                    }
                    match claim_of(surface, mode, chrome) {
                        Claim::Surface => {
                            assert!(
                                surface,
                                "nothing was over {location:?} and the pointer \
                                 stood aside for it"
                            );
                            assert_eq!(claim_of(surface, mode, chrome).cursor(), None);
                        }
                        Claim::Mode => {
                            assert!(
                                mode && !surface,
                                "no mode holds the grab at {location:?}, or a \
                                 surface should have taken it first"
                            );
                            assert_eq!(claim_of(surface, mode, chrome).cursor(), None);
                        }
                        Claim::Chrome(Chrome::Frame) => {
                            assert!(
                                !surface && !mode,
                                "a press at {location:?} would never reach the \
                                 frame, and the pointer says it would"
                            );
                            assert!(
                                framed_here,
                                "a press at {location:?} would not hit the \
                                 frame, but the pointer says it would"
                            );
                        }
                        Claim::Chrome(Chrome::Resize(edges)) => {
                            assert!(
                                !surface && !mode,
                                "a press at {location:?} would never reach the \
                                 resize border, and the pointer offered to drag \
                                 it {edges:?}"
                            );
                            assert!(
                                !framed_here,
                                "a press at {location:?} moves the window, and \
                                 the pointer offered to resize it {edges:?}"
                            );
                            assert_ne!(edges, ResizeEdge::None);
                            assert_eq!(
                                Chrome::Resize(edges).cursor(),
                                resize::cursor(edges),
                                "the cursor over a border is the border's own, \
                                 whatever else is on screen"
                            );
                        }
                        Claim::Nothing => assert!(
                            !surface && !mode && !framed_here && edges == ResizeEdge::None,
                            "the compositor asserts nothing at {location:?} \
                             while claiming to own it"
                        ),
                    }
                }
            }
        }

        // The deferring worlds are only worth sweeping if the chrome had
        // something to say at those pixels, which is what made the first
        // version of this test green over a live disagreement. Three of the
        // four worlds defer, so the same pixels are counted three times.
        assert!(chrome_pixels > 0, "the sweep never crossed any chrome");
        assert_eq!(
            deferred_pixels,
            chrome_pixels * 3,
            "every pixel the chrome would have claimed must be one the pointer \
             gave up in each of the three worlds where the press never gets \
             there"
        );
    }

    /// The frame band is the insets and only the insets.
    ///
    /// Two directions, because getting it wrong either way is a bug with a
    /// face: too wide and a strip of the client stops taking clicks, too
    /// narrow and the titlebar has a dead line along one edge. The outer bound
    /// is asserted separately -- it is what stops every point above a window
    /// from counting as its titlebar, which is the mistake a "not the client
    /// rect" test makes on its own.
    #[test]
    fn the_frame_band_is_what_the_insets_reserved() {
        let (outer, insets) = framed();
        let local = |x: f64, y: f64| Point::<f64, Logical>::from((x, y));

        assert!(on_frame(outer.size, insets, local(200.0, 0.0)));
        assert!(on_frame(
            outer.size,
            insets,
            local(200.0, f64::from(TITLEBAR_HEIGHT) - 1.0)
        ));
        assert!(
            !on_frame(outer.size, insets, local(200.0, f64::from(TITLEBAR_HEIGHT))),
            "the first row below the bar is the client's"
        );
        assert!(
            !on_frame(outer.size, insets, local(200.0, -1.0)),
            "a point above the window is not its titlebar"
        );
        assert!(
            !on_frame(outer.size, insets, local(200.0, 500.0)),
            "nor is a point below it, which reserves nothing"
        );
        assert!(
            !on_frame(outer.size, Insets::NONE, local(200.0, 0.0)),
            "a decoration that reserves nothing owns no band, and its clicks \
             belong to the window under it"
        );
    }

    /// One pane of a stack, as the pure rules see it: where it is drawn, and
    /// what its frame reserves.
    ///
    /// No presentation transform, so the drawn rect *is* the outer rect and a
    /// screen point differs from a pane-local one only by the pane's corner.
    /// That is the situation #111 was reported in — two ordinary overlapping
    /// windows on the desktop, neither of them in a mode — and it keeps the
    /// arithmetic below readable enough to check by hand. A built decoration is
    /// assumed, which both windows in the report had.
    #[derive(Clone, Copy)]
    struct Stacked {
        outer: Rectangle<i32, Logical>,
        insets: Insets,
        /// Whether the client has arrived. `false` is a pane still loading: it
        /// is drawn and it covers, its frame's buttons work, and it has no
        /// window for a resize border to drag.
        window: bool,
        /// Whether the layout owns this pane. `false` is an X11 menu, tooltip
        /// or dropdown the client placed itself.
        managed: bool,
        /// Whether this pane paints anything. `false` is a pane held at opacity
        /// zero — which on this desktop means one between its close animation
        /// landing and its client acting on the request. It has a rectangle and
        /// it covers nothing.
        shows: bool,
    }

    impl Stacked {
        /// A framed window at a corner: a titlebar across the top and nothing
        /// else reserved, which is [`framed`]'s shape at an arbitrary place.
        fn window(at: (i32, i32), size: (i32, i32)) -> Self {
            Self {
                outer: Rectangle::new(at.into(), size.into()),
                insets: Insets {
                    top: TITLEBAR_HEIGHT,
                    ..Insets::NONE
                },
                window: true,
                managed: true,
                shows: true,
            }
        }

        /// The same pane with its application not yet arrived.
        const fn loading(mut self) -> Self {
            self.window = false;
            self
        }

        /// The same pane placed by its own client: a menu, a tooltip, a
        /// dropdown. It draws, so it covers; it is nothing's to resize or move.
        const fn unmanaged(mut self) -> Self {
            self.managed = false;
            self
        }

        /// The same pane held at opacity zero: asked to close and waiting on a
        /// client that has not answered. It is still in the stack, still the
        /// topmost thing at this rectangle, and on screen it is not there.
        const fn invisible(mut self) -> Self {
            self.shows = false;
            self
        }

        /// What [`Solium::pane_chrome`] makes of a point, out of the same two
        /// functions in the same order it uses them.
        ///
        /// [`chrome_offered`] and [`pane_hit_of`] are called here rather than
        /// reimplemented, which is the difference between a fixture and a
        /// second copy of the rule: `covers` is what `pane_chrome` passes —
        /// the *drawn* rect containing the point, which with no presentation
        /// transform is this rect — and both of `chrome_offered`'s gates apply
        /// exactly as they do there. A hand-rolled composition here is how the
        /// covers-before-chrome mistake could come back with every test still
        /// green.
        fn hit(self, point: (f64, f64)) -> PaneHit<Chrome> {
            let location = Point::<f64, Logical>::from(point);
            // Both halves of `Frame::covers`, in the same order and for the
            // same reason `pane_chrome` asks them: the rectangle says where the
            // pane would be drawn, and `shows` says whether it is drawn at all.
            let covers = self.shows && self.outer.to_f64().contains(location);
            let framed = covers
                && on_frame(
                    self.outer.size,
                    self.insets,
                    location - self.outer.loc.to_f64(),
                );
            pane_hit_of(
                chrome_offered(
                    self.shows,
                    self.managed,
                    self.window,
                    framed,
                    resize::border_edges(self.outer, location),
                ),
                covers,
            )
        }
    }

    /// The walk as it stood before #111: every pane's frame over the whole
    /// stack, and only then every pane's resize border, with a pane that merely
    /// *covers* the point stopping nothing.
    ///
    /// Kept rather than deleted so the bug is pinned and not only the fix. A
    /// test that asserts the new answer alone stays green against a walk that
    /// never occludes anything — which is how this fault survived #108's
    /// rewrite of the very same function, and why each test below measures both
    /// rules at the same point.
    ///
    /// `stack` is topmost-first, as [`Solium::chrome_under`]'s `rev` makes it.
    fn two_pass(stack: &[Stacked], point: (f64, f64)) -> Option<Chrome> {
        let claimed = |frames: bool| {
            stack.iter().find_map(|pane| match pane.hit(point) {
                // Drawn or not made no difference to this walk: it asked each
                // pane for chrome and took the first that answered.
                PaneHit::Chrome(Chrome::Frame) | PaneHit::Halo(Chrome::Frame) if frames => {
                    Some(Chrome::Frame)
                }
                PaneHit::Chrome(Chrome::Resize(edges)) | PaneHit::Halo(Chrome::Resize(edges))
                    if !frames =>
                {
                    Some(Chrome::Resize(edges))
                }
                _ => None,
            })
        };
        claimed(true).or_else(|| claimed(false))
    }

    /// The walk as `ab11731` first fixed #111: one pass, topmost first, with
    /// *any* chrome claim winning outright whether or not the pane claiming it
    /// draws anything at that point.
    ///
    /// The second control, kept for the same reason [`two_pass`] is. It got the
    /// covered titlebar right — that was the fix — and it got a halo over a
    /// lower pane's drawn chrome wrong, because "which window is on top" was
    /// asked at a pixel the upper window does not occupy. Every case below
    /// measures all three rules at the same point, so what each one gets right
    /// and wrong is written down rather than remembered.
    fn halo_wins(stack: &[Stacked], point: (f64, f64)) -> Option<Chrome> {
        for pane in stack {
            match pane.hit(point) {
                PaneHit::Chrome(chrome) | PaneHit::Halo(chrome) => return Some(chrome),
                PaneHit::Client => return None,
                PaneHit::Miss => {}
            }
        }
        None
    }

    /// **Issue #111, as reported: a titlebar took clicks through the window
    /// covering it.**
    ///
    /// Two overlapping windows. A press on the *top* one, at a point where the
    /// lower one's titlebar happened to lie underneath, focused and raised the
    /// lower window. The walk searched only for chrome, so the top window —
    /// whose client covers the point and which therefore had no chrome to
    /// offer — did not stop the descent, and the lower window's titlebar was
    /// found beneath it.
    ///
    /// The last assertion is the control, and it was run red before the fix:
    /// with the first assertion pointed at `two_pass` the test fails with
    /// `Some(Frame)` against an expected `None`, which is the reported
    /// behaviour reproduced in a unit test rather than on hardware.
    #[test]
    fn a_covered_titlebar_does_not_take_the_click() {
        let lower = Stacked::window((100, 100), (400, 300));
        let upper = Stacked::window((60, 60), (400, 300));
        let stack = [upper, lower];

        // (300, 116) is 56px down into the upper window: past its 32px titlebar
        // and 160px clear of its nearest resize border, so it is that window's
        // client and nothing else. The same point is 16px down into the lower
        // window, inside its titlebar and clear of its top border -- so the two
        // windows genuinely disagree here, which is what makes the walk's
        // answer worth anything.
        let point = (300.0, 116.0);
        assert_eq!(
            upper.hit(point),
            PaneHit::Client,
            "the top window covers this point with its client"
        );
        assert_eq!(
            lower.hit(point),
            PaneHit::Chrome(Chrome::Frame),
            "and the lower window's titlebar is underneath it, which is the \
             whole setup"
        );

        assert_eq!(
            topmost_chrome(stack.iter().map(|pane| pane.hit(point))),
            None,
            "a press on the top window's client is the client's, and the \
             titlebar buried under it may not have it"
        );
        assert_eq!(
            two_pass(&stack, point),
            Some(Chrome::Frame),
            "the walk this replaced hands the point to the covered titlebar, \
             which is the bug"
        );
        assert_eq!(
            halo_wins(&stack, point),
            None,
            "and the one-pass rule that replaced it got this case right -- \
             what it got wrong is the halo, below"
        );
    }

    /// **A resize border hanging outside its own window loses to anything a
    /// lower pane actually draws, and beats bare desktop.**
    ///
    /// A border reaches [`resize::RESIZE_BORDER`] pixels *outside* the window it
    /// belongs to, over whatever is behind. Out there the pane draws nothing, so
    /// the claim is not backed by a single pixel and "which window is on top"
    /// has no answer: an upper window's bottom edge floating four pixels above a
    /// lower window's close button is not on top of that button in any sense a
    /// user would recognise. It is beside it. The button is what is drawn there
    /// and the button takes the press.
    ///
    /// The halo still wins over the desktop, which is the only reason an edge
    /// can be grabbed from outside at all — and its *inside* half still wins
    /// outright, since there the pane does draw the pixel it is claiming.
    ///
    /// Three rules are measured at every point: the corrected one, the
    /// [`two_pass`] walk from before #111, and [`halo_wins`] as #111 was first
    /// fixed. The first case is the one that separates them.
    #[test]
    fn a_halo_loses_to_what_a_lower_pane_draws() {
        let lower = Stacked::window((100, 100), (400, 300));
        // Overlapping the top-right of the lower window, where its close button
        // is. The bottom edge, y = 120, floats inside the lower window's
        // titlebar band -- drawn, and 30px clear of the lower window's own
        // right border, so the only things meeting here are one window's empty
        // margin and another window's buttons.
        let upper = Stacked::window((300, 20), (200, 100));
        let stack = [upper, lower];

        let button = (470.0, 124.0);
        assert_eq!(
            upper.hit(button),
            PaneHit::Halo(Chrome::Resize(ResizeEdge::Bottom)),
            "4px below the upper window's bottom edge is its resize border, and \
             outside everything that window draws"
        );
        assert_eq!(
            lower.hit(button),
            PaneHit::Chrome(Chrome::Frame),
            "and 24px down into the lower window's titlebar, which is drawn"
        );
        assert_eq!(
            topmost_chrome(stack.iter().map(|pane| pane.hit(button))),
            Some(Chrome::Frame),
            "the titlebar is drawn there and the border is not, so the press is \
             the button's"
        );
        assert_eq!(
            two_pass(&stack, button),
            Some(Chrome::Frame),
            "which the walk from before #111 also answered -- it was not wrong \
             here, only wrong about why, having never asked which pane was on \
             top at all"
        );
        assert_eq!(
            halo_wins(&stack, button),
            Some(Chrome::Resize(ResizeEdge::Bottom)),
            "where #111's first fix drew NsResize over a visible close button \
             and started a resize grab on it"
        );

        // The same halo over a lower window's *client*, which is drawn just as
        // surely as its titlebar is. Nothing is offered and the client keeps
        // its own cursor: a window's margin does not reach through the window
        // under it.
        let deeper = Stacked::window((300, 20), (200, 180));
        let body = (470.0, 204.0);
        assert_eq!(
            deeper.hit(body),
            PaneHit::Halo(Chrome::Resize(ResizeEdge::Bottom))
        );
        assert_eq!(lower.hit(body), PaneHit::Client);
        assert_eq!(
            topmost_chrome([deeper, lower].iter().map(|pane| pane.hit(body))),
            None
        );
        assert_eq!(
            two_pass(&[deeper, lower], body),
            Some(Chrome::Resize(ResizeEdge::Bottom)),
            "and here the older walk is wrong too, so neither rule this \
             replaces got the halo right"
        );

        // Over the desktop the halo is the best claim there is, and it must
        // still win: this is what makes an edge grabbable from outside, and a
        // rule that demanded a pane draw what it claims would take every
        // window's outer border away.
        let sky = (400.0, 16.0);
        assert_eq!(
            upper.hit(sky),
            PaneHit::Halo(Chrome::Resize(ResizeEdge::Top)),
            "4px above the upper window's top edge"
        );
        assert_eq!(lower.hit(sky), PaneHit::Miss);
        assert_eq!(
            topmost_chrome(stack.iter().map(|pane| pane.hit(sky))),
            Some(Chrome::Resize(ResizeEdge::Top)),
            "nothing is drawn under the halo, so the halo has it"
        );
        assert_eq!(two_pass(&stack, sky), Some(Chrome::Resize(ResizeEdge::Top)));
        assert_eq!(
            halo_wins(&stack, sky),
            Some(Chrome::Resize(ResizeEdge::Top))
        );

        // And the half of that border that lies *inside* its own window is
        // chrome outright: the pane draws the pixel it is claiming, so it wins
        // over everything below and is never downgraded to a halo. A rule that
        // answered `Client` wherever the drawn rect contained the point would
        // have swallowed it and left every window resizable only from outside.
        let inside = (300.0, 355.0);
        let alone = Stacked::window((60, 60), (400, 300));
        assert!(
            alone
                .outer
                .to_f64()
                .contains(Point::<f64, Logical>::from(inside))
        );
        assert_eq!(
            alone.hit(inside),
            PaneHit::Chrome(Chrome::Resize(ResizeEdge::Bottom))
        );
        assert_eq!(
            topmost_chrome([alone, lower].iter().map(|pane| pane.hit(inside))),
            Some(Chrome::Resize(ResizeEdge::Bottom)),
            "and it beats the lower window it is drawn over, which is the half \
             of #111's fix that was right"
        );
    }

    /// **A halo over two panes takes the topmost one's, and a lower pane's
    /// border does not reach up through a window covering it.**
    ///
    /// The tie-break the corrected rule needs and the covered case it must not
    /// lose. Remembering a halo and carrying on is only safe if the *first* one
    /// is kept: two stacked windows whose edges both hang over the same strip
    /// of desktop are an ordinary sight, and the answer there is still the one
    /// on top.
    #[test]
    fn the_topmost_halo_is_the_one_kept() {
        // Two windows 6px apart with a strip of desktop between them: the
        // upper's bottom border and the lower's top border both hang over it,
        // and they name opposite edges, so which one the walk keeps is
        // legible in the answer.
        let upper = Stacked::window((100, 100), (200, 100));
        let lower = Stacked::window((100, 206), (200, 100));
        let below = (200.0, 202.0);

        assert_eq!(
            upper.hit(below),
            PaneHit::Halo(Chrome::Resize(ResizeEdge::Bottom)),
            "2px below the upper window"
        );
        assert_eq!(
            lower.hit(below),
            PaneHit::Halo(Chrome::Resize(ResizeEdge::Top)),
            "and 4px above the lower one"
        );
        assert_eq!(
            topmost_chrome([upper, lower].iter().map(|pane| pane.hit(below))),
            Some(Chrome::Resize(ResizeEdge::Bottom)),
            "two halos over the same desktop, and the topmost is the one kept"
        );
        assert_eq!(
            topmost_chrome([lower, upper].iter().map(|pane| pane.hit(below))),
            Some(Chrome::Resize(ResizeEdge::Top)),
            "stack them the other way and the answer follows the stacking order"
        );

        // A lower window's border, under a window that covers where it hangs.
        // The border is the lower window's own and the point is still the upper
        // window's client: a halo is a claim on the desktop, not a tunnel.
        let over = Stacked::window((60, 150), (400, 200));
        assert_eq!(over.hit(below), PaneHit::Client);
        assert_eq!(
            topmost_chrome([over, upper].iter().map(|pane| pane.hit(below))),
            None,
            "the covering window's client owns it, and the border reaching up \
             from underneath does not"
        );
    }

    /// **An unmanaged pane occludes and offers nothing.**
    ///
    /// An X11 menu, tooltip or dropdown is placed by its client and owned by
    /// it: `size_window` refuses an override-redirect surface, and nothing in
    /// the layout moves one. So a resize border eight pixels outside a Steam or
    /// GTK menu is a cursor promising a drag that cannot happen, and a press
    /// there starts a `ResizeGrab` that does nothing when it should have
    /// dismissed the menu. The single-pass walk made that worse before this
    /// gate: the phantom border beat a lower window's real titlebar.
    ///
    /// Covering is untouched, which is the half that was always right — the
    /// menu is drawn and a press on it is the menu's.
    #[test]
    fn an_unmanaged_pane_occludes_but_offers_no_chrome() {
        let lower = Stacked::window((100, 100), (400, 300));
        let menu = Stacked::window((300, 20), (200, 100)).unmanaged();
        let stack = [menu, lower];

        // The same point as the halo case above: 4px below the menu's bottom
        // edge, over the lower window's titlebar.
        let button = (470.0, 124.0);
        assert_eq!(
            menu.hit(button),
            PaneHit::Miss,
            "a menu has no resize border to offer, inside or out"
        );
        assert_eq!(
            topmost_chrome(stack.iter().map(|pane| pane.hit(button))),
            Some(Chrome::Frame),
            "so the titlebar under it is pressable, phantom border or not"
        );

        // And on the menu itself: covered, and the press is the client's.
        let on_menu = (400.0, 60.0);
        assert_eq!(menu.hit(on_menu), PaneHit::Client);
        assert_eq!(
            topmost_chrome(stack.iter().map(|pane| pane.hit(on_menu))),
            None,
            "a press on a menu is the menu's, which is what occluding means"
        );

        // The inside half of the border it would otherwise have offered is the
        // client's too, rather than a resize of something nothing may resize.
        let edge = (400.0, 116.0);
        assert_eq!(
            Stacked::window((300, 20), (200, 100)).hit(edge),
            PaneHit::Chrome(Chrome::Resize(ResizeEdge::Bottom)),
            "a managed window of the same shape would offer this"
        );
        assert_eq!(menu.hit(edge), PaneHit::Client);
    }

    /// **An invisible pane offers nothing and covers nothing** — the one kind
    /// that fails both of [`pane_hit_of`]'s questions at once.
    ///
    /// Issue #127's review finding 1 at the level of the rule rather than of
    /// the compositor. A pane between its close animation landing and its
    /// client acting on the request is held at opacity zero by a transform
    /// written `release: false`, for `CLOSING` plus the whole grace period. It
    /// is still in the stack and its rectangle still contains the point; it is
    /// simply not on screen.
    ///
    /// **The `Halo` is what makes this two gates and not one.** Suppressing
    /// only `covers` would turn the chrome this pane claims into
    /// [`PaneHit::Halo`] — the weakest claim, but a claim that survives the
    /// walk and wins wherever nothing lower paints. A closed window's titlebar
    /// and resize border would go on being pressable over bare desktop, which
    /// is a worse bug than the one being fixed because there is nothing on
    /// screen to explain it. Both questions are therefore asked, and every
    /// point is a `Miss`.
    ///
    /// Contrast [`an_unmanaged_pane_occludes_but_offers_no_chrome`]: a menu
    /// offers no chrome *and still covers*, because it is drawn. That is the
    /// distinction — offering is about what a press would mean, covering is
    /// about pixels, and this pane has no pixels.
    #[test]
    fn an_invisible_pane_offers_no_chrome_and_covers_nothing() {
        let lower = Stacked::window((100, 100), (400, 300));
        // Directly over the lower window, which is the situation a close
        // leaves behind once the layout has reflowed into the space.
        let closed = Stacked::window((100, 100), (400, 300)).invisible();
        let stack = [closed, lower];

        // On the lower window's titlebar, which the invisible pane's own
        // titlebar sits exactly on top of.
        let titlebar = (300.0, 110.0);
        assert_eq!(
            Stacked::window((100, 100), (400, 300)).hit(titlebar),
            PaneHit::Chrome(Chrome::Frame),
            "a visible pane of the same shape owns this point -- without which \
             the assertions below pass for want of a titlebar rather than for \
             want of a pane"
        );
        assert_eq!(
            closed.hit(titlebar),
            PaneHit::Miss,
            "an invisible pane's titlebar is not pressable"
        );
        assert_eq!(
            topmost_chrome(stack.iter().map(|pane| pane.hit(titlebar))),
            Some(Chrome::Frame),
            "so the press reaches the titlebar that is actually drawn there"
        );

        // The body, which is the click-to-focus case and the one that took the
        // keyboard with it.
        let body = (300.0, 250.0);
        assert_eq!(
            closed.hit(body),
            PaneHit::Miss,
            "and it occludes nothing, so the walk descends rather than \
             stopping at `PaneHit::Client`"
        );

        // The resize border, inside and out. Outside is the `Halo` this gate
        // exists to prevent.
        let inside = (300.0, 396.0);
        let outside = (300.0, 404.0);
        assert_eq!(
            Stacked::window((100, 100), (400, 300)).hit(outside),
            PaneHit::Halo(Chrome::Resize(ResizeEdge::Bottom)),
            "a visible pane of the same shape claims this as a halo, which is \
             the claim that must not survive being invisible"
        );
        assert_eq!(closed.hit(inside), PaneHit::Miss);
        assert_eq!(
            closed.hit(outside),
            PaneHit::Miss,
            "an edge nobody can see is an edge nobody can drag, and a halo \
             would have been honoured over bare desktop"
        );
    }

    /// A pane whose application has not arrived offers its frame and not its
    /// border, and covers either way.
    ///
    /// The `window.is_some()` half of [`chrome_offered`]: a frame around a
    /// loading window has working buttons, which is the point of drawing one,
    /// and there is nothing yet for a resize to resize. What it must not do is
    /// let the window behind it take the press, which is issue #111 again with
    /// a different pane on top.
    #[test]
    fn a_loading_pane_offers_its_frame_and_not_its_border() {
        let lower = Stacked::window((100, 100), (400, 300));
        let loading = Stacked::window((300, 20), (200, 100)).loading();
        let stack = [loading, lower];

        assert_eq!(
            loading.hit((400.0, 30.0)),
            PaneHit::Chrome(Chrome::Frame),
            "its titlebar is drawn and its buttons work"
        );
        assert_eq!(
            loading.hit((400.0, 116.0)),
            PaneHit::Client,
            "and its bottom border is not offered, so the point is simply \
             inside what it draws"
        );
        assert_eq!(
            loading.hit((470.0, 124.0)),
            PaneHit::Miss,
            "nor is the half of that border outside it"
        );
        assert_eq!(
            topmost_chrome(stack.iter().map(|pane| pane.hit((400.0, 80.0)))),
            None,
            "a press on a loading window's body is its own, not the window \
             behind it"
        );
    }

    /// **The cursor is composed from the same walk, so an occluded border does
    /// not promise a resize.**
    ///
    /// #108's finding one altitude up: the pointer's shape is read off
    /// [`claim_of`] rather than off any second hit test, so whatever the walk
    /// declines to claim is a point the compositor says nothing about. Without
    /// this the fix would be half-applied — presses landing correctly while the
    /// pointer went on describing the resize that the press no longer performs,
    /// which is exactly the symptom #108 was filed for.
    #[test]
    fn an_occluded_border_promises_no_resize_cursor() {
        let lower = Stacked::window((100, 100), (400, 300));
        let upper = Stacked::window((300, 20), (200, 100));
        let stack = [upper, lower];
        let cursor_at = |point| {
            claim_of(
                false,
                false,
                topmost_chrome(stack.iter().map(|pane| pane.hit(point))),
            )
            .cursor()
        };

        // Over the lower window's close button, under the upper window's halo:
        // the frame's own arrow, and not the resize the halo would have asked
        // for.
        assert_eq!(cursor_at((470.0, 124.0)), Some(CursorIcon::Default));
        assert_eq!(
            claim_of(false, false, halo_wins(&stack, (470.0, 124.0))).cursor(),
            Some(CursorIcon::NsResize),
            "which is the pointer #111's first fix drew over that button"
        );

        // Over the desktop above the upper window, where the halo is the whole
        // claim: the resize cursor, because the press really would resize.
        assert_eq!(cursor_at((400.0, 16.0)), Some(CursorIcon::NsResize));

        // And on the upper window's client, over the lower window's titlebar:
        // nothing at all, so the client's own cursor stands. This is #111's
        // point and the reason `chrome_under` is the only hit test.
        let covered = Stacked::window((60, 60), (400, 300));
        assert_eq!(
            claim_of(
                false,
                false,
                topmost_chrome([covered, lower].iter().map(|pane| pane.hit((300.0, 116.0)))),
            )
            .cursor(),
            None
        );
    }

    /// A point no window covers still descends, which is the case the fix must
    /// not break: [`PaneHit::Miss`] and [`PaneHit::Client`] are both "no chrome
    /// here" and only one of them may stop the walk.
    #[test]
    fn a_point_nothing_covers_still_descends() {
        let lower = Stacked::window((100, 100), (400, 300));
        let upper = Stacked::window((150, 20), (200, 100));
        let stack = [upper, lower];

        // 92px to the right of the upper window -- well past its border -- and
        // 10px down into the lower window's titlebar.
        let past = (450.0, 110.0);
        assert_eq!(upper.hit(past), PaneHit::Miss);
        assert_eq!(
            topmost_chrome(stack.iter().map(|pane| pane.hit(past))),
            Some(Chrome::Frame),
            "a window that is not there covers nothing, and the titlebar \
             beside it is still pressable"
        );
        assert_eq!(
            two_pass(&stack, past),
            Some(Chrome::Frame),
            "which the rule this replaced also got right -- only the covered \
             case differs"
        );

        // Bare desktop: every pane misses and the walk runs out.
        let desktop = (700.0, 700.0);
        assert_eq!(upper.hit(desktop), PaneHit::Miss);
        assert_eq!(lower.hit(desktop), PaneHit::Miss);
        assert_eq!(
            topmost_chrome(stack.iter().map(|pane| pane.hit(desktop))),
            None
        );
        assert_eq!(
            topmost_chrome(std::iter::empty::<PaneHit<Chrome>>()),
            None,
            "and a desktop with no windows on it at all"
        );
    }

    /// The tests that need a client on the other end of a socket.
    ///
    /// A bare `WlSurface` is not enough for any of them: `Window` only wraps a
    /// real `ToplevelSurface`, and Smithay gives no way to fabricate one except
    /// a client asking for it over the wire. Anything that is *state a client
    /// sets* -- the scale it was told, the parent it named, the modal flag it
    /// raised -- can only be reached from this side. `wl-probe` (see its own
    /// `Cargo.toml`) exists in this workspace for the identical reason, and its
    /// dependencies are what make this affordable here: `wayland-client` and
    /// `wayland-protocols`'s `client` feature were already in the lockfile.
    ///
    /// The module was one test's and was named after it. It is two now, and the
    /// fixture was always the expensive part of it.
    mod real_client {
        use super::*;
        use smithay::output::{Mode, PhysicalProperties, Subpixel};
        use smithay::reexports::wayland_server::Display;
        use std::os::unix::io::{AsFd, OwnedFd};
        use std::os::unix::net::UnixStream;
        use wayland_client::protocol::{
            wl_buffer, wl_callback, wl_compositor, wl_data_device, wl_data_device_manager,
            wl_data_offer, wl_keyboard, wl_output, wl_registry, wl_seat, wl_shm, wl_shm_pool,
            wl_surface,
        };
        use wayland_client::{Connection, Dispatch, QueueHandle};
        use wayland_protocols::ext::session_lock::v1::client::{
            ext_session_lock_manager_v1, ext_session_lock_surface_v1, ext_session_lock_v1,
        };
        use wayland_protocols::xdg::dialog::v1::client::{xdg_dialog_v1, xdg_wm_dialog_v1};
        use wayland_protocols::xdg::shell::client::{
            xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base,
        };
        use wayland_protocols_wlr::layer_shell::v1::client::{
            zwlr_layer_shell_v1, zwlr_layer_surface_v1,
        };
        use wayland_protocols_wlr::screencopy::v1::client::{
            zwlr_screencopy_frame_v1, zwlr_screencopy_manager_v1,
        };

        /// The `edge_at` a `ResizeGrab` would record on the frame it produced
        /// `wanted`, for a pane the layout has at that same rectangle.
        ///
        /// These fixtures build a `ResizeRequest` by hand, because what is under
        /// test is what `settle_resize` does with one rather than how a grab
        /// assembles one. It still has to be a payload a grab could *have*
        /// assembled. Spelled `(wanted.loc.x, wanted.loc.y)` it was not: that is
        /// the top-left corner whatever the edges say, so every `Right`,
        /// `Bottom` and `BottomRight` case carried a pair
        /// `crate::input::resize::dragged_edge` cannot produce. Nothing read it
        /// -- every one of these takes the floating path, where no layout is
        /// offered the drag at all -- which is exactly why it would have sat
        /// there until a tiled test was written against it and believed.
        ///
        /// Built by the real function for that reason, rather than by a second
        /// copy of its rules here.
        fn payload_edge(wanted: Rectangle<i32, Logical>, edges: ResizeEdge) -> (f64, f64) {
            // The pointer, for an axis this drag has no hold of. Its centre is
            // as good as anywhere: the value is passed straight through and
            // these fixtures never look at it.
            let pointer: Point<f64, Logical> = (
                f64::from(wanted.loc.x) + f64::from(wanted.size.w) / 2.0,
                f64::from(wanted.loc.y) + f64::from(wanted.size.h) / 2.0,
            )
                .into();
            crate::input::resize::dragged_edge(
                crate::input::resize::LaidOut(wanted),
                edges,
                pointer,
                pointer,
            )
        }

        /// The client side of the fixture. Binds exactly the globals a window
        /// needs and nothing else -- there is no renderer on this end to answer
        /// anything more, and none of what follows needs one.
        #[derive(Debug, Default)]
        struct Client {
            compositor: Option<wl_compositor::WlCompositor>,
            wm_base: Option<xdg_wm_base::XdgWmBase>,
            shm: Option<wl_shm::WlShm>,
            /// The global #72 added. Bound here because "the client said modal"
            /// is not a thing the server side can say on a client's behalf --
            /// which is the whole reason this test is in this module.
            dialogs: Option<xdg_wm_dialog_v1::XdgWmDialogV1>,
            /// For [`bar`]: the one way to make a monitor's work area smaller
            /// than the monitor, which is what tells a maximised window from a
            /// fullscreen one.
            layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
            /// Every `xdg_toplevel.configure` this client has been sent, with
            /// the toplevel it was sent to.
            ///
            /// The *rate* a client is configured at is the subject of #123 and
            /// there is no way to see it from the server side: `size_window`
            /// hands a size to Smithay and Smithay decides whether that is a
            /// change worth a wire message. Counting them here counts what the
            /// client actually receives, which is the only number the issue is
            /// about — and it is also what keeps a test honest, because a
            /// compositor that sends the same size sixty times a second looks
            /// identical from its own side and costs nothing on the wire.
            configures: Vec<(wayland_client::backend::ObjectId, i32, i32)>,

            /// The monitor, the lock manager and the clipboard, for
            /// `lock_focus`: the only tests that need a surface to lock a
            /// screen with, or a data device. Bound by every client and used by
            /// none of the others, which is harmless -- binding a global asks
            /// for nothing until a request is made on it. The seat, which
            /// `lock_focus` needs as well, is with the keyboard below.
            output: Option<wl_output::WlOutput>,
            /// Every monitor, where `output` is the last one bound: a lock
            /// client covers them all.
            outputs: Vec<wl_output::WlOutput>,
            locks: Option<ext_session_lock_manager_v1::ExtSessionLockManagerV1>,
            data_devices: Option<wl_data_device_manager::WlDataDeviceManager>,
            screencopy: Option<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1>,
            /// Every `wl_keyboard.key` this client has been sent, as evdev
            /// codes, presses and releases alike. What the lock tests are
            /// about in the end: a key that reaches an application's client
            /// is a key the application has.
            keys: Vec<u32>,
            /// The same keys with whether each was a press, for the one
            /// question `keys` cannot answer: whether a key the client was
            /// told went down was ever told it came up.
            key_events: Vec<(u32, bool)>,
            /// The keys the last `wl_keyboard.enter` said were already held.
            enter_keys: Vec<u32>,
            /// The lock objects this client was told `locked` on, and the
            /// ones it was told `finished` on.
            locked: Vec<wayland_client::backend::ObjectId>,
            finished: Vec<wayland_client::backend::ObjectId>,
            /// Screen captures this client was offered a buffer for, and ones
            /// it was told failed.
            captures_offered: usize,
            captures_failed: usize,
            /// The surface this client's keyboard is on, as its own
            /// `enter`/`leave` events say -- the client's view, which is the
            /// one that matters, and not the server's.
            keyboard_on: Option<wayland_client::backend::ObjectId>,
            /// The serial of the last `wl_keyboard.enter`, for a menu to grab
            /// with the way a real one does.
            serial: u32,
            /// How many `wl_data_device.selection` events have arrived. One is
            /// sent each time this client is made the clipboard's client, so a
            /// count that moves while the session is locked is an application
            /// behind the lock being handed the clipboard.
            selections: usize,
            /// How many `xdg_popup.popup_done` events have arrived.
            popups_done: usize,
            /// Every `xdg_toplevel.close` this client has been sent, with the
            /// toplevel it was sent to.
            ///
            /// Counted here for the same reason `configures` is: `send_close`
            /// is a call the server makes into Smithay, and from the server's
            /// own side a compositor that asks twice looks exactly like one
            /// that asks once. Issue #127's second fault is precisely a second
            /// request going out, so the only honest place to count is the end
            /// that receives them.
            closes: Vec<wayland_client::backend::ObjectId>,
            /// The seat and its keyboard, bound so that a test can ask where
            /// typing *went* rather than where focus was set.
            ///
            /// The distinction is the whole of #127's second review, finding 1:
            /// a compositor that leaves focus on a window nobody can see is
            /// indistinguishable, from its own side, from one that moved it —
            /// the seat is perfectly happy to hold a surface that is drawn at
            /// opacity zero, and `give_keyboard`'s `true` says only that the
            /// lock's rule allowed it, not that anyone can see the surface. The
            /// end that receives the keystrokes is the only one that can say,
            /// which is why `typing_after_a_close_reaches_the_window_that_is_drawn`
            /// asserts here and not at the seat.
            ///
            /// The keyboard is the one the seat's handler asks for, and the
            /// only one: see that handler.
            seat: Option<wl_seat::WlSeat>,
            keyboard: Option<wl_keyboard::WlKeyboard>,
            /// The surface this client currently has keyboard focus on, as
            /// `wl_keyboard.enter` and `.leave` report it.
            ///
            /// Kept as the protocol id rather than the proxy, because that is
            /// the one number both ends of this fixture agree on: a
            /// `wl_surface` is created by the client and carries the same id in
            /// the server's object map, so a test holding only the server's
            /// `Window` can still say which surface this was.
            entered: Option<u32>,
            /// Every key this client was sent, paired with the surface it was
            /// focused on when it arrived.
            ///
            /// The pairing is the point. `wl_keyboard.key` carries no surface —
            /// it goes wherever the last `enter` put the focus — so "which
            /// window did this character reach" is a question only a client
            /// tracking both events can answer, and it is exactly the question
            /// the user asks when they close a window and keep typing.
            typed: Vec<(Option<u32>, u32)>,
        }

        impl Dispatch<wl_registry::WlRegistry, ()> for Client {
            fn event(
                state: &mut Self,
                registry: &wl_registry::WlRegistry,
                event: wl_registry::Event,
                (): &(),
                _conn: &Connection,
                qh: &QueueHandle<Self>,
            ) {
                let wl_registry::Event::Global {
                    name, interface, ..
                } = event
                else {
                    return;
                };
                match interface.as_str() {
                    "wl_compositor" => state.compositor = Some(registry.bind(name, 1, qh, ())),
                    "xdg_wm_base" => state.wm_base = Some(registry.bind(name, 1, qh, ())),
                    "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
                    "xdg_wm_dialog_v1" => state.dialogs = Some(registry.bind(name, 1, qh, ())),
                    // Version 5 rather than the newest: everything below needs
                    // `enter`, `leave` and `key`, which are version 1, and the
                    // lower the bind the fewer ways this fixture can stop
                    // matching the compositor's advertised version.
                    "wl_seat" => state.seat = Some(registry.bind(name, 5, qh, ())),
                    "wl_output" => {
                        let output: wl_output::WlOutput = registry.bind(name, 1, qh, ());
                        state.outputs.push(output.clone());
                        state.output = Some(output);
                    }
                    "zwlr_screencopy_manager_v1" => {
                        state.screencopy = Some(registry.bind(name, 3, qh, ()));
                    }
                    "ext_session_lock_manager_v1" => {
                        state.locks = Some(registry.bind(name, 1, qh, ()));
                    }
                    "wl_data_device_manager" => {
                        state.data_devices = Some(registry.bind(name, 1, qh, ()));
                    }
                    "zwlr_layer_shell_v1" => {
                        state.layer_shell = Some(registry.bind(name, 1, qh, ()));
                    }
                    _ => {}
                }
            }
        }

        /// The keyboard is taken as soon as the seat says it has one.
        ///
        /// Here rather than at a call site, because capabilities arrive
        /// asynchronously: a test that asked for the keyboard at the moment it
        /// wanted to type would be asking before the registry round trip that
        /// `connect` ends with had delivered this event.
        ///
        /// **Here and nowhere else**, so that a client has one keyboard. A
        /// second one -- `lock_focus`'s `Side::connect` asked for its own
        /// before the two suites met -- is sent every event again, and a
        /// client with two records every key twice.
        /// `a_mode_active_at_the_lock_does_not_garble_the_password` asserts
        /// the lock client's exact key sequence, and fails on the doubling.
        impl Dispatch<wl_seat::WlSeat, ()> for Client {
            fn event(
                state: &mut Self,
                seat: &wl_seat::WlSeat,
                event: wl_seat::Event,
                (): &(),
                _conn: &Connection,
                qh: &QueueHandle<Self>,
            ) {
                let wl_seat::Event::Capabilities { capabilities } = event else {
                    return;
                };
                let has_keyboard = capabilities
                    .into_result()
                    .is_ok_and(|capabilities| capabilities.contains(wl_seat::Capability::Keyboard));
                if has_keyboard && state.keyboard.is_none() {
                    state.keyboard = Some(seat.get_keyboard(qh, ()));
                }
            }
        }

        /// Where the typing went, as both suites ask it: see [`Client::keys`]
        /// and [`Client::keyboard_on`] for `lock_focus`, and [`Client::typed`]
        /// for #127's.
        ///
        /// The keymap fd is dropped rather than read: nothing here interprets
        /// keysyms, and the assertion is about which surface received a key
        /// rather than which character it was.
        impl Dispatch<wl_keyboard::WlKeyboard, ()> for Client {
            fn event(
                state: &mut Self,
                _keyboard: &wl_keyboard::WlKeyboard,
                event: wl_keyboard::Event,
                (): &(),
                _conn: &Connection,
                _qh: &QueueHandle<Self>,
            ) {
                match event {
                    wl_keyboard::Event::Enter {
                        serial,
                        surface,
                        keys,
                    } => {
                        let id = wayland_client::Proxy::id(&surface);
                        state.entered = Some(id.protocol_id());
                        state.keyboard_on = Some(id);
                        state.serial = serial;
                        state.enter_keys = keys
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .map(|key| u32::from_ne_bytes(*key))
                            .collect();
                    }
                    wl_keyboard::Event::Leave { .. } => {
                        state.entered = None;
                        state.keyboard_on = None;
                    }
                    wl_keyboard::Event::Key {
                        key,
                        state: key_state,
                        ..
                    } => {
                        state.keys.push(key);
                        state.key_events.push((
                            key,
                            key_state
                                == wayland_client::WEnum::Value(wl_keyboard::KeyState::Pressed),
                        ));
                        state.typed.push((state.entered, key));
                    }
                    _ => {}
                }
            }
        }

        wayland_client::delegate_noop!(Client: ignore xdg_wm_dialog_v1::XdgWmDialogV1);
        wayland_client::delegate_noop!(Client: ignore xdg_dialog_v1::XdgDialogV1);
        wayland_client::delegate_noop!(Client: ignore wl_compositor::WlCompositor);
        wayland_client::delegate_noop!(Client: ignore wl_surface::WlSurface);
        wayland_client::delegate_noop!(Client: ignore wl_shm::WlShm);
        wayland_client::delegate_noop!(Client: ignore wl_shm_pool::WlShmPool);
        wayland_client::delegate_noop!(Client: ignore wl_buffer::WlBuffer);
        wayland_client::delegate_noop!(Client: ignore xdg_wm_base::XdgWmBase);
        wayland_client::delegate_noop!(Client: ignore xdg_surface::XdgSurface);
        wayland_client::delegate_noop!(Client: ignore wl_callback::WlCallback);
        wayland_client::delegate_noop!(Client: ignore wl_output::WlOutput);
        wayland_client::delegate_noop!(Client: ignore wl_data_device_manager::WlDataDeviceManager);
        wayland_client::delegate_noop!(Client: ignore wl_data_offer::WlDataOffer);
        wayland_client::delegate_noop!(Client: ignore xdg_positioner::XdgPositioner);
        wayland_client::delegate_noop!(
            Client: ignore ext_session_lock_manager_v1::ExtSessionLockManagerV1
        );
        wayland_client::delegate_noop!(
            Client: ignore zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1
        );

        /// See [`Client::locked`] and [`Client::finished`].
        impl Dispatch<ext_session_lock_v1::ExtSessionLockV1, ()> for Client {
            fn event(
                state: &mut Self,
                lock: &ext_session_lock_v1::ExtSessionLockV1,
                event: ext_session_lock_v1::Event,
                _data: &(),
                _conn: &Connection,
                _qh: &QueueHandle<Self>,
            ) {
                let id = wayland_client::Proxy::id(lock);
                match event {
                    ext_session_lock_v1::Event::Locked => state.locked.push(id),
                    ext_session_lock_v1::Event::Finished => state.finished.push(id),
                    _ => {}
                }
            }
        }

        /// See [`Client::captures_offered`].
        impl Dispatch<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, ()> for Client {
            fn event(
                state: &mut Self,
                _frame: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
                event: zwlr_screencopy_frame_v1::Event,
                _data: &(),
                _conn: &Connection,
                _qh: &QueueHandle<Self>,
            ) {
                match event {
                    zwlr_screencopy_frame_v1::Event::Buffer { .. } => state.captures_offered += 1,
                    zwlr_screencopy_frame_v1::Event::Failed => state.captures_failed += 1,
                    _ => {}
                }
            }
        }

        wayland_client::delegate_noop!(
            Client: ignore ext_session_lock_surface_v1::ExtSessionLockSurfaceV1
        );

        /// See [`Client::selections`].
        impl Dispatch<wl_data_device::WlDataDevice, ()> for Client {
            fn event(
                state: &mut Self,
                _device: &wl_data_device::WlDataDevice,
                event: wl_data_device::Event,
                _data: &(),
                _conn: &Connection,
                _qh: &QueueHandle<Self>,
            ) {
                if let wl_data_device::Event::Selection { .. } = event {
                    state.selections += 1;
                }
            }

            // An offer is a new object the server creates, so the client has to
            // be told what to make of one even though no test sets a selection
            // and none should ever arrive.
            wayland_client::event_created_child!(Client, wl_data_device::WlDataDevice, [
                wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, ()),
            ]);
        }

        /// See [`Client::popups_done`].
        impl Dispatch<xdg_popup::XdgPopup, ()> for Client {
            fn event(
                state: &mut Self,
                _popup: &xdg_popup::XdgPopup,
                event: xdg_popup::Event,
                _data: &(),
                _conn: &Connection,
                _qh: &QueueHandle<Self>,
            ) {
                if let xdg_popup::Event::PopupDone = event {
                    state.popups_done += 1;
                }
            }
        }
        wayland_client::delegate_noop!(Client: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
        wayland_client::delegate_noop!(Client: ignore zwlr_layer_surface_v1::ZwlrLayerSurfaceV1);

        /// The two events this fixture does not ignore. See
        /// [`Client::configures`] and [`Client::closes`].
        ///
        /// Configures are deliberately not acked:
        /// `ToplevelSurface::send_pending_configure` compares against the last
        /// configure it *sent*, not the last one a client acknowledged, so a
        /// fixture that never acks still sees exactly the deduplication a real
        /// client would.
        ///
        /// A close is deliberately not acted on either, and that is the whole
        /// of what makes this client a stand-in for a slow one. `close` is a
        /// request with no reply in the protocol; a client honours it by
        /// destroying its toplevel, and one that has not done so *yet* is
        /// indistinguishable from one that has decided not to. This fixture
        /// never destroys anything, so it is both — which is what issue #127's
        /// third fault is about.
        impl Dispatch<xdg_toplevel::XdgToplevel, ()> for Client {
            fn event(
                state: &mut Self,
                toplevel: &xdg_toplevel::XdgToplevel,
                event: xdg_toplevel::Event,
                _data: &(),
                _conn: &Connection,
                _qh: &QueueHandle<Self>,
            ) {
                match event {
                    xdg_toplevel::Event::Configure { width, height, .. } => state
                        .configures
                        .push((wayland_client::Proxy::id(toplevel), width, height)),
                    xdg_toplevel::Event::Close => {
                        state.closes.push(wayland_client::Proxy::id(toplevel));
                    }
                    _ => {}
                }
            }
        }

        /// An anonymous, already-unlinked file of `size` bytes -- enough for a
        /// client to back a `wl_shm_pool` with. Its contents are never read:
        /// nothing in this fixture renders. Only its *size* matters, because
        /// that is what gives the mapped `Window` a real, non-zero bounding
        /// box instead of the `Rectangle::zero()` an uncommitted surface has,
        /// which overlaps no output at all and so would never appear in
        /// `elements_for_output` for either monitor below.
        fn anon_file(size: i32) -> OwnedFd {
            let path = std::env::temp_dir().join(format!(
                "solium-scale-resend-test-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&path)
                .expect("creating a backing file for a test wl_shm_pool");
            std::fs::remove_file(&path).expect("unlinking the test shm file");
            file.set_len(u64::from(size.unsigned_abs()))
                .expect("sizing the test shm file");
            file.into()
        }

        /// A client on the other end of a socketpair, with every global this
        /// compositor has already in its registry.
        ///
        /// The queue comes back rather than a handle to it, because a
        /// `QueueHandle` borrows its queue and the caller has to own one.
        fn connect(
            display: &mut Display<Solium>,
            state: &mut Solium,
        ) -> (Connection, wayland_client::EventQueue<Client>, Client) {
            let (server_side, client_side) = UnixStream::pair().expect("a socketpair");
            display
                .handle()
                .insert_client(server_side, std::sync::Arc::new(ClientState::default()))
                .expect("inserting the test client");
            let conn = Connection::from_socket(client_side).expect("wrapping the client socket");
            let mut event_queue = conn.new_event_queue::<Client>();
            let qh = event_queue.handle();
            let mut client = Client::default();

            conn.display().get_registry(&qh, ());
            conn.flush().expect("flushing get_registry");
            display
                .dispatch_clients(state)
                .expect("dispatching get_registry");
            display
                .flush_clients()
                .expect("flushing the registry snapshot");
            // The one blocking read in this fixture: it is safe because the
            // server has, on the line above, already written the response --
            // every global this compositor has -- onto the socket. Blocking
            // here waits on bytes that are already in the kernel buffer, not
            // on the server, which nothing is driving but this same thread.
            event_queue
                .blocking_dispatch(&mut client)
                .expect("reading the registry snapshot");

            (conn, event_queue, client)
        }

        /// Opens one window through the real protocol: a surface, an
        /// `xdg_toplevel`, and a tiny committed buffer, so `new_toplevel` maps
        /// a `Window` with a real, non-zero bounding box at `(0, 0)` -- where
        /// every window is first mapped; the caller repositions it from
        /// there. Returns the newly-mapped `Window`.
        ///
        /// The client's own `xdg_toplevel` comes back with it, because some of
        /// what a window is only exists on that side: `set_parent` and the modal
        /// flag are both requests, and there is no way to ask for them except as
        /// the client.
        fn open_window(
            display: &mut Display<Solium>,
            state: &mut Solium,
            conn: &Connection,
            client: &Client,
            qh: &QueueHandle<Client>,
        ) -> (Window, xdg_toplevel::XdgToplevel) {
            let (window, toplevel, _surface) = open_surface(display, state, conn, client, qh);
            (window, toplevel)
        }

        /// The same, handing back the `wl_surface` as well.
        ///
        /// For the one thing no other fixture has needed: making the client
        /// *answer* a configure with a size of its own. A window's size is its
        /// surface's committed bounding box, so the only way to change it is to
        /// attach another buffer from this side — which is exactly what a client
        /// refusing a resize does, and refusal is the trap `crate::resizing` is
        /// built around. Dropping a `wayland-client` proxy sends nothing, so the
        /// callers that do not want it are unaffected.
        fn open_surface(
            display: &mut Display<Solium>,
            state: &mut Solium,
            conn: &Connection,
            client: &Client,
            qh: &QueueHandle<Client>,
        ) -> (Window, xdg_toplevel::XdgToplevel, wl_surface::WlSurface) {
            let (window, toplevel, surface, _xdg_surface) =
                open_xdg(display, state, conn, client, qh);
            (window, toplevel, surface)
        }

        /// The same again, with the `xdg_surface` too: a menu names its parent
        /// by that, and there is no other way to open one.
        fn open_xdg(
            display: &mut Display<Solium>,
            state: &mut Solium,
            conn: &Connection,
            client: &Client,
            qh: &QueueHandle<Client>,
        ) -> (
            Window,
            xdg_toplevel::XdgToplevel,
            wl_surface::WlSurface,
            xdg_surface::XdgSurface,
        ) {
            let compositor = client.compositor.clone().expect("wl_compositor bound");
            let wm_base = client.wm_base.clone().expect("xdg_wm_base bound");

            let before: Vec<Window> = state.space.elements().cloned().collect();

            let surface = compositor.create_surface(qh, ());
            let xdg_surface = wm_base.get_xdg_surface(&surface, qh, ());
            let toplevel = xdg_surface.get_toplevel(qh, ());

            const SIDE: i32 = 64;
            commit_buffer(client, qh, &surface, SIDE, SIDE);

            conn.flush().expect("flushing the window-open requests");
            display
                .dispatch_clients(state)
                .expect("dispatching the window-open requests");

            let window = state
                .space
                .elements()
                .find(|window| !before.contains(window))
                .cloned()
                .expect("new_toplevel mapped a window");
            (window, toplevel, surface, xdg_surface)
        }

        /// Attach a buffer of exactly this size and commit it.
        ///
        /// A `Window`'s geometry is its surface's committed bounding box — this
        /// fixture never calls `set_window_geometry` — so this *is* how a client
        /// says what size it has become, and how it says it has become a
        /// different one from the one it was asked for.
        fn commit_buffer(
            client: &Client,
            qh: &QueueHandle<Client>,
            surface: &wl_surface::WlSurface,
            width: i32,
            height: i32,
        ) {
            let shm = client.shm.clone().expect("wl_shm bound");
            let stride = width * 4;
            let bytes = stride * height;
            let fd = anon_file(bytes);
            let pool = shm.create_pool(fd.as_fd(), bytes, qh, ());
            let buffer =
                pool.create_buffer(0, width, height, stride, wl_shm::Format::Argb8888, qh, ());
            surface.attach(Some(&buffer), 0, 0);
            // Without this the server keeps the *old* buffer's damage and the
            // bounding box does not move: an attach is a promise and a commit is
            // the moment it counts.
            surface.damage(0, 0, width, height);
            surface.commit();
        }

        /// One full round trip: the client's requests to the server, the
        /// server's events back to the client.
        ///
        /// **The `sync` is what makes the read safe.** `blocking_dispatch` waits
        /// on the socket and nothing but this thread drives the server, so a
        /// read issued when the server happened to have written nothing would
        /// hang the test binary for ever — and "the server wrote nothing" is
        /// precisely the assertion a throttle test is trying to make. A
        /// `wl_display.sync` guarantees at least the `done` event, so the read
        /// always returns and what it returns is *whatever else* was queued.
        fn pump(
            display: &mut Display<Solium>,
            state: &mut Solium,
            conn: &Connection,
            qh: &QueueHandle<Client>,
            queue: &mut wayland_client::EventQueue<Client>,
            client: &mut Client,
        ) {
            conn.display().sync(qh, ());
            conn.flush().expect("flushing the round trip");
            display
                .dispatch_clients(state)
                .expect("dispatching the round trip");
            display
                .flush_clients()
                .expect("flushing the server's events");
            queue
                .blocking_dispatch(client)
                .expect("reading the server's events");
        }

        /// **Issue #99: a rescale left already-open windows blurry.**
        ///
        /// Two protocols tell a client what scale to draw at. `commit`'s call to
        /// `send_surface_state` resends `wl_surface.preferred_buffer_scale` on
        /// every commit, so an existing client picks up a new output scale the
        /// moment it next draws. `new_fractional_scale` answers the other one,
        /// `wp_fractional_scale_v1`, but only when a client asks -- once, ever,
        /// per surface, and nothing called it again when `scale_outputs` changed
        /// a monitor's scale later. A window opened before a `super+shift+r`
        /// rescale kept the scale it was told at startup, and the compositor
        /// upscaled its buffer to fill the larger area the new scale gave it.
        ///
        /// The scenario it describes: two monitors, a `super+shift+r` rescale of
        /// one of them, and a window already open on each.
        #[test]
        fn changed_output_resends_fractional_scale_and_unchanged_output_does_not() {
            let mut display = Display::<Solium>::new().expect("creating a test wayland display");
            let mut state = Solium::new(display.handle());

            // No decoration, and that is load-bearing rather than tidiness.
            //
            // This fixture is the only test in the tree that drives a *real*
            // wayland-client, and a window that never negotiates
            // `xdg_decoration` now has a frame built for it on first show
            // (#103) -- so its two windows would each construct a Qt scene.
            // Doing that in a process already holding a raw libwayland
            // connection of its own aborts the whole test binary: SIGABRT,
            // nothing on stderr even under `--nocapture`, taking every test
            // after it down as well.
            //
            // It surfaced only when #103 and this fixture first met in one
            // build, each having been green on its own branch. `bare()`
            // short-circuits `Decorations::insert` before it reaches Qt, so
            // this asks for the one thing the fixture does not need and
            // cannot survive.
            //
            // Nothing is weakened by it. The subject here is whether a scale
            // change reaches surfaces that are already open; a frame has no
            // part in that, and the two assertions are untouched.
            state
                .decorations
                .set_style(&mut state.panes, Some("none".to_string()));

            // Two monitors side by side, both starting at 1x -- the state
            // they would be in right after `place_outputs` first ran.
            let output_a = Output::new(
                "scale-resend-test-a".to_string(),
                PhysicalProperties {
                    size: (0, 0).into(),
                    subpixel: Subpixel::Unknown,
                    make: "solium".to_string(),
                    model: "test-a".to_string(),
                },
            );
            output_a.change_current_state(
                Some(Mode {
                    size: (1920, 1080).into(),
                    refresh: 60_000,
                }),
                None,
                Some(Scale::Fractional(1.0)),
                None,
            );
            state.space.map_output(&output_a, (0, 0));

            let output_b = Output::new(
                "scale-resend-test-b".to_string(),
                PhysicalProperties {
                    size: (0, 0).into(),
                    subpixel: Subpixel::Unknown,
                    make: "solium".to_string(),
                    model: "test-b".to_string(),
                },
            );
            output_b.change_current_state(
                Some(Mode {
                    size: (1920, 1080).into(),
                    refresh: 60_000,
                }),
                None,
                Some(Scale::Fractional(1.0)),
                None,
            );
            state.space.map_output(&output_b, (1920, 0));

            // A real client -- see the module doc comment for why.
            let (conn, event_queue, client) = connect(&mut display, &mut state);
            let qh = event_queue.handle();

            let (window_a, _toplevel_a) =
                open_window(&mut display, &mut state, &conn, &client, &qh);
            let (window_b, _toplevel_b) =
                open_window(&mut display, &mut state, &conn, &client, &qh);

            // Placed explicitly, one per monitor: `new_toplevel` maps every
            // window at `(0, 0)`, and where the pane system fits it from
            // there depends on a layout this test does not configure.
            state.space.map_element(window_a.clone(), (100, 100), false);
            state
                .space
                .map_element(window_b.clone(), (2100, 100), false);
            // `elements_for_output` reads overlap data that only
            // `Space::refresh` computes -- see its own doc comment. The real
            // backends call it once a frame, for the same reason.
            state.space.refresh();

            let surface_a = window_a
                .wl_surface()
                .expect("window a has a surface")
                .into_owned();
            let surface_b = window_b
                .wl_surface()
                .expect("window b has a surface")
                .into_owned();

            let preferred_scale = |surface: &WlSurface| {
                with_states(surface, |states| {
                    with_fractional_scale(states, |fractional| fractional.preferred_scale())
                })
            };

            assert_eq!(
                preferred_scale(&surface_a),
                None,
                "neither window has asked for a fractional scale yet, so \
                 neither should have one before the rescale"
            );
            assert_eq!(preferred_scale(&surface_b), None);

            // The rescale: `a`'s monitor is asked for 2x, as if someone had
            // just edited it in and pressed `super+shift+r`. `b`'s monitor is
            // asked for exactly the scale it already has.
            state.arrangement = crate::monitor::Arrangement::new(vec![
                crate::monitor::Placement {
                    name: "scale-resend-test-a".to_string(),
                    at: None,
                    beside: None,
                    mode: crate::monitor::Wanted::default(),
                    vrr: None,
                    transform: None,
                    enabled: true,
                    primary: false,
                    scale: crate::monitor::Scaling::Fixed(2.0),
                },
                crate::monitor::Placement {
                    name: "scale-resend-test-b".to_string(),
                    at: None,
                    beside: None,
                    mode: crate::monitor::Wanted::default(),
                    vrr: None,
                    transform: None,
                    enabled: true,
                    primary: false,
                    scale: crate::monitor::Scaling::Fixed(1.0),
                },
            ]);
            state.scale_outputs();

            assert_eq!(
                preferred_scale(&surface_a),
                Some(2.0),
                "a's monitor actually changed scale (1x to 2x), so a real \
                 client's wp_fractional_scale_v1 object -- bound once, at \
                 startup, and never asked again -- must be told the new \
                 value, or it keeps drawing at the old one forever. This is \
                 issue #99."
            );
            assert_eq!(
                preferred_scale(&surface_b),
                None,
                "b's monitor was asked for exactly the scale it already had, \
                 so scale_outputs's own early `continue` means this function \
                 never runs for it at all -- and separately, b's window was \
                 never on the monitor that changed, so it must be untouched \
                 even if it had been"
            );
        }

        /// One 1920x1080 monitor at the origin, at 1x.
        ///
        /// Fullscreen fills the monitor a window is on and maximise fills its
        /// work area, so both need one to exist; with no layer surfaces on it,
        /// the work area is the whole monitor.
        fn one_screen(state: &mut Solium) -> Output {
            a_screen(state, "restore-test", (0, 0))
        }

        /// The last size this toplevel was configured with, as the client saw it.
        fn last_configured(
            client: &Client,
            toplevel: &xdg_toplevel::XdgToplevel,
        ) -> Option<(i32, i32)> {
            let id = wayland_client::Proxy::id(toplevel);
            client
                .configures
                .iter()
                .rev()
                .find(|(to, _, _)| *to == id)
                .map(|&(_, width, height)| (width, height))
        }

        /// **Issue #92: a window leaving fullscreen went nowhere.**
        ///
        /// `fullscreen_request` kept the rect to come back to on the window's
        /// `Decoration`, and then dropped that decoration so the fullscreen
        /// window would have no titlebar; the one `unfullscreen_request` built
        /// in its place had no rect in it. With nothing to re-place it, which is
        /// how this test runs -- no layout script at all -- the window stayed
        /// where fullscreen put it, covering the monitor, and was configured
        /// with no size. A layout that re-places windows on the relayout that
        /// follows covers for it, which is why the issue saw it on floating
        /// layouts and not on tiled ones.
        ///
        /// Run with `pane = "none"` for the reason every test in this module is
        /// (see the #99 test), which means this window never has a `Decoration`
        /// to lose. That fails on the old code all the same, and for the second
        /// half of the same defect: a window with no server-side frame had
        /// nowhere to keep the rect in the first place. The half with a real
        /// frame is `decoration.rs`'s
        /// `a_rebuilt_frame_does_not_take_the_way_back_with_it`.
        ///
        /// The request is sent twice, because a client may, and the second one
        /// must not overwrite the way back with the monitor it is already
        /// covering.
        #[test]
        fn a_window_leaving_fullscreen_is_back_where_it_was() {
            let mut display = Display::<Solium>::new().expect("creating a test wayland display");
            let mut state = Solium::new(display.handle());
            state
                .decorations
                .set_style(&mut state.panes, Some("none".to_string()));
            let output = one_screen(&mut state);
            let screen = state
                .space
                .output_geometry(&output)
                .expect("the monitor is mapped");

            let (conn, mut event_queue, mut client) = connect(&mut display, &mut state);
            let qh = event_queue.handle();
            let (window, toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
            state.space.map_element(window.clone(), (400, 300), false);
            state.space.refresh();
            let before = state.real_geometry(&window).expect("the window is mapped");
            assert_eq!(
                before,
                Rectangle::new((400, 300).into(), (64, 64).into()),
                "the fixture's window, where this test put it"
            );
            let id = state.panes.id_of(&window).expect("the window has a pane");

            toplevel.set_fullscreen(None);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut event_queue,
                &mut client,
            );
            toplevel.set_fullscreen(None);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut event_queue,
                &mut client,
            );
            assert_eq!(
                state.panes.get(id).map(Pane::slot),
                Some(screen),
                "fullscreen really did move the window, or coming back would \
                 prove nothing"
            );
            assert_eq!(state.space.element_location(&window), Some(screen.loc));

            toplevel.unset_fullscreen();
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut event_queue,
                &mut client,
            );

            assert_eq!(
                state.panes.get(id).map(Pane::slot),
                Some(before),
                "a window leaving fullscreen goes back to the rect it had before \
                 -- not the monitor it was covering, which is where it stayed \
                 when there was no rect kept to go back to"
            );
            assert_eq!(
                state.space.element_location(&window),
                Some(before.loc),
                "and it is drawn there, not only recorded there"
            );
            assert_eq!(
                last_configured(&client, &toplevel),
                Some((before.size.w, before.size.h)),
                "and the client is told the size it had, rather than being \
                 configured with no size and left to guess"
            );
        }

        /// **A window with no frame toggles back from maximised.**
        ///
        /// The rect a maximise goes back to lived on the window's `Decoration`,
        /// and a window with no server-side frame -- one that draws its own, or
        /// any window under `pane = "none"` -- has none, so on stage this
        /// toggle kept nothing for such a window and the second call maximised
        /// it again. Found with #92, and fixed by the same move.
        ///
        /// **Latent, not something a user could hit.** `toggle_maximize` has
        /// one caller, `frame_action`, reached only from a button on a
        /// `Styled` frame; there is no `maximize_request` handler, binding or
        /// script path. A window with no frame had no button to press. It is
        /// tested anyway because `pane = "none"` is the only way this module
        /// can drive `toggle_maximize` at all, and the maximise-then-fullscreen
        /// tests below build on it working. The frameless half of #92 that was
        /// seen is fullscreen's, in `a_window_leaving_fullscreen_is_back_where_it_was`.
        #[test]
        fn a_window_with_no_frame_toggles_back_from_maximised() {
            let mut display = Display::<Solium>::new().expect("creating a test wayland display");
            let mut state = Solium::new(display.handle());
            state
                .decorations
                .set_style(&mut state.panes, Some("none".to_string()));
            let output = one_screen(&mut state);
            let screen = state
                .space
                .output_geometry(&output)
                .expect("the monitor is mapped");

            let (conn, mut event_queue, mut client) = connect(&mut display, &mut state);
            let qh = event_queue.handle();
            let (window, toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
            state.space.map_element(window.clone(), (400, 300), false);
            state.space.refresh();
            let before = state.real_geometry(&window).expect("the window is mapped");
            let surface = window.toplevel().cloned().expect("an xdg toplevel");
            // The server's `xdg_toplevel`, which this module's `use` of the
            // client's shadows.
            let maximized = || {
                surface.with_pending_state(|pending| {
                    pending.states.contains(
                        smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Maximized,
                    )
                })
            };

            state.toggle_maximize(&window);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut event_queue,
                &mut client,
            );
            assert!(maximized(), "the first press maximises");
            assert_eq!(
                state.space.element_location(&window),
                Some(screen.loc),
                "and moves the window to the work area, which is the whole \
                 monitor here, or coming back would prove nothing"
            );
            assert_eq!(
                last_configured(&client, &toplevel),
                Some((screen.size.w, screen.size.h))
            );

            state.toggle_maximize(&window);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut event_queue,
                &mut client,
            );
            assert!(
                !maximized(),
                "the second press restores, rather than maximising again"
            );
            assert_eq!(
                state.space.element_location(&window),
                Some(before.loc),
                "back to exactly where it was"
            );
            assert_eq!(
                last_configured(&client, &toplevel),
                Some((before.size.w, before.size.h)),
                "at exactly the size it was"
            );
        }

        /// A 1920x1080 monitor at 1x, mapped at `at`.
        ///
        /// Named, because two of them in one test must not share a name:
        /// `place_outputs` goes by names, and the unplug tests below run it.
        fn a_screen(state: &mut Solium, name: &str, at: (i32, i32)) -> Output {
            let output = Output::new(
                name.to_string(),
                PhysicalProperties {
                    size: (0, 0).into(),
                    subpixel: Subpixel::Unknown,
                    make: "solium".to_string(),
                    model: name.to_string(),
                },
            );
            output.change_current_state(
                Some(Mode {
                    size: (1920, 1080).into(),
                    refresh: 60_000,
                }),
                None,
                Some(Scale::Fractional(1.0)),
                None,
            );
            state.space.map_output(&output, at);
            output
        }

        /// A bar across the top of the primary monitor, `height` pixels tall,
        /// holding them as its exclusive zone.
        ///
        /// Committed once and with no buffer, which is enough: the zone is
        /// double-buffered state that the initial commit applies, and
        /// `configure_layer` arranges the monitor on that commit. The caller
        /// checks the work area it leaves rather than trusting this.
        fn bar(
            display: &mut Display<Solium>,
            state: &mut Solium,
            conn: &Connection,
            client: &Client,
            qh: &QueueHandle<Client>,
            height: i32,
        ) -> (
            wl_surface::WlSurface,
            zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        ) {
            let compositor = client.compositor.clone().expect("wl_compositor bound");
            let shell = client
                .layer_shell
                .clone()
                .expect("zwlr_layer_shell_v1 bound");
            let surface = compositor.create_surface(qh, ());
            let layer = shell.get_layer_surface(
                &surface,
                None,
                zwlr_layer_shell_v1::Layer::Top,
                "restore-test-bar".to_string(),
                qh,
                (),
            );
            layer.set_anchor(
                zwlr_layer_surface_v1::Anchor::Top
                    | zwlr_layer_surface_v1::Anchor::Left
                    | zwlr_layer_surface_v1::Anchor::Right,
            );
            layer.set_size(0, height.unsigned_abs());
            layer.set_exclusive_zone(height);
            surface.commit();
            conn.flush().expect("flushing the bar");
            display
                .dispatch_clients(state)
                .expect("dispatching the bar");
            (surface, layer)
        }

        /// Whether the server has this window in `wanted`, as the next
        /// configure it is sent will say.
        fn in_state(
            window: &Window,
            wanted: smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State,
        ) -> bool {
            window.toplevel().is_some_and(|surface| {
                surface.with_pending_state(|pending| pending.states.contains(wanted))
            })
        }

        /// Every size this toplevel has been configured with, oldest first.
        fn sizes_sent(client: &Client, toplevel: &xdg_toplevel::XdgToplevel) -> Vec<(i32, i32)> {
            let id = wayland_client::Proxy::id(toplevel);
            client
                .configures
                .iter()
                .filter(|(to, _, _)| *to == id)
                .map(|&(_, width, height)| (width, height))
                .collect()
        }

        /// **#92 review, finding 1: a maximised window leaving fullscreen is
        /// maximised again.**
        ///
        /// Maximise and fullscreen keep their way back in the one slot on the
        /// pane, and a window maximised and then sent fullscreen keeps the rect
        /// from before the maximise in it. Leaving fullscreen took that rect:
        /// the window was placed and sized un-maximised while its xdg state
        /// still said `Maximized`, and the slot was empty, so the next toggle
        /// maximised it again rather than restoring it. A maximised browser
        /// sent fullscreen for a video came back small on Esc, still told it
        /// was maximised.
        ///
        /// The bar is what makes the placement assertion mean anything. Without
        /// one the work area is the whole monitor, which is also where
        /// fullscreen put the window, so "back at the work area" and "left where
        /// fullscreen put it" would be the same rectangle.
        ///
        /// `pane = "none"`, as everywhere in this module, so there is no frame's
        /// share to take out of the work area. `toggle_maximize` and leaving
        /// fullscreen both take it from `frame_insets`, and this does not test
        /// that they agree about a frame that is really there.
        #[test]
        fn a_maximised_window_leaving_fullscreen_is_maximised_again() {
            use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;

            let mut display = Display::<Solium>::new().expect("creating a test wayland display");
            let mut state = Solium::new(display.handle());
            state
                .decorations
                .set_style(&mut state.panes, Some("none".to_string()));
            let output = one_screen(&mut state);
            let screen = state
                .space
                .output_geometry(&output)
                .expect("the monitor is mapped");

            let (conn, mut event_queue, mut client) = connect(&mut display, &mut state);
            let qh = event_queue.handle();
            let _bar = bar(&mut display, &mut state, &conn, &client, &qh, 30);
            let work = state.work_area_on(&output).expect("the monitor is mapped");
            assert_eq!(
                work,
                Rectangle::new((0, 30).into(), (1920, 1050).into()),
                "the bar holds the top of the screen, or the work area is the \
                 monitor and nothing below can tell maximised from fullscreen"
            );

            let (window, toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
            state.space.map_element(window.clone(), (400, 300), false);
            state.space.refresh();
            let before = state.real_geometry(&window).expect("the window is mapped");
            let id = state.panes.id_of(&window).expect("the window has a pane");

            state.toggle_maximize(&window);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut event_queue,
                &mut client,
            );
            assert!(in_state(&window, State::Maximized));
            assert_eq!(state.space.element_location(&window), Some(work.loc));

            toplevel.set_fullscreen(None);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut event_queue,
                &mut client,
            );
            assert_eq!(
                state.space.element_location(&window),
                Some(screen.loc),
                "fullscreen covers the bar, so leaving it has somewhere to come \
                 back from"
            );

            client.configures.clear();
            toplevel.unset_fullscreen();
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut event_queue,
                &mut client,
            );

            assert!(!in_state(&window, State::Fullscreen));
            assert!(
                in_state(&window, State::Maximized),
                "it was maximised when it went fullscreen and nothing has \
                 un-maximised it since"
            );
            assert_eq!(
                state.space.element_location(&window),
                Some(work.loc),
                "so it is placed maximised, below the bar -- not at the rect \
                 from before the maximise, which is where leaving fullscreen \
                 used to put it while its state still said maximised"
            );
            assert_eq!(state.panes.get(id).map(Pane::slot), Some(work));
            assert_eq!(
                sizes_sent(&client, &toplevel),
                vec![(work.size.w, work.size.h)],
                "and it is told the work area's size, once"
            );
            assert_eq!(
                state.panes.get(id).and_then(Pane::restore),
                Some(before),
                "the rect from before the maximise is still kept, for the \
                 un-maximise"
            );

            state.toggle_maximize(&window);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut event_queue,
                &mut client,
            );
            assert!(
                !in_state(&window, State::Maximized),
                "the next toggle restores, rather than maximising again"
            );
            assert_eq!(
                state.space.element_location(&window),
                Some(before.loc),
                "to where it was before the maximise"
            );
            assert_eq!(
                last_configured(&client, &toplevel),
                Some((before.size.w, before.size.h))
            );
        }

        /// **#92 review, finding 1: leaving fullscreen needs a window that is
        /// fullscreen.**
        ///
        /// A client may send `unset_fullscreen` whenever it likes. One that
        /// sent it while merely maximised had the maximise's way back spent on
        /// it, because the two share one slot: the window jumped back to its
        /// pre-maximise rect still marked maximised, and the next toggle
        /// maximised it again.
        #[test]
        fn a_window_that_is_not_fullscreen_has_nothing_to_leave() {
            use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;

            let mut display = Display::<Solium>::new().expect("creating a test wayland display");
            let mut state = Solium::new(display.handle());
            state
                .decorations
                .set_style(&mut state.panes, Some("none".to_string()));
            let output = one_screen(&mut state);
            let screen = state
                .space
                .output_geometry(&output)
                .expect("the monitor is mapped");

            let (conn, mut event_queue, mut client) = connect(&mut display, &mut state);
            let qh = event_queue.handle();
            let (window, toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
            state.space.map_element(window.clone(), (400, 300), false);
            state.space.refresh();
            let before = state.real_geometry(&window).expect("the window is mapped");
            let id = state.panes.id_of(&window).expect("the window has a pane");

            state.toggle_maximize(&window);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut event_queue,
                &mut client,
            );
            assert!(in_state(&window, State::Maximized));

            client.configures.clear();
            toplevel.unset_fullscreen();
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut event_queue,
                &mut client,
            );
            assert_eq!(
                state.panes.get(id).and_then(Pane::restore),
                Some(before),
                "a window that was never fullscreen keeps its maximise's way back"
            );
            assert_eq!(
                state.space.element_location(&window),
                Some(screen.loc),
                "and stays maximised where it was"
            );
            assert_eq!(
                sizes_sent(&client, &toplevel),
                Vec::<(i32, i32)>::new(),
                "and is told nothing, because nothing changed"
            );

            state.toggle_maximize(&window);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut event_queue,
                &mut client,
            );
            assert!(!in_state(&window, State::Maximized));
            assert_eq!(state.space.element_location(&window), Some(before.loc));
        }

        /// **#92 review, finding 3: leaving fullscreen is one configure.**
        ///
        /// It was two: the size cleared -- 0x0, "pick your own" -- and then the
        /// size to go back to. A client that acts on every configure it reads,
        /// a terminal reflowing its grid, resized twice, once to a size of its
        /// own choosing and once to the real one. On stage the second was never
        /// sent, because there was never a rect to send.
        #[test]
        fn a_window_leaving_fullscreen_is_told_its_size_once() {
            let mut display = Display::<Solium>::new().expect("creating a test wayland display");
            let mut state = Solium::new(display.handle());
            state
                .decorations
                .set_style(&mut state.panes, Some("none".to_string()));
            let _output = one_screen(&mut state);

            let (conn, mut event_queue, mut client) = connect(&mut display, &mut state);
            let qh = event_queue.handle();
            let (window, toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
            state.space.map_element(window.clone(), (400, 300), false);
            state.space.refresh();
            let before = state.real_geometry(&window).expect("the window is mapped");

            toplevel.set_fullscreen(None);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut event_queue,
                &mut client,
            );
            client.configures.clear();
            toplevel.unset_fullscreen();
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut event_queue,
                &mut client,
            );
            assert_eq!(
                sizes_sent(&client, &toplevel),
                vec![(before.size.w, before.size.h)],
                "one configure, with the size it had before"
            );
        }

        /// **#92 review, finding 4: a window leaving fullscreen on a monitor
        /// that has gone comes back on one that is here.**
        ///
        /// The rect a window goes back to is stored, and a stored rect can be
        /// on a monitor that has since been unplugged or gone to sleep.
        /// `rescue_offscreen` brings the fullscreen window itself onto a
        /// remaining screen, but not the rect it goes back to, and it runs
        /// only when the monitors change: leaving fullscreen put the window on
        /// no screen at all, and it stayed there until the next hotplug.
        #[test]
        fn a_window_leaving_fullscreen_after_its_monitor_went_is_on_a_screen() {
            let mut display = Display::<Solium>::new().expect("creating a test wayland display");
            let mut state = Solium::new(display.handle());
            state
                .decorations
                .set_style(&mut state.panes, Some("none".to_string()));
            let left = a_screen(&mut state, "restore-left", (0, 0));
            let right = a_screen(&mut state, "restore-right", (1920, 0));
            let remaining = state
                .space
                .output_geometry(&left)
                .expect("the left monitor is mapped");

            let (conn, mut event_queue, mut client) = connect(&mut display, &mut state);
            let qh = event_queue.handle();
            let (window, toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
            state.space.map_element(window.clone(), (2320, 300), false);
            state.space.refresh();
            let before = state.real_geometry(&window).expect("the window is mapped");

            toplevel.set_fullscreen(None);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut event_queue,
                &mut client,
            );
            assert_eq!(
                state.space.element_location(&window),
                Some((1920, 0).into()),
                "fullscreen on the monitor the window is on, the right one"
            );

            // Unplugged, the way both backends do it.
            state.space.unmap_output(&right);
            state.settle_monitors();
            let rescued = state.real_geometry(&window).expect("the window is mapped");
            assert!(
                remaining.overlaps(rescued),
                "rescue_offscreen brought the fullscreen window onto the screen \
                 that is left, or leaving fullscreen would start from nowhere: \
                 {rescued:?}"
            );

            toplevel.unset_fullscreen();
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut event_queue,
                &mut client,
            );
            let back = state.real_geometry(&window).expect("the window is mapped");
            assert!(
                state.on_any_output(back),
                "the window that left fullscreen is on a screen: {back:?}, not \
                 at the {before:?} it had on the monitor that went"
            );
            assert_eq!(
                back,
                Rectangle::new((1920 - 64, 300).into(), (64, 64).into()),
                "moved the way rescue_offscreen moves a stranded window: onto \
                 the nearest screen, its size kept, clamped at the edge it was \
                 beyond"
            );
            assert_eq!(last_configured(&client, &toplevel), Some((64, 64)));
        }

        /// **#92 review, finding 4, for a maximise.**
        ///
        /// The same stored rect and the same hazard. Leaving fullscreen now
        /// puts a maximised window back at its work area and leaves the rect
        /// from before the maximise for the un-maximise, so a window maximised
        /// on a monitor that then goes needs the un-maximise to check too.
        #[test]
        fn a_window_maximised_on_a_monitor_that_went_restores_onto_a_screen() {
            let mut display = Display::<Solium>::new().expect("creating a test wayland display");
            let mut state = Solium::new(display.handle());
            state
                .decorations
                .set_style(&mut state.panes, Some("none".to_string()));
            let left = a_screen(&mut state, "restore-left", (0, 0));
            let right = a_screen(&mut state, "restore-right", (1920, 0));
            let remaining = state
                .space
                .output_geometry(&left)
                .expect("the left monitor is mapped");

            let (conn, mut event_queue, mut client) = connect(&mut display, &mut state);
            let qh = event_queue.handle();
            let (window, toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
            state.space.map_element(window.clone(), (2320, 300), false);
            state.space.refresh();

            state.toggle_maximize(&window);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut event_queue,
                &mut client,
            );
            assert_eq!(
                state.space.element_location(&window),
                Some((1920, 0).into()),
                "maximised on the monitor the window is on, the right one"
            );

            state.space.unmap_output(&right);
            state.settle_monitors();
            let rescued = state.real_geometry(&window).expect("the window is mapped");
            assert!(
                remaining.overlaps(rescued),
                "rescue_offscreen brought the maximised window onto the screen \
                 that is left: {rescued:?}"
            );

            state.toggle_maximize(&window);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut event_queue,
                &mut client,
            );
            let back = state.real_geometry(&window).expect("the window is mapped");
            assert_eq!(
                back,
                Rectangle::new((1920 - 64, 300).into(), (64, 64).into()),
                "restored onto the screen that is left, not to 2320,300 on the \
                 one that went"
            );
            assert_eq!(last_configured(&client, &toplevel), Some((64, 64)));
        }

        /// **#92 review, finding 5: a window fullscreen before it has drawn
        /// keeps no way back.**
        ///
        /// A player started with `--fs` asks before its first commit.
        /// `new_toplevel` has mapped it at 0,0 by then, with no buffer and so no
        /// size, and that 0x0 rect was kept as the way back: leaving fullscreen
        /// configured 0x0 -- which on the wire is "pick your own", the same as
        /// no rect at all -- and set the pane's slot to a rect of no size.
        ///
        /// And with no rect kept, a second request once the window has drawn
        /// must still not keep one: the window is fullscreen by then, and the
        /// rect it has is the monitor.
        #[test]
        fn a_window_fullscreen_before_it_has_drawn_keeps_no_way_back() {
            let mut display = Display::<Solium>::new().expect("creating a test wayland display");
            let mut state = Solium::new(display.handle());
            state
                .decorations
                .set_style(&mut state.panes, Some("none".to_string()));
            let output = one_screen(&mut state);
            let screen = state
                .space
                .output_geometry(&output)
                .expect("the monitor is mapped");

            let (conn, mut event_queue, mut client) = connect(&mut display, &mut state);
            let qh = event_queue.handle();
            let compositor = client.compositor.clone().expect("wl_compositor bound");
            let wm_base = client.wm_base.clone().expect("xdg_wm_base bound");
            let surface = compositor.create_surface(&qh, ());
            let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
            let toplevel = xdg_surface.get_toplevel(&qh, ());
            toplevel.set_fullscreen(None);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut event_queue,
                &mut client,
            );
            let window = state
                .space
                .elements()
                .next()
                .cloned()
                .expect("new_toplevel maps a window before it has drawn anything");
            let id = state.panes.id_of(&window).expect("the window has a pane");
            assert_eq!(
                state.panes.get(id).and_then(Pane::restore),
                None,
                "a window with no size has no rect worth going back to"
            );

            commit_buffer(&client, &qh, &surface, screen.size.w, screen.size.h);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut event_queue,
                &mut client,
            );
            toplevel.set_fullscreen(None);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut event_queue,
                &mut client,
            );
            assert_eq!(
                state.panes.get(id).and_then(Pane::restore),
                None,
                "asking again while fullscreen does not keep the monitor as the \
                 way back"
            );

            client.configures.clear();
            toplevel.unset_fullscreen();
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut event_queue,
                &mut client,
            );
            assert_eq!(
                sizes_sent(&client, &toplevel),
                vec![(0, 0)],
                "with nowhere kept to go back to, the client picks its own size"
            );
            assert!(
                state
                    .panes
                    .get(id)
                    .is_some_and(|pane| !pane.slot().is_empty()),
                "and the pane is not given a rect of no size"
            );
        }

        /// **A modal cannot be buried under the window it is waiting on.**
        ///
        /// The regression floating them introduced. Tiled, a dialog took a slot
        /// of its own and overlapped nothing, so there was nowhere for it to be
        /// lost; floated, it sits *on* its parent, and nothing in the stack said
        /// which of the two belongs on top. `Space::map_element` takes an
        /// element out of the stack and pushes it back at the top whatever
        /// `activate` says, so every raise buried the prompt: one click on the
        /// strip of document left showing, or one pointer crossing with
        /// `focus_follows_mouse`, and "Discard changes?" was behind the window
        /// refusing to accept keystrokes until it is answered.
        ///
        /// The third window is what stops this passing for the wrong reason. If
        /// `focus_window` had quietly stopped restacking at all, the dialog
        /// would still be above its parent and the test would be green over a
        /// broken raise -- so the parent is also asserted to have come above the
        /// window it was under.
        #[test]
        fn a_modal_stays_above_the_window_it_is_waiting_on() {
            let mut display = Display::<Solium>::new().expect("creating a test wayland display");
            let mut state = Solium::new(display.handle());
            // See the #99 test: a Qt scene in a process holding a libwayland
            // connection of its own aborts the whole test binary.
            state
                .decorations
                .set_style(&mut state.panes, Some("none".to_string()));

            let (conn, event_queue, client) = connect(&mut display, &mut state);
            let qh = event_queue.handle();

            let (parent, parent_toplevel) =
                open_window(&mut display, &mut state, &conn, &client, &qh);
            let (dialog, dialog_toplevel) =
                open_window(&mut display, &mut state, &conn, &client, &qh);
            let (other, _other_toplevel) =
                open_window(&mut display, &mut state, &conn, &client, &qh);

            // The two requests that make this a modal dialog, in the order a
            // toolkit sends them and from the only side that can send them.
            let dialogs = client.dialogs.clone().expect("xdg_wm_dialog_v1 bound");
            dialog_toplevel.set_parent(Some(&parent_toplevel));
            let object = dialogs.get_xdg_dialog(&dialog_toplevel, &qh, ());
            object.set_modal();
            conn.flush().expect("flushing set_parent and set_modal");
            display
                .dispatch_clients(&mut state)
                .expect("dispatching set_parent and set_modal");

            // The fixture is doing what it claims before anything is asserted
            // about stacking: the compositor agrees this is a modal, and agrees
            // whose.
            assert!(
                state.is_modal(&dialog),
                "the client called set_modal and the compositor did not read it \
                 back, so nothing below is about a modal dialog at all"
            );
            let parent_pane = state.panes.id_of(&parent).expect("the parent has a pane");
            assert_eq!(
                state.parent_of(&dialog),
                Parentage::Window(parent_pane.get()),
                "the client called set_parent and the compositor did not read it \
                 back"
            );

            // `elements` is back to front, so a higher index is nearer the top.
            let depth = |state: &Solium, window: &Window| {
                state
                    .space
                    .elements()
                    .position(|element| element == window)
                    .expect("a mapped window is in the space")
            };
            assert!(
                depth(&state, &dialog) > depth(&state, &parent),
                "the dialog opened after its parent, so it starts above it -- and \
                 a test that starts in the state it is checking for proves nothing"
            );

            // One click on the parent. This is the whole of the bug.
            state.focus_window(&parent, SERIAL_COUNTER.next_serial());

            assert!(
                depth(&state, &parent) > depth(&state, &other),
                "focusing the parent did not raise it above the window that was \
                 over it, so this run says nothing about what a raise does to its \
                 dialog"
            );
            assert!(
                depth(&state, &dialog) > depth(&state, &parent),
                "the parent was raised over the dialog that is waiting on it: the \
                 prompt is now behind the window it is blocking, where it cannot \
                 be read or dismissed"
            );
        }

        /// **This is issue #113, through the compositor rather than beside it.**
        ///
        /// A top-left drag, on the first frame, with a real client that has
        /// committed one buffer and will not commit another. That last part is
        /// the whole test: the client answers *nothing*, so what is asserted is
        /// what the compositor draws while it is waiting — which is exactly the
        /// window the shake lives in.
        ///
        /// **A test that checked the final rectangle would pass on `stage`.**
        /// The client does commit eventually and the window does end up the
        /// right size; the bug is entirely in the frames before that. So this
        /// asserts the rectangle with no answer in hand, and it asserts the
        /// *pinned* edge rather than the dragged one — `stage` gets the dragged
        /// corner right, because it applies the origin immediately. What it
        /// gets wrong is the opposite corner, which it moves by the whole of
        /// the drag's delta and then snaps back.
        ///
        /// Confirmed failing against `stage`'s rule, not merely assumed to: the
        /// rectangle `stage` would produce is computed here from the same
        /// inputs and asserted to be wrong. `stage`'s `pane_geometry` answers
        /// `real_geometry`, which is the space's location paired with the
        /// *client's* size — `on_stage` below, spelled out.
        #[test]
        fn the_edge_being_dragged_is_the_edge_that_moves_before_any_client_answers() {
            let mut display = Display::<Solium>::new().expect("creating a test wayland display");
            let mut state = Solium::new(display.handle());
            // See the #99 test: a Qt scene in a process holding a libwayland
            // connection of its own aborts the whole test binary.
            state
                .decorations
                .set_style(&mut state.panes, Some("none".to_string()));

            let (conn, event_queue, client) = connect(&mut display, &mut state);
            let qh = event_queue.handle();
            let (window, _toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);

            // Away from the origin, so a drag that moved the window to where it
            // already was could not pass by accident.
            state.map_stacked(window.clone(), (400, 300), false);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&window)
                .expect("a client in the space has a pane");
            let before = state
                .pane_outer_of(pane)
                .expect("a mapped pane has a rectangle");
            let painted = window.geometry().size;
            assert!(
                painted.w > 20 && painted.h > 12,
                "the drag below has to leave a window with a size"
            );

            // The drag: the top-left corner, 20 right and 12 down. Smaller by
            // that much, moved by that much, with the bottom-right corner
            // standing still — that is what dragging a top-left corner means.
            let wanted = Rectangle::new(
                (before.loc.x + 20, before.loc.y + 12).into(),
                (before.size.w - 20, before.size.h - 12).into(),
            );
            state.pending_resize = Some(ResizeRequest {
                window: window.clone(),
                wanted,
                edge_at: payload_edge(wanted, ResizeEdge::TopLeft),
                edges: ResizeEdge::TopLeft,
            });
            state.settle_resize();

            // The client has been asked and has said nothing. Asserted rather
            // than assumed, because a client that *had* answered would make
            // every assertion below true for the wrong reason.
            assert_eq!(
                window.geometry().size,
                painted,
                "the fixture's client never commits a second buffer; if it had, \
                 this test would be checking the easy case"
            );

            let after = state
                .pane_outer_of(pane)
                .expect("a mapped pane has a rectangle");
            assert_eq!(
                after, wanted,
                "the pane takes the dragged rectangle whole — origin and size \
                 in the same frame — or the two halves land at different times \
                 and that gap is the shake"
            );
            assert_eq!(
                (after.loc.x + after.size.w, after.loc.y + after.size.h),
                (before.loc.x + before.size.w, before.loc.y + before.size.h),
                "the bottom-right corner is not being dragged and must not move"
            );

            // And `stage`'s rule, computed from the same two inputs: the new
            // origin paired with the client's size, which is what
            // `real_geometry` answers and what `pane_geometry` used to return.
            let on_stage = Rectangle::new(wanted.loc, painted);
            assert_ne!(
                (
                    on_stage.loc.x + on_stage.size.w,
                    on_stage.loc.y + on_stage.size.h
                ),
                (before.loc.x + before.size.w, before.loc.y + before.size.h),
                "the control: pairing the new origin with the client's old size \
                 walks the corner nobody is dragging across the desktop, and \
                 snaps it back when the client finally commits. That is #113, \
                 and it is what this test fails on against stage"
            );
            assert_eq!(
                on_stage.loc.x + on_stage.size.w,
                before.loc.x + before.size.w + 20,
                "by the drag's whole delta, every frame"
            );

            // The space agrees about *position* throughout. Only the size is
            // held back, because only the size needs the client's consent —
            // holding the position back too would put the space and the pane
            // into the standing disagreement #84 is about, for no gain.
            assert_eq!(
                state
                    .space
                    .element_location(&window)
                    .expect("a mapped window has a location"),
                wanted.loc,
                "the slot is authoritative for the size, not for where the \
                 window is; nothing here may add a fourth opinion about that"
            );
        }

        /// **The round trip this whole fix rests on, with a frame's insets in
        /// it, on all eight edges.**
        ///
        /// `hold_resize` writes the pane's slot as `inner(wanted)` and every
        /// reader grows it back with `pane_outer`, so `pane_outer(inner(wanted))
        /// == wanted` is what makes the dragged rectangle survive the trip. It
        /// held for zero insets whichever spelling was used, which is why a
        /// window with no frame could not catch this: `frame_insets` asks
        /// `is_decorated` — `Styled` and nothing else — while every reader goes
        /// through `insets_of`, which reserves a titlebar for `Frame::Pending`
        /// so a window does not change shape when its frame arrives.
        ///
        /// `Frame::Pending` is not a moment, it is where a pane whose
        /// decoration *failed to build* stays for good — see
        /// `decoration::Decorations::insert`, which logs and leaves it there. So
        /// the two spellings disagreeing meant that for those windows the slot
        /// grew by a titlebar on every frame of a drag: the top edge landing a
        /// titlebar above the rectangle under the pointer, and the client asked
        /// for a size a titlebar too tall.
        ///
        /// Set by hand rather than by failing a build, because building a frame
        /// needs Qt and a Qt scene in a process holding a libwayland connection
        /// aborts the test binary (see the #99 test). The state is the same
        /// state; how a pane got into it is `decoration.rs`'s business.
        #[test]
        fn a_framed_window_is_dragged_to_the_rectangle_the_pointer_asks_for() {
            let mut display = Display::<Solium>::new().expect("creating a test wayland display");
            let mut state = Solium::new(display.handle());
            state
                .decorations
                .set_style(&mut state.panes, Some("none".to_string()));

            let (conn, event_queue, client) = connect(&mut display, &mut state);
            let qh = event_queue.handle();
            let (window, _toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
            state.map_stacked(window.clone(), (400, 300), false);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&window)
                .expect("a client in the space has a pane");

            // A pane reserving room for a frame that is never coming.
            state
                .panes
                .get_mut(pane)
                .expect("the pane is here")
                .set_frame(crate::pane::Frame::Pending);
            assert!(
                !state.is_decorated(&window),
                "`Pending` is not decorated, which is exactly why the two \
                 spellings of the insets disagree about it"
            );
            assert_eq!(
                state.insets_of(pane).top,
                TITLEBAR_HEIGHT,
                "and it reserves a titlebar all the same, or this test is about \
                 two things that agree"
            );

            for edges in [
                ResizeEdge::Top,
                ResizeEdge::Bottom,
                ResizeEdge::Left,
                ResizeEdge::Right,
                ResizeEdge::TopLeft,
                ResizeEdge::TopRight,
                ResizeEdge::BottomLeft,
                ResizeEdge::BottomRight,
            ] {
                // A fresh gesture each time, which also reconciles the previous
                // one rather than leaving it hanging.
                let before = state
                    .begin_resize(&window)
                    .expect("a mapped pane has a rectangle");

                // Twenty pixels out of whichever edges this drag holds, with
                // the opposite ones standing still. `resized` is where that
                // arithmetic lives and it is tested beside itself; what is
                // being checked here is only that the rectangle survives the
                // compositor.
                let (left, top) = (
                    crate::input::resize::pulls_left(edges),
                    crate::input::resize::pulls_top(edges),
                );
                let right = matches!(
                    edges,
                    ResizeEdge::Right | ResizeEdge::TopRight | ResizeEdge::BottomRight
                );
                let bottom = matches!(
                    edges,
                    ResizeEdge::Bottom | ResizeEdge::BottomLeft | ResizeEdge::BottomRight
                );
                let wanted = Rectangle::new(
                    (
                        before.loc.x - i32::from(left) * 20,
                        before.loc.y - i32::from(top) * 20,
                    )
                        .into(),
                    (
                        before.size.w + i32::from(left || right) * 20,
                        before.size.h + i32::from(top || bottom) * 20,
                    )
                        .into(),
                );
                assert_ne!(wanted, before, "every edge has to actually move one");
                state.pending_resize = Some(ResizeRequest {
                    window: window.clone(),
                    wanted,
                    edge_at: payload_edge(wanted, edges),
                    edges,
                });
                state.settle_resize();

                assert_eq!(
                    state
                        .pane_outer_of(pane)
                        .expect("a mapped pane has a rectangle"),
                    wanted,
                    "the drag asked for this rectangle and {edges:?} did not get \
                     it: the slot is written with one spelling of the frame's \
                     insets and read back with another"
                );
                // And the client is asked for what is left inside the frame,
                // not for the whole of it.
                assert_eq!(
                    state
                        .panes
                        .get(pane)
                        .expect("the pane is here")
                        .slot()
                        .size
                        .h,
                    wanted.size.h - TITLEBAR_HEIGHT,
                    "a titlebar's worth of the dragged rectangle belongs to the \
                     frame, so the client must not be sized the whole of it"
                );
            }
        }

        /// **A whole gesture inside one dispatch batch, which is an ordinary
        /// quick nudge of a border.**
        ///
        /// The press, the motion and the release all land in one calloop
        /// dispatch — sixteen milliseconds is plenty — and `settle_resize` runs
        /// at the frame *after* all three. So the hold is born after the
        /// gesture it belongs to has already ended: `ResizeGrab::motion` only
        /// records `pending_resize`, and `hold_resize` is what turns that into a
        /// `Hold`.
        ///
        /// A hold that cannot observe its own release never sets `released`,
        /// and `Hold::settle` answers `Waiting` unconditionally without one. The
        /// hold is then **permanent**: `holding_resize` stays true for ever,
        /// `pane_geometry` keeps answering the slot, and every later size the
        /// client chooses for itself is stretched into a rectangle from a drag
        /// that finished minutes ago. A window soft until something drags it
        /// again.
        ///
        /// So what this waits for is the deadline, which is the *only* thing
        /// that can end this gesture: the fixture's client answers nothing at
        /// all. Slept rather than faked because `present::Clock` reads the
        /// monotonic clock through — see its own documentation for why it has no
        /// settable "now" to lie to.
        #[test]
        fn a_gesture_that_ends_before_its_first_frame_still_lets_go() {
            let mut display = Display::<Solium>::new().expect("creating a test wayland display");
            let mut state = Solium::new(display.handle());
            // See the #99 test: a Qt scene in a process holding a libwayland
            // connection of its own aborts the whole test binary.
            state
                .decorations
                .set_style(&mut state.panes, Some("none".to_string()));

            let (conn, event_queue, client) = connect(&mut display, &mut state);
            let qh = event_queue.handle();
            let (window, _toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);

            state.map_stacked(window.clone(), (400, 300), false);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&window)
                .expect("a client in the space has a pane");
            let before = state
                .pane_outer_of(pane)
                .expect("a mapped pane has a rectangle");

            // The press.
            state.begin_resize(&window);
            // The motion. `ResizeGrab::motion` records and does not apply, so
            // this is the whole of what a motion does before a frame.
            let wanted = Rectangle::new(
                (before.loc.x + 20, before.loc.y + 12).into(),
                (before.size.w - 20, before.size.h - 12).into(),
            );
            state.pending_resize = Some(ResizeRequest {
                window: window.clone(),
                wanted,
                edge_at: payload_edge(wanted, ResizeEdge::TopLeft),
                edges: ResizeEdge::TopLeft,
            });
            // And the release, still with no frame in between: this is
            // `ResizeGrab::unset`, which is where the button coming up lands.
            state.release_resize(&window);

            // *Now* the frame. This is where the hold is born, and it is born
            // into a gesture that is already over.
            state.settle_resize();
            assert!(
                state.holding_resize(pane),
                "the frame after the release is where this hold is born; if no \
                 hold is created at all then this test is about nothing"
            );
            assert_eq!(
                window.geometry().size,
                before.size,
                "the fixture's client never commits a second buffer, which is \
                 what leaves the deadline as the only thing that can end this"
            );

            // Past the deadline, and one more frame to notice it.
            std::thread::sleep(crate::resizing::PATIENCE + std::time::Duration::from_millis(100));
            state.settle_resize();
            assert!(
                !state.holding_resize(pane),
                "the hold outlived its own gesture's deadline, so it will outlive \
                 everything: `pane_geometry` answers the slot for as long as this \
                 is true and the client's own size is stretched into it for ever"
            );
            assert_eq!(
                state.pane_geometry(state.panes.get(pane).expect("the pane is still here")),
                state
                    .real_geometry(&window)
                    .expect("a mapped window has a rectangle"),
                "once the hold is gone the slot and the space agree again, which \
                 is what makes the stretch exactly 1"
            );
        }

        /// **The same defect by the other route: a hold dropped mid-gesture and
        /// re-created after the release.**
        ///
        /// `settle_resize` drops the hold on any frame a layout claims the
        /// drag — `trigger_resize` answering true — and creates a fresh one on
        /// any frame it does not. A script whose answer changes between two
        /// frames therefore destroys a hold and builds another, and if the
        /// second one is built on the frame *after* the button came up it is
        /// built into a gesture that is already over. Identical outcome to the
        /// quick-nudge case and a completely different way in, which is why the
        /// release is recorded on the compositor rather than guarded at each
        /// place a hold is made.
        ///
        /// The drop is driven directly here rather than through a Lua layout:
        /// `drop_resize_hold_for` *is* the line `settle_resize` runs when a
        /// layout claims the drag, and standing up a script that changes its
        /// mind between frames would test mlua rather than this.
        #[test]
        fn a_hold_rebuilt_after_the_release_is_rebuilt_already_released() {
            let mut display = Display::<Solium>::new().expect("creating a test wayland display");
            let mut state = Solium::new(display.handle());
            state
                .decorations
                .set_style(&mut state.panes, Some("none".to_string()));

            let (conn, event_queue, client) = connect(&mut display, &mut state);
            let qh = event_queue.handle();
            let (window, _toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
            state.map_stacked(window.clone(), (400, 300), false);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&window)
                .expect("a client in the space has a pane");
            let before = state
                .begin_resize(&window)
                .expect("a mapped pane has a rectangle");

            let dragged = |state: &mut Solium, by: i32| {
                let wanted = Rectangle::new(
                    (before.loc.x + by, before.loc.y + by).into(),
                    (before.size.w - by, before.size.h - by).into(),
                );
                state.pending_resize = Some(ResizeRequest {
                    window: window.clone(),
                    wanted,
                    edge_at: payload_edge(wanted, ResizeEdge::TopLeft),
                    edges: ResizeEdge::TopLeft,
                });
            };

            // A frame of ordinary drag, so a hold exists to be dropped.
            dragged(&mut state, 8);
            state.settle_resize();
            assert!(state.holding_resize(pane), "the drag is live");

            // The frame a layout claims it.
            state.drop_resize_hold_for(&window);
            assert!(!state.holding_resize(pane));

            // The button comes up with no hold to tell, and the frame after it
            // carries the last motion — which is where a hold is made again.
            state.release_resize(&window);
            dragged(&mut state, 12);
            state.settle_resize();
            assert!(
                state.holding_resize(pane),
                "the post-release frame is where the second hold is born; \
                 without one there is nothing here to go wrong"
            );

            std::thread::sleep(crate::resizing::PATIENCE + std::time::Duration::from_millis(100));
            state.settle_resize();
            assert!(
                !state.holding_resize(pane),
                "the second hold never heard about the release that preceded it, \
                 so nothing can ever end it"
            );
        }

        /// One frame of a drag that a layout claims, without standing up a
        /// layout.
        ///
        /// These three statements in this order *are* what `settle_resize` runs
        /// on the claimed branch: it arms the gesture, asks the scripts, and the
        /// scripts' own `apply` is what reaches `move_pane` — the whole layout
        /// sweep happens inside `trigger_resize`, before the caller learns
        /// whether the drag was claimed. Driving them directly is the same
        /// choice `a_hold_rebuilt_after_the_release_is_rebuilt_already_released`
        /// makes about `drop_resize_hold_for`, and for the same reason: a Lua
        /// layout here would be testing mlua and `tiling.lua`'s arithmetic
        /// rather than what the compositor does with the rectangle it is given.
        ///
        /// `duration: ZERO` is not a simplification. `tiling.lua` passes
        /// `{ duration = 0 }` for every frame of a seam drag, and the zero is
        /// load-bearing: a transform that finishes instantly is retired by
        /// `Solium::settle` after the very frame it was written on, which is why
        /// the layout's rectangle used to survive exactly one frame.
        fn tiled_frame(
            state: &mut Solium,
            request: &ResizeRequest,
            pane: crate::pane::PaneId,
            outer: Rectangle<i32, Logical>,
            now: Duration,
        ) {
            state.arm_resize_gesture(request);
            let was = state
                .pane_outer_of(pane)
                .expect("a mapped pane has a rectangle");
            state.move_pane(
                pane,
                outer,
                was,
                AnimationSpec {
                    duration: Duration::ZERO,
                    ..AnimationSpec::default()
                },
                now,
                Standing::Tile,
            );
            state.resize_gesture = None;
            // The claimed branch's own line: the bridge is watching every pane
            // the sweep moved, so a floating hold on this window would be a
            // second authority over one of them.
            state.drop_resize_hold_for(&request.window);
            // What `settle_resize` does after the layout has run. No-op while
            // the pointer is down, and included so the loop under test is the
            // loop that ships rather than a shorter one.
            paused_frame(state, now);
        }

        /// A frame of a live drag that carried **no** motion.
        ///
        /// The whole of what `settle_resize` does when `pending_resize` is
        /// empty, which is every frame the pointer is not moving on — including
        /// the ones at the end of a gesture, because people stop moving before
        /// they let go. The flush is the half `move_pane` cannot do: it is only
        /// reached from a frame that carried a motion, so the offers made in
        /// the last `TELL_EVERY` of travel have nowhere else to come from.
        fn paused_frame(state: &mut Solium, now: Duration) {
            state.flush_resize(now);
            state.settle_resize_hold(now);
            state.settle_resize_bridge(now);
        }

        /// Everything a tiled-drag test needs: a compositor, a client, a mapped
        /// window at a known place, and a clean configure log.
        ///
        /// Returned as a tuple rather than a struct because the borrow checker
        /// wants the queue and the client separately at every call site.
        macro_rules! tiled_fixture {
            ($display:ident, $state:ident, $conn:ident, $queue:ident, $client:ident, $qh:ident) => {
                let mut $display =
                    Display::<Solium>::new().expect("creating a test wayland display");
                let mut $state = Solium::new($display.handle());
                // No decoration, for the reason `scale_resend` gives at length:
                // building a Qt scene inside a process that already holds a raw
                // libwayland connection aborts the whole test binary.
                $state
                    .decorations
                    .set_style(&mut $state.panes, Some("none".to_string()));
                let ($conn, mut $queue, mut $client) = connect(&mut $display, &mut $state);
                let $qh = $queue.handle();
            };
        }

        /// **#124 review, finding 2: a client that rounds must not move the
        /// seam on the first frame.**
        ///
        /// **The cross-boundary test, and the only one there is.** Every other
        /// test of this gesture lives on one side of the call or the other:
        /// `input::resize::dragged_edge_tests` is arithmetic on rectangles the
        /// test itself made up, and `solium_layout::tree::dragged_edge_tests`
        /// feeds the tree a number the same tree produced. Neither can see the
        /// link that actually broke — `Solium`'s rectangle for a pane against
        /// the tree's — because neither crosses it. This one does: a real
        /// `Tiling` is laid out, placed through `Solium::place` exactly as
        /// `tiling.apply` places it, disagreed with by a real client over the
        /// real protocol, and then the number the compositor would hand a
        /// layout on the first frame of a drag is fed back into that same tree.
        ///
        /// What #124 shipped read the edge off `Solium::pane_outer`, which goes
        /// through `pane_geometry` and answers `real_geometry` for a mapped
        /// client with no hold live: the space's location paired with the size
        /// the *client* committed. So the first frame handed the layout the
        /// client's edge and `drag_seam` obediently moved the seam there,
        /// shifting the whole column by the client's rounding residue with the
        /// pointer still on the pixel it pressed.
        ///
        /// **Eight pixels, which is what makes it the nasty kind.** The client
        /// below answers a cell short, which is what every terminal does to
        /// every configure it is ever sent; `crate::resizing` puts the threshold
        /// for calling such an answer a refusal rather than a rounding at
        /// `max(asked / 20, CELL)`, so this is deliberately *under* it — the
        /// compositor is meant to accept it and does. It is at or above the
        /// half-gap that was the whole of #120, and it is silent.
        ///
        /// **A right edge, because a left edge cannot show it.** `real.loc` is
        /// compositor-set and `real.size` is the client's, so `pane_outer`'s
        /// left and top edges are exact and its right and bottom carry the whole
        /// of the disagreement. A version of this test on `Edge::Left` passes
        /// against the defect.
        ///
        /// **Off-centre first**, for the reason
        /// `solium_layout::tree::dragged_edge_tests::a_window_handed_its_own_edge_does_not_move`
        /// gives at length: a fixture at `split: 0.5` is where a skew is least
        /// visible, and a test that cannot fail is not evidence. The seam is
        /// dragged somewhere lopsided and re-placed before the client is ever
        /// asked to disagree.
        ///
        /// Confirmed failing against `ec1da24` rather than assumed: the value
        /// that commit would have sent is computed here from the same live
        /// compositor state and fed into a clone of the same tree, and the
        /// second half of this test pins that it moves the window. Both halves
        /// are needed — the first alone would pass on a tree that ignored
        /// `drag_seam` entirely.
        #[test]
        fn a_client_that_rounds_its_size_does_not_move_the_seam() {
            use solium_layout::tree::{Edge, Tiling};

            tiled_fixture!(display, state, conn, queue, client, qh);
            let (left, _left_toplevel, left_surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            let (right, _right_toplevel, _right_surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&left)
                .expect("a client in the space has a pane");
            let (left_id, right_id) = (state.window_id(&left), state.window_id(&right));

            let (area, settings) = (tiled_area(), tiled_settings());
            let mut tiling = Tiling::new();
            tiling.insert(left_id, None, None, area, settings);
            tiling.insert(right_id, Some(left_id), None, area, settings);
            sweep(&mut state, &tiling);

            // Lopsided, so a skew that is self-cancelling at the midpoint of
            // the seam's box cannot hide in this fixture.
            tiling.drag_seam(left_id, Edge::Right, (340.0, 300.0), area, settings);
            sweep(&mut state, &tiling);

            let slot = leaf_of(&tiling, left_id);
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a rect from a layout is screen-sized, and `Solium::place` \
                          rounds these same numbers the same way"
            )]
            let asked = at(slot.x as i32, slot.y as i32, slot.w as i32, slot.h as i32);

            // First the obedient client, which is the round trip this whole
            // test rests on: what `tree:layout` returned, through `sol.place`,
            // is what the pane's rectangle becomes -- so `pane_outer` and the
            // layout's own answer agree whenever a client does as it is told.
            // The defect below is entirely about the case where one does not.
            commit_buffer(&client, &qh, &left_surface, asked.size.w, asked.size.h);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            state.sync_panes();
            assert_eq!(
                state.pane_outer_of(pane),
                Some(asked),
                "a client at the size it was asked for puts `pane_outer` on \
                 the layout's own rectangle"
            );

            // And now the same client answering a cell short. This is the one
            // thing no rectangle arithmetic can fake, and the whole reason this
            // test needs a real client.
            let short = asked.size.w - 8;
            commit_buffer(&client, &qh, &left_surface, short, asked.size.h);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            state.sync_panes();
            assert_eq!(
                state.pane_outer_of(pane).map(|outer| outer.size.w),
                Some(short),
                "the client committed a size of its own and the compositor \
                 took it -- if it had not, this test would be checking nothing"
            );

            // The drag begins. `begin_resize` is what both grab sites call
            // first, and its answer is `began` -- the client's rectangle.
            let began = state.begin_resize(&left).expect("a mapped pane");
            let laid_out = state
                .pane_laid_out(&left)
                .expect("a pane a layout has placed");

            // The first frame: the pointer has not moved.
            let grab: Point<f64, Logical> =
                (f64::from(began.loc.x + began.size.w) - 3.0, 300.0).into();
            let sent = crate::input::resize::dragged_edge(laid_out, ResizeEdge::Right, grab, grab);

            let mut unmoved = tiling.clone();
            unmoved.drag_seam(left_id, Edge::Right, sent, area, settings);
            let after = leaf_of(&unmoved, left_id);
            assert!(
                (after.x - slot.x).abs() < 0.5
                    && (after.y - slot.y).abs() < 0.5
                    && (after.w - slot.w).abs() < 0.5
                    && (after.h - slot.h).abs() < 0.5,
                "a drag that has not moved must not move the seam. The layout \
                 is handed {sent:?}; its own edge is at {:?}. {slot:?} became \
                 {after:?}",
                (slot.x + slot.w, slot.y + slot.h)
            );

            // And what `ec1da24` sent, from the same state, into the same
            // tree. `began` is the rectangle that commit derived its edge from,
            // and it is a different rectangle from `laid_out` by exactly the
            // eight pixels the client kept -- asserted, because if they were
            // ever equal the half below would be testing the same thing twice.
            assert_ne!(
                began, laid_out.0,
                "the client's rectangle and the layout's have to differ here, \
                 or the defect this test is about cannot arise"
            );
            let shipped = crate::input::resize::dragged_edge(
                crate::input::resize::LaidOut(began),
                ResizeEdge::Right,
                grab,
                grab,
            );
            let mut moved = tiling.clone();
            moved.drag_seam(left_id, Edge::Right, shipped, area, settings);
            let jumped = leaf_of(&moved, left_id);
            assert!(
                (jumped.w - slot.w).abs() > 4.0,
                "reading the edge off the client's rectangle moved the seam by \
                 the client's rounding on frame one, which is the defect: \
                 {slot:?} became {jumped:?}"
            );
        }

        /// Everything a #133 test needs after the fixture: one window in a
        /// one-leaf tree, swept through `Solium::place` as `tiling.apply`
        /// would, with every finished transform retired so that what is drawn
        /// is what the pane's own geometry says -- the state a desktop is in
        /// between gestures. Returns the pane and the tile it was given.
        fn tiled_alone(
            state: &mut Solium,
            window: &Window,
        ) -> (crate::pane::PaneId, Rectangle<i32, Logical>) {
            use solium_layout::tree::Tiling;

            state.sync_panes();
            let pane = state
                .panes
                .id_of(window)
                .expect("a client in the space has a pane");
            let mut tiling = Tiling::new();
            tiling.insert(
                state.window_id(window),
                None,
                None,
                tiled_area(),
                tiled_settings(),
            );
            sweep(state, &tiling);
            let tile = state
                .panes
                .get(pane)
                .and_then(Pane::placed)
                .expect("a pane the layout has just placed is in a tile");
            state.settle(state.clock.now());
            (pane, tile)
        }

        /// A frame's worth of bookkeeping after a client has committed: the
        /// space into the panes, and finished transforms retired.
        fn a_frame(state: &mut Solium) {
            state.sync_panes();
            state.settle(state.clock.now());
        }

        /// **#133: a tiled client that commits more than its tile is held
        /// inside it**, and the hit test stops where the tile does.
        ///
        /// Two windows side by side, and the left one's client answers with a
        /// buffer 120 pixels wider than its tile and 8 shorter -- a browser at
        /// its minimum width, with a terminal's cell-grid residue on the other
        /// axis. On stage `pane_geometry` answered the committed size, so the
        /// pane, its frame canvas and its hit test all reached 120 pixels into
        /// the right-hand window, and a press there went to the left one.
        ///
        /// The left window is raised first, so that it is the one a hit test
        /// meets first. Without that the right window would win the overlap by
        /// being on top, and the last two assertions would pass on stage.
        #[test]
        fn a_tiled_client_that_commits_more_than_its_tile_is_held_inside_it() {
            use solium_layout::tree::Tiling;

            tiled_fixture!(display, state, conn, queue, client, qh);
            let (left, _left_toplevel, left_surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            let (right, _right_toplevel, right_surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&left)
                .expect("a client in the space has a pane");
            let beside_pane = state
                .panes
                .id_of(&right)
                .expect("a client in the space has a pane");
            let (left_id, right_id) = (state.window_id(&left), state.window_id(&right));

            let (area, settings) = (tiled_area(), tiled_settings());
            let mut tiling = Tiling::new();
            tiling.insert(left_id, None, None, area, settings);
            tiling.insert(right_id, Some(left_id), None, area, settings);
            sweep(&mut state, &tiling);
            let placed = |state: &Solium, pane| {
                state
                    .panes
                    .get(pane)
                    .and_then(Pane::placed)
                    .expect("a pane the layout has placed is in a tile")
            };
            let (tile, beside) = (placed(&state, pane), placed(&state, beside_pane));
            assert!(
                beside.loc.x > tile.loc.x + tile.size.w,
                "the fixture is two tiles side by side, left then right: {tile:?}, {beside:?}"
            );

            // The right-hand client does as it is told, so its tile is all
            // its own; the left one does not.
            commit_buffer(&client, &qh, &right_surface, beside.size.w, beside.size.h);
            let wide = (tile.size.w + 120, tile.size.h - 8);
            commit_buffer(&client, &qh, &left_surface, wide.0, wide.1);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            state.space.raise_element(&left, false);
            a_frame(&mut state);
            assert_eq!(
                left.geometry().size,
                Size::from(wide),
                "the client really did commit more than its tile, or there is \
                 nothing here to hold"
            );

            assert_eq!(
                state.pane_outer_of(pane),
                Some(Rectangle::new(tile.loc, (tile.size.w, wide.1).into())),
                "cut to the tile across, where the client is too wide, and left \
                 at the client's own height, where it is short of the tile"
            );
            assert_eq!(
                state.pane_laid_out(&left).map(|laid_out| laid_out.0),
                Some(tile),
                "a tiled edge drag still starts from the layout's rectangle (#124)"
            );
            let drawn = state.drawn(pane, state.pane_outer_of(pane).expect("the pane is here"));
            assert!(
                (drawn.rect.size.w - f64::from(tile.size.w)).abs() < 0.5,
                "and drawn at the tile's width, which is what the frame canvas \
                 and the titlebar are sized from: {drawn:?}"
            );

            // Inside the right-hand tile, and inside the left client's
            // committed width.
            let point: Point<f64, Logical> =
                (f64::from(beside.loc.x + 20), f64::from(beside.loc.y + 100)).into();
            assert!(
                point.x < f64::from(tile.loc.x + wide.0),
                "the point has to be one the oversized client would reach"
            );
            assert_eq!(
                state.window_under(point).map(|(window, _)| window),
                Some(right.clone()),
                "a press in the right-hand tile is the right-hand window's, \
                 whatever the left one committed"
            );
            let right_surface = right
                .toplevel()
                .map(|toplevel| toplevel.wl_surface().clone());
            assert_eq!(
                state.surface_under(point).map(|(surface, _)| surface),
                right_surface,
                "and so is the pointer's motion there"
            );
        }

        /// **Maximised is not tiled**: the work area a maximise configures is
        /// not cut down to the tile the window left.
        ///
        /// `Pane::placed` is the tile a client is held inside, and on stage
        /// `toggle_maximize` never touched it -- harmless while nothing read
        /// it but #124's edge drag. With the cap it is a maximised window
        /// drawn in the corner its tile used to occupy.
        #[test]
        fn a_maximised_window_is_not_held_in_its_old_tile() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let output = one_screen(&mut state);
            let screen = state
                .space
                .output_geometry(&output)
                .expect("the monitor is mapped");
            let (window, toplevel, surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            let (pane, tile) = tiled_alone(&mut state, &window);
            assert!(
                tile.size.w < screen.size.w && tile.size.h < screen.size.h,
                "the tile has to be smaller than the monitor, or a cap to it \
                 would cut nothing: {tile:?}"
            );

            state.toggle_maximize(&window);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert_eq!(
                last_configured(&client, &toplevel),
                Some((screen.size.w, screen.size.h)),
                "the client was asked for the work area"
            );
            commit_buffer(&client, &qh, &surface, screen.size.w, screen.size.h);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            a_frame(&mut state);

            assert_eq!(
                state.panes.get(pane).and_then(Pane::placed),
                None,
                "a maximised window is in no tile"
            );
            assert_eq!(
                state.pane_outer_of(pane),
                Some(screen),
                "and it is the size it was maximised to, not the tile it left"
            );
        }

        /// **And the way back puts it in the tile again**, so a tiled edge drag
        /// started from it begins at the layout's rectangle (#124).
        ///
        /// The client answers a cell short on the way back, the way a terminal
        /// does, so that the layout's rectangle and the pane's own differ: a
        /// window that came back in no tile would have `pane_laid_out` fall
        /// back to the client's rectangle, and a drag begun from it would move
        /// the seam by the client's rounding on its first frame.
        #[test]
        fn a_restored_window_goes_back_into_its_tile() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let output = one_screen(&mut state);
            let screen = state
                .space
                .output_geometry(&output)
                .expect("the monitor is mapped");
            let (window, _toplevel, surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            let (pane, tile) = tiled_alone(&mut state, &window);

            state.toggle_maximize(&window);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            commit_buffer(&client, &qh, &surface, screen.size.w, screen.size.h);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            a_frame(&mut state);

            state.toggle_maximize(&window);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            let short = tile.size.w - 8;
            commit_buffer(&client, &qh, &surface, short, tile.size.h);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            a_frame(&mut state);

            assert_eq!(
                state.panes.get(pane).and_then(Pane::placed),
                Some(tile),
                "the un-maximise put the window back in the tile it left"
            );
            assert_eq!(
                state.pane_outer_of(pane).map(|outer| outer.size.w),
                Some(short),
                "the client kept a cell short of its tile, so the two \
                 rectangles below can be told apart"
            );
            assert_eq!(
                state.pane_laid_out(&window).map(|laid_out| laid_out.0),
                Some(tile),
                "and an edge drag starts from the layout's rectangle, not the \
                 client's"
            );
        }

        /// **Fullscreen is not tiled either**, and leaving it goes back into
        /// the tile.
        ///
        /// The same field and the same stage behaviour as the maximise above:
        /// `fullscreen_request` kept a way back and moved the window, and left
        /// `Pane::placed` saying it was still in its tile -- which with the cap
        /// is a video cut down to the corner of the monitor it was tiled in.
        #[test]
        fn a_fullscreen_window_is_not_held_in_its_old_tile() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let output = one_screen(&mut state);
            let screen = state
                .space
                .output_geometry(&output)
                .expect("the monitor is mapped");
            let (window, toplevel, surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            let (pane, tile) = tiled_alone(&mut state, &window);

            toplevel.set_fullscreen(None);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert_eq!(
                last_configured(&client, &toplevel),
                Some((screen.size.w, screen.size.h)),
                "the client was asked for the whole monitor"
            );
            commit_buffer(&client, &qh, &surface, screen.size.w, screen.size.h);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            a_frame(&mut state);

            assert_eq!(state.panes.get(pane).and_then(Pane::placed), None);
            assert_eq!(
                state.pane_outer_of(pane),
                Some(screen),
                "a fullscreen window covers its monitor, not the tile it left"
            );

            toplevel.unset_fullscreen();
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert_eq!(
                state.panes.get(pane).and_then(Pane::placed),
                Some(tile),
                "and leaving fullscreen puts it back in that tile"
            );
        }

        /// **A window a layout lets go of is not held in the tile it had.**
        ///
        /// `sol.unplace` is what `modes.use` sends for every window when the
        /// layout in charge changes: the mode is the script's, so this is the
        /// only way the compositor hears that a window is floating now. Without
        /// it, a window left in its tile by a switch to floating is cut back to
        /// that tile the moment its client commits more -- which a floating
        /// window does whenever it is resized or grows itself.
        #[test]
        fn a_window_let_go_by_its_layout_is_not_held_in_its_old_tile() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (window, _toplevel, surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            let (pane, tile) = tiled_alone(&mut state, &window);

            state.apply(Outcome {
                commands: vec![Command::Unplace {
                    id: state.window_id(&window),
                }],
                ..Outcome::default()
            });
            let grown = (tile.size.w + 200, tile.size.h + 50);
            commit_buffer(&client, &qh, &surface, grown.0, grown.1);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            a_frame(&mut state);

            assert_eq!(state.panes.get(pane).and_then(Pane::placed), None);
            assert_eq!(
                state.pane_outer_of(pane).map(|outer| outer.size),
                Some(Size::from(grown)),
                "a floating window is the size its client committed"
            );
        }

        /// **A dialog a layout centres is placed and not tiled**, through
        /// `sol.place` with `tile = false`.
        ///
        /// A dialog is floating whichever layout is running -- `dialogs.lua`
        /// centres it over its parent at the size it had -- and one that grows
        /// after it was centred, as a file chooser settling on its size does,
        /// must be drawn at the size it grew to. A layout's ordinary placement
        /// beside it is still a tile, which the last assertion holds the
        /// fixture to.
        #[test]
        fn a_dialog_a_layout_centres_is_not_held_in_its_rect() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (window, _toplevel, surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&window)
                .expect("a client in the space has a pane");
            let id = state.window_id(&window);
            let rect = Rect {
                x: 300.0,
                y: 200.0,
                w: 400.0,
                h: 300.0,
            };
            let instant = AnimationSpec {
                duration: Duration::ZERO,
                ..AnimationSpec::default()
            };
            state.apply(Outcome {
                commands: vec![Command::Place {
                    id,
                    rect,
                    animation: instant,
                    tile: false,
                }],
                ..Outcome::default()
            });
            commit_buffer(&client, &qh, &surface, 520, 380);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            a_frame(&mut state);

            assert_eq!(state.panes.get(pane).and_then(Pane::placed), None);
            assert_eq!(
                state.pane_outer_of(pane),
                Some(at(300, 200, 520, 380)),
                "placed where the layout said, at the size the client grew to"
            );

            state.apply(Outcome {
                commands: vec![Command::Place {
                    id,
                    rect,
                    animation: instant,
                    tile: true,
                }],
                ..Outcome::default()
            });
            a_frame(&mut state);
            assert_eq!(
                state.pane_outer_of(pane),
                Some(at(300, 200, 400, 300)),
                "the same rect as a tile holds the same client inside it"
            );
        }

        /// **A pane under a resize hold is bridged, not cut to its tile.**
        ///
        /// While an edge is being dragged the dragged rectangle is the truth
        /// and the client's last buffer is stretched or held into it by
        /// `resizing::factor` (#113, #123). The tile the drag is moving is
        /// `Pane::placed` all the while, so a cut to it would cut the bridge:
        /// `Solium::tile_of` answers `None` under a hold, and the committed
        /// size is shown whole. Here the client is larger than the rectangle
        /// the drag has reached, which is exactly the case a cut would change.
        #[test]
        fn a_held_pane_is_bridged_and_not_cut_to_its_tile() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (window, _toplevel, surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            state.map_stacked(window.clone(), (400, 300), false);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&window)
                .expect("a client in the space has a pane");
            commit_buffer(&client, &qh, &surface, 400, 300);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            state.sync_panes();

            let outer = at(400, 300, 200, 150);
            let request = ResizeRequest {
                window: window.clone(),
                wanted: outer,
                edge_at: (600.0, 450.0),
                edges: ResizeEdge::BottomRight,
            };
            state.begin_resize(&window);
            tiled_frame(&mut state, &request, pane, outer, Duration::ZERO);
            assert!(state.holding_resize(pane), "the drag is holding the pane");
            assert_eq!(
                state.panes.get(pane).and_then(Pane::placed),
                Some(outer),
                "and the layout has it in the dragged tile, which a cut would use"
            );

            let held = state.panes.get(pane).expect("the pane is still here");
            assert_eq!(
                state.shown_size(held, window.geometry().size),
                Size::from((400, 300)),
                "the whole of the client's last buffer is shown, to be bridged \
                 into the dragged rectangle rather than cut to it"
            );
            assert_eq!(
                state.pane_geometry(held),
                outer,
                "and the pane is the dragged rectangle, as #113 made it"
            );
        }

        /// **A floating window brought back onto a screen is still floating.**
        ///
        /// `rescue_offscreen` reaches `move_pane`, which is where a layout's
        /// tile is written -- so on the way to #133 a window lost off a
        /// departed monitor came back *tiled* at the rectangle it was rescued
        /// to, and was cut to it from then on. A rescue is not a layout's
        /// opinion, and keeps a pane's standing as it found it.
        #[test]
        fn a_floating_window_brought_back_onto_a_screen_is_not_given_a_tile() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let _output = one_screen(&mut state);
            let (window, _toplevel, surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            state.map_stacked(window.clone(), (-5000, 300), false);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&window)
                .expect("a client in the space has a pane");

            state.rescue_offscreen();
            assert!(
                state
                    .pane_outer_of(pane)
                    .is_some_and(|outer| state.on_any_output(outer)),
                "the rescue brought the window back, or it proves nothing"
            );
            commit_buffer(&client, &qh, &surface, 700, 500);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            a_frame(&mut state);

            assert_eq!(state.panes.get(pane).and_then(Pane::placed), None);
            assert_eq!(
                state.pane_outer_of(pane).map(|outer| outer.size),
                Some(Size::from((700, 500))),
                "and it is the size its client committed"
            );
        }

        /// The work area every tiled-tree fixture in this module lays out over.
        fn tiled_area() -> solium_layout::Rect {
            solium_layout::Rect::new(0.0, 0.0, 1000.0, 600.0)
        }

        /// The shipped gap, because at `gap: 0` the two sides of a seam are one
        /// line and half the arithmetic under test disappears.
        fn tiled_settings() -> solium_layout::Settings {
            solium_layout::Settings {
                gap: 12.0,
                split: 0.5,
                ..solium_layout::Settings::default()
            }
        }

        /// Where a tree has put one window. `rect_at` in the layout crate's own
        /// suite, which is not public.
        fn leaf_of(tiling: &solium_layout::tree::Tiling, id: u64) -> solium_layout::Rect {
            tiling
                .layout(tiled_area(), tiled_settings())
                .into_iter()
                .find(|(other, _)| *other == id)
                .expect("the window is in the tree")
                .1
        }

        /// One layout sweep, which is what `tiling.apply` is in Lua: every leaf
        /// placed, every frame, whether it moved or not.
        ///
        /// `sol.place` is `Solium::place`, and a script's rect is the pane's
        /// *outer* rectangle — `place` subtracts the insets itself.
        fn sweep(state: &mut Solium, tiling: &solium_layout::tree::Tiling) {
            for (id, rect) in tiling.layout(tiled_area(), tiled_settings()) {
                state.place(
                    id,
                    Rect {
                        x: rect.x,
                        y: rect.y,
                        w: rect.w,
                        h: rect.h,
                    },
                    AnimationSpec {
                        duration: Duration::ZERO,
                        ..AnimationSpec::default()
                    },
                    Duration::ZERO,
                    Standing::Tile,
                );
            }
        }

        /// **A tiled drag measures from the grab, not from the last frame.**
        ///
        /// `ResizeGrab` freezes two rectangles at the press and this pins why
        /// the second of them has to be one of them. `laid_out` is where the
        /// layout had this pane when the button went down, and the layout moves
        /// the pane on *every frame of the drag* — that is what a seam moving
        /// means. So a version that asked `Solium::pane_laid_out` afresh each
        /// frame and added the gesture's total travel to whatever came back
        /// would add the previous frame's travel a second time, and the frame
        /// before that a third. Not drift: the whole of the motion, compounding,
        /// for as long as the button is held.
        ///
        /// It is an easy thing to write, because "ask the layout where the pane
        /// is now" reads like the honest version. The first half below is the
        /// fixture earning its keep — the compositor really is moving this pane
        /// under the drag — and the second is the runaway, computed by doing
        /// exactly that to a second copy of the same tree.
        ///
        /// A *position* built from a frozen base and a total delta, which is
        /// what `ResizeRequest` promises and what keeps `drag_seam` idempotent.
        /// Both halves of that matter and neither implies the other: the base
        /// is frozen so the drag does not read its own output back, and the
        /// delta is total rather than per-frame so it does not accumulate
        /// rounding. See `crate::input::resize::dragged_edge`.
        #[test]
        fn a_tiled_drag_measures_from_where_the_grab_began_and_not_from_the_last_frame() {
            use solium_layout::tree::{Edge, Tiling};

            tiled_fixture!(display, state, conn, queue, client, qh);
            let (left, _left_toplevel, _left_surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            let (right, _right_toplevel, _right_surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            state.sync_panes();
            let (left_id, right_id) = (state.window_id(&left), state.window_id(&right));

            let (area, settings) = (tiled_area(), tiled_settings());
            let mut tiling = Tiling::new();
            tiling.insert(left_id, None, None, area, settings);
            tiling.insert(right_id, Some(left_id), None, area, settings);
            sweep(&mut state, &tiling);

            let began = leaf_of(&tiling, left_id);
            let frozen = state
                .pane_laid_out(&left)
                .expect("a pane a layout has placed");
            let from: Point<f64, Logical> =
                (f64::from(frozen.0.loc.x + frozen.0.size.w), 300.0).into();

            // Three frames of one gesture. The numbers are the pointer's total
            // travel from the press, which is what a grab has -- never the step
            // since the last frame.
            let mut the_layout_moved_it = false;
            for total in [10.0, 40.0, 90.0] {
                let now: Point<f64, Logical> = (from.x + total, from.y).into();
                let sent = crate::input::resize::dragged_edge(frozen, ResizeEdge::Right, from, now);
                tiling.drag_seam(left_id, Edge::Right, sent, area, settings);
                sweep(&mut state, &tiling);
                // A whole frame, configures and all. The client is never made
                // to answer here -- what this test is about happens whether it
                // does or not -- but a sweep that never reached the wire would
                // be a different code path from the one a drag takes.
                pump(
                    &mut display,
                    &mut state,
                    &conn,
                    &qh,
                    &mut queue,
                    &mut client,
                );
                state.sync_panes();
                the_layout_moved_it |= state.pane_laid_out(&left) != Some(frozen);
            }
            assert!(
                the_layout_moved_it,
                "the layout has to actually move this pane under the drag, or \
                 freezing its rectangle costs nothing and this test pins nothing"
            );

            let ended = leaf_of(&tiling, left_id);
            assert!(
                (ended.w - (began.w + 90.0)).abs() < 0.5,
                "the pointer travelled 90 from the press, so the edge is 90 \
                 from where it was: {began:?} became {ended:?}"
            );

            // And the same three frames against a base re-read each time, which
            // is the mistake this is here to keep out.
            let mut runaway = Tiling::new();
            runaway.insert(left_id, None, None, area, settings);
            runaway.insert(right_id, Some(left_id), None, area, settings);
            for total in [10.0, 40.0, 90.0] {
                let live = leaf_of(&runaway, left_id);
                runaway.drag_seam(
                    left_id,
                    Edge::Right,
                    (live.x + live.w + total, 300.0),
                    area,
                    settings,
                );
            }
            let flew = leaf_of(&runaway, left_id);
            assert!(
                flew.w - began.w > 130.0,
                "re-reading the layout's rectangle each frame adds every \
                 earlier frame's travel again -- 140 rather than 90 here, and \
                 unbounded on a real gesture: {began:?} became {flew:?}"
            );
        }

        /// **Issue #123, the rate half: a tiled drag configured its client once
        /// per frame.**
        ///
        /// `crate::resizing::TELL_EVERY` is consulted by `Hold::dragged` and by
        /// nothing else, and until now a tiled pane had no hold — so `move_pane`
        /// sent a configure on every frame of every drag, which is sixty a
        /// second. `resizing.rs` says in its own words why that fails: a client
        /// that cannot render at that rate does not try, it falls behind, and
        /// the window stutters against the pointer rather than following it.
        ///
        /// **Every frame asks for a different width**, deliberately. Smithay
        /// deduplicates a configure that repeats the size it last sent, so a
        /// test that dragged a window to the same place twice would be green
        /// against a compositor with no throttle at all.
        #[test]
        fn a_tiled_drag_configures_on_the_throttle_and_not_once_per_frame() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (window, _toplevel, _surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            state.map_stacked(window.clone(), (400, 300), false);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&window)
                .expect("a client in the space has a pane");

            // Opening a window configures it, and that configure is not this
            // test's subject.
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            client.configures.clear();

            let request = ResizeRequest {
                window: window.clone(),
                wanted: at(400, 300, 300, 200),
                edge_at: (700.0, 500.0),
                edges: ResizeEdge::Right,
            };
            state.begin_resize(&window);

            // Six frames at sixteen milliseconds, which is what a drag at sixty
            // hertz is and is entirely inside one hundred-millisecond interval.
            for (millis, width) in [
                (0, 300),
                (16, 302),
                (32, 304),
                (48, 306),
                (64, 308),
                (80, 310),
            ] {
                tiled_frame(
                    &mut state,
                    &request,
                    pane,
                    at(400, 300, width, 200),
                    Duration::from_millis(millis),
                );
            }
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert_eq!(
                client.configures.len(),
                1,
                "six frames of a claimed drag inside one `TELL_EVERY` must be \
                 one configure, not six: the first offer goes out on the frame \
                 it is decided and the rest wait. Got {:?}",
                client.configures
            );

            // And past the interval the client hears again, because the throttle
            // is a rate and not a gate: a drag that went on for a second with a
            // single configure in it would end a long way from the pointer.
            tiled_frame(
                &mut state,
                &request,
                pane,
                at(400, 300, 340, 200),
                Duration::from_millis(120),
            );
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert_eq!(
                client.configures.len(),
                2,
                "the interval has passed, so the next frame's rectangle is sent"
            );
            assert_eq!(
                client.configures.last().map(|&(_, w, h)| (w, h)),
                Some((340, 200)),
                "and what is sent is the rectangle of the frame that sent it, \
                 not a stale one the throttle had been sitting on"
            );
        }

        /// **Issue #123, the flicker half: a tiled pane drew the size its client
        /// last committed.**
        ///
        /// `pane_geometry` lets a pane's slot outrank its client only while
        /// `holding_resize` is true, which was false for every tiled drag, so
        /// `pane_outer` and its twenty-odd callers read `real_geometry` — the
        /// size the client last *agreed to*, which during a drag is frames
        /// behind the layout.
        ///
        /// Two assertions and they are not the same one twice. The first is the
        /// inversion. The second is `sync_panes`, which copies the space into
        /// every pane's slot once a frame and so used to undo the layout's
        /// rectangle between the frame that drew it and the next one: that
        /// alternation — the layout's rectangle on a frame carrying a motion,
        /// the client's on a frame without one — is what the user sees as
        /// stutter.
        #[test]
        fn a_tiled_pane_draws_the_layouts_rectangle_while_its_client_lags() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (window, _toplevel, _surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            state.map_stacked(window.clone(), (400, 300), false);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&window)
                .expect("a client in the space has a pane");
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );

            let request = ResizeRequest {
                window: window.clone(),
                wanted: at(400, 300, 300, 200),
                edge_at: (700.0, 500.0),
                edges: ResizeEdge::Right,
            };
            state.begin_resize(&window);
            let outer = at(400, 300, 300, 200);
            tiled_frame(&mut state, &request, pane, outer, Duration::ZERO);

            assert_eq!(
                window.geometry().size,
                Size::from((64, 64)),
                "the fixture's client never answers a configure, which is what \
                 makes it a stand-in for one that is merely slow -- if it had \
                 answered there would be no disagreement to test"
            );
            let geometry = |state: &Solium| {
                state.pane_geometry(state.panes.get(pane).expect("the pane is still here"))
            };
            assert_eq!(
                geometry(&state),
                outer,
                "the layout's rectangle is what the pane is, immediately. \
                 Falling back to the client's committed size draws a window \
                 the size it was before the drag started at the position the \
                 drag has reached"
            );

            state.sync_panes();
            assert_eq!(
                geometry(&state),
                outer,
                "and it survives the frame. `sync_panes` writes the space into \
                 every pane's slot, and the space's size is whatever the client \
                 last committed, so without `held_slot` the layout's authority \
                 lasts exactly until the next frame's reconciliation"
            );
        }

        /// **The trap `crate::resizing` is built around, on the tiled path: a
        /// client that refuses must not be stretched for ever.**
        ///
        /// A client with a minimum size — Firefox has one, a terminal rounds to
        /// its cell grid — answers a configure with a size of its own. Nothing
        /// on the tiled path noticed: `resize_fill` answered `None`, `factor`
        /// took the free-to-grow arm, and the buffer was scaled without limit
        /// towards a rectangle its client had already walked away from. That is
        /// the permanently-soft window this module's documentation says cost a
        /// bug once already, and #115 — reading the client's minimum — is still
        /// unread, so this is the only thing standing between a refusal and a
        /// blur.
        ///
        /// Both halves are asserted, because they happen at different times: the
        /// stretch stops *during* the gesture, and the pane lands on the
        /// client's own size when the gesture ends.
        #[test]
        fn a_tiled_client_that_refuses_a_size_stops_the_stretch_and_ends_the_bridge() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (window, _toplevel, surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            state.map_stacked(window.clone(), (400, 300), false);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&window)
                .expect("a client in the space has a pane");
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );

            let request = ResizeRequest {
                window: window.clone(),
                wanted: at(400, 300, 300, 200),
                edge_at: (700.0, 500.0),
                edges: ResizeEdge::Right,
            };
            state.begin_resize(&window);
            let outer = at(400, 300, 300, 200);
            tiled_frame(&mut state, &request, pane, outer, Duration::ZERO);
            assert_eq!(
                state.resize_fill(pane),
                Some(crate::resizing::Fill::Stretch),
                "nothing has been refused yet, so the configured fill stands"
            );

            // The answer, and it is not the one that was asked for: 120x90
            // where 300x200 was offered. A real client saying "this is my
            // minimum" says it exactly this way.
            commit_buffer(&client, &qh, &surface, 120, 90);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert_eq!(
                window.geometry().size,
                Size::from((120, 90)),
                "the client committed a size of its own, which is the whole \
                 scenario"
            );

            tiled_frame(&mut state, &request, pane, outer, Duration::from_millis(16));
            assert_eq!(
                state.resize_fill(pane),
                Some(crate::resizing::Fill::Hold),
                "a refusal overrides the configured fill. A stretch towards a \
                 size the client has walked away from never returns to 1, so \
                 the window stays soft until something else resizes it"
            );

            state.release_resize(&window);
            state.settle_resize();
            assert!(
                !state.holding_resize(pane),
                "any answer ends the bridge, and a refusal is an answer -- \
                 waiting out `PATIENCE` for a size that has already been \
                 declined only adds a quarter second of squashed window"
            );
            assert_eq!(
                state
                    .panes
                    .get(pane)
                    .expect("the pane is still here")
                    .slot()
                    .size,
                Size::from((120, 90)),
                "and the pane lands on the size the client chose rather than \
                 keeping a tile its client will never fill"
            );
        }

        /// **A window merely pushed aside by someone else's drag still needs its
        /// configure.**
        ///
        /// `move_pane` runs for every pane a layout touches, not just the one
        /// under the pointer: `tiling.apply` emits a placement for every leaf on
        /// every visible monitor, and a seam moving means at least two of them
        /// have genuinely changed. So the throttle has to be per pane. Retarget
        /// the single hold at "the pane the layout moved" instead — the obvious
        /// smaller fix — and the interval opened by the dragged window swallows
        /// the neighbour's one and only configure, which is a window that never
        /// hears its new size at all.
        ///
        /// Both counts are asserted from one drag, because they are the two
        /// halves of the same rule: throttled is not silenced, and silent is not
        /// throttled.
        #[test]
        fn a_pane_pushed_aside_by_another_panes_drag_is_configured_at_once() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (dragged, dragged_toplevel, _dragged_surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            let (beside, beside_toplevel, _beside_surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            state.map_stacked(dragged.clone(), (400, 300), false);
            state.map_stacked(beside.clone(), (700, 300), false);
            state.sync_panes();
            let dragged_pane = state
                .panes
                .id_of(&dragged)
                .expect("a client in the space has a pane");
            let beside_pane = state
                .panes
                .id_of(&beside)
                .expect("a client in the space has a pane");
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            client.configures.clear();

            let request = ResizeRequest {
                window: dragged.clone(),
                wanted: at(400, 300, 300, 200),
                edge_at: (700.0, 500.0),
                edges: ResizeEdge::Right,
            };
            state.begin_resize(&dragged);

            // The seam has not reached the neighbour yet, so the layout moves
            // one pane.
            tiled_frame(
                &mut state,
                &request,
                dragged_pane,
                at(400, 300, 300, 200),
                Duration::ZERO,
            );
            // Fifty milliseconds later — half an interval — the seam moves both.
            let later = Duration::from_millis(50);
            tiled_frame(
                &mut state,
                &request,
                dragged_pane,
                at(400, 300, 320, 200),
                later,
            );
            tiled_frame(
                &mut state,
                &request,
                beside_pane,
                at(720, 300, 180, 200),
                later,
            );

            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            let counted = |toplevel: &xdg_toplevel::XdgToplevel| {
                let id = wayland_client::Proxy::id(toplevel);
                client
                    .configures
                    .iter()
                    .filter(|(sent_to, ..)| sent_to == &id)
                    .count()
            };
            assert_eq!(
                counted(&dragged_toplevel),
                1,
                "the dragged pane moved on both frames and both were inside one \
                 interval, so its client hears once"
            );
            assert_eq!(
                counted(&beside_toplevel),
                1,
                "and the pane beside it moved for the first time on the second \
                 frame, so it hears at once -- its interval starts when it \
                 moves, not when somebody else did"
            );
        }

        /// **Only a live gesture arms a bridge.**
        ///
        /// The keyboard `nudge` reaches `move_pane` through `Scripts::key` and
        /// never through `settle_resize`, and so do a config reload, a monitor
        /// change, a decoration restyle, a workspace switch and
        /// `rescue_offscreen`. A hold armed by any of them could never be let
        /// go of: `release_resize` has exactly one caller and it is the pointer
        /// grab, so `Hold::settle` would answer `Waiting` for ever and the
        /// pane's slot and the space would be held apart for the life of the
        /// window — the never-resolved disagreement `PATIENCE` exists to
        /// prevent.
        ///
        /// The second assertion is the one that keeps the first honest: a fix
        /// that arms nothing by arming nothing is not a fix. The client still
        /// has to be told.
        #[test]
        fn a_keyboard_nudge_arms_no_bridge_and_still_reaches_its_client() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (window, _toplevel, _surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            state.map_stacked(window.clone(), (400, 300), false);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&window)
                .expect("a client in the space has a pane");
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            client.configures.clear();

            // No `arm_resize_gesture`, because nothing in a key dispatch calls
            // it. This is the whole of what a nudge is by the time it arrives.
            let was = state
                .pane_outer_of(pane)
                .expect("a mapped pane has a rectangle");
            state.move_pane(
                pane,
                at(400, 300, 300, 200),
                was,
                AnimationSpec::default(),
                Duration::ZERO,
                Standing::Tile,
            );

            assert!(
                !state.holding_resize(pane),
                "a keypress is not a gesture and has nothing that could ever \
                 end a hold"
            );
            assert!(
                state.resize_bridge.is_none(),
                "and no bridge was opened for it either"
            );
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert_eq!(
                client.configures.len(),
                1,
                "the window still has to hear its new size: arming nothing is \
                 only correct if it costs nothing"
            );
        }

        /// **A layout re-placing a pane exactly where it already is says
        /// nothing to its client.**
        ///
        /// `tree:layout` emits every leaf whether or not it moved, once per
        /// visible monitor, and a drag runs it once a frame — so in a dwindle
        /// tree the eight panes that did not move were configured sixty times a
        /// second each for the length of every gesture. Smithay hides half of
        /// that: `send_pending_configure` drops a configure that repeats the
        /// size it last sent, so on the wire the xdg clients saw nothing. The
        /// X11 arm has no such check and sent a real `ConfigureWindow` every
        /// time, for every X11 window in the layout.
        ///
        /// **Asserted against `offers_size` rather than against the wire**, and
        /// that is forced rather than chosen: the fixture speaks xdg, which is
        /// exactly the protocol whose own deduplication makes the defect
        /// invisible from the client's side. A test that counted configures
        /// here would pass against the unfixed compositor and prove nothing.
        #[test]
        fn a_layout_replacing_a_pane_where_it_already_is_tells_its_client_nothing() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (window, _toplevel, _surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            state.map_stacked(window.clone(), (400, 300), false);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&window)
                .expect("a client in the space has a pane");
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );

            // Where the client actually is: the fixture's buffer is 64x64 and
            // it was mapped at (400, 300).
            let settled = at(400, 300, 64, 64);
            assert_eq!(
                state.real_geometry(&window),
                Some(settled),
                "the client and the space agree before the sweep, which is the \
                 state every unchanged leaf is in on every frame of a drag"
            );
            assert!(
                !state.offers_size(pane, &window, settled, Duration::ZERO),
                "a leaf the layout re-placed where it already was has nothing \
                 to be told"
            );
            // A position it does not have is a real change, even at the same
            // size: for an X11 window `size_window` is the only thing that
            // carries a position at all, so deduplicating on size alone would
            // leave one told to stay where it no longer is.
            assert!(
                state.offers_size(pane, &window, at(500, 300, 64, 64), Duration::ZERO),
                "a move with no resize still has to reach the client"
            );
        }

        /// **The throttle had no trailing edge, so the end of every drag was
        /// never sent.**
        ///
        /// `Hold::dragged` is reached from `move_pane` and from `hold_resize`,
        /// and `settle_resize` reaches either only on a frame whose
        /// `pending_resize` carried a motion. So the offers a drag makes inside
        /// its last `TELL_EVERY` are written into the pane's slot, drawn from
        /// the pane's slot, and never put on the wire: the client sits at a size
        /// up to a tenth of a second stale while the pane is drawn where the
        /// pointer is, and the bridge between the two is the visible gap.
        ///
        /// **A paused pointer is the ordinary end of a drag**, not an edge
        /// case — people stop moving before they let go — and the gap is held
        /// for as long as the pause lasts. Before #123 the tiled path
        /// configured on every frame and had no tail at all, which makes this
        /// the regression of exactly the symptom that was reported.
        ///
        /// The pause is driven with `paused_frame`, which is the whole of what
        /// `settle_resize` runs on a frame with no motion. Against the code
        /// before this fix that call is `settle_resize_bridge` alone, and
        /// nothing in it sends anything.
        #[test]
        fn a_drag_that_pauses_still_tells_its_client_where_it_stopped() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (window, _toplevel, _surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            state.map_stacked(window.clone(), (400, 300), false);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&window)
                .expect("a client in the space has a pane");
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            client.configures.clear();

            let request = ResizeRequest {
                window: window.clone(),
                wanted: at(400, 300, 300, 200),
                edge_at: (700.0, 500.0),
                edges: ResizeEdge::Right,
            };
            state.begin_resize(&window);

            // Three frames of a drag well inside one interval. The first is the
            // pane joining the bridge, which goes out at once; the other two are
            // throttled, and the third is where the pointer stops.
            for (millis, width) in [(0, 300), (16, 340), (32, 380)] {
                tiled_frame(
                    &mut state,
                    &request,
                    pane,
                    at(400, 300, width, 200),
                    Duration::from_millis(millis),
                );
            }
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert_eq!(
                client.configures.last().map(|&(_, w, _)| w),
                Some(300),
                "the control: while the interval is running the client is still \
                 at the first offer, and the 80 pixels since are the gap"
            );

            // The pointer has stopped. No motion reaches `settle_resize`, so
            // nothing calls `move_pane` again — these are the frames a paused
            // drag is made of.
            for millis in [48, 64, 80, 96, 112, 128] {
                paused_frame(&mut state, Duration::from_millis(millis));
            }
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert_eq!(
                client.configures.last().map(|&(_, w, h)| (w, h)),
                Some((380, 200)),
                "once the interval passes the pane's rectangle has to reach its \
                 client whether or not the pointer moved again. Without a \
                 trailing flush the client stays 80 pixels behind a pane that is \
                 drawn where the pointer stopped, for as long as the pause \
                 lasts. Got {:?}",
                client.configures
            );
            assert_eq!(
                client.configures.len(),
                2,
                "and once, not once a frame: the flush is the throttle's \
                 trailing edge and not a way around it. Got {:?}",
                client.configures
            );
        }

        /// **A pane given a different client mid-gesture was configured through
        /// the old one.**
        ///
        /// `settle_resize_hold`, `settle_resize_bridge` and `release_bridge`
        /// all check `panes.get(pane).and_then(Pane::client)` against the window
        /// their hold remembers before touching anything. `flush_resize` did
        /// not, and it is the one that runs *first* on every frame — so a pane
        /// whose content was replaced while its hold was still alive sent the
        /// new client's slot to the old client, and read the old client's
        /// committed size back into the throttle's bookkeeping while it was
        /// there. `Pane::adopt` is how a pane's content is replaced without the
        /// pane changing: same id, same slot, different window, which is
        /// exactly the state the guard is about.
        ///
        /// **The control is
        /// [`a_drag_that_pauses_still_tells_its_client_where_it_stopped`],
        /// which is this test without the adoption**: the same fixture, the
        /// same two frames inside one interval, the same paused frame past it,
        /// and there the configure does arrive. A test that only asserts
        /// silence passes against a flush that was never going to fire, so the
        /// slot is asserted here as well — the pane is still holding the offer
        /// the throttle swallowed, which is the thing that would have been sent
        /// to the wrong client.
        #[test]
        fn a_pane_given_a_different_client_is_not_flushed_through_the_old_one() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (window, toplevel, _surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            state.map_stacked(window.clone(), (400, 300), false);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&window)
                .expect("a client in the space has a pane");
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );

            let request = ResizeRequest {
                window: window.clone(),
                wanted: at(400, 300, 300, 200),
                edge_at: (700.0, 500.0),
                edges: ResizeEdge::Right,
            };
            state.begin_resize(&window);
            // Two frames inside one interval: the first joins the bridge and
            // goes out at once, the second is throttled and is what a trailing
            // flush exists to send.
            for (millis, width) in [(0, 300), (16, 340)] {
                tiled_frame(
                    &mut state,
                    &request,
                    pane,
                    at(400, 300, width, 200),
                    Duration::from_millis(millis),
                );
            }

            // The pane's content is replaced. Its id and its slot are the same
            // ones the hold is holding; the window inside it is not.
            let (other, _other_toplevel, _other_surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            state
                .panes
                .get_mut(pane)
                .expect("the pane is still here")
                .adopt(other);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            client.configures.clear();

            // Past the interval, which is the frame the flush would fire on.
            paused_frame(&mut state, Duration::from_millis(120));
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            let id = wayland_client::Proxy::id(&toplevel);
            assert!(
                !client.configures.iter().any(|(sent_to, ..)| sent_to == &id),
                "the window this hold remembers no longer owns the pane, so the \
                 slot being flushed is not its rectangle to be told about. Got \
                 {:?}",
                client.configures
            );
            assert_eq!(
                state
                    .panes
                    .get(pane)
                    .expect("the pane is still here")
                    .slot()
                    .size,
                Size::from((340, 200)),
                "the control: the slot still carries the offer the throttle \
                 swallowed, so there was something for the flush to send"
            );
        }

        /// **The same tail on the floating path**, which `settle_resize_hold`
        /// looks like it covers and does not.
        ///
        /// `Hold::settle` decides whether a hold is over; it has never sent a
        /// configure and does not know how. So a paused floating drag sat on
        /// its last offer exactly as a tiled one did — the only thing that ever
        /// sent unconditionally was the release. Older than #123 rather than a
        /// regression of it, and fixed by the same flush, so it is asserted
        /// here rather than left to be rediscovered.
        #[test]
        fn a_floating_drag_that_pauses_also_tells_its_client() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (window, _toplevel, _surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            state.map_stacked(window.clone(), (400, 300), false);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&window)
                .expect("a client in the space has a pane");
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            client.configures.clear();
            state.begin_resize(&window);

            let dragged = |state: &mut Solium, width: i32, millis: u64| {
                let request = ResizeRequest {
                    window: window.clone(),
                    wanted: at(400, 300, width, 200),
                    edge_at: (f64::from(400 + width), 500.0),
                    edges: ResizeEdge::Right,
                };
                state.hold_resize(&request, Duration::from_millis(millis));
            };
            dragged(&mut state, 300, 0);
            dragged(&mut state, 340, 16);
            dragged(&mut state, 380, 32);
            for millis in [48, 64, 80, 96, 112, 128] {
                paused_frame(&mut state, Duration::from_millis(millis));
            }
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert!(
                state.holding_resize(pane),
                "the gesture is still live, which is what makes the pause a \
                 pause rather than an ending"
            );
            assert_eq!(
                client.configures.last().map(|&(_, w, h)| (w, h)),
                Some((380, 200)),
                "a floating drag's tail is the same tail. Got {:?}",
                client.configures
            );
        }

        /// **A pane the layout only *moved* was never told, for the whole of
        /// the gesture.**
        ///
        /// `offers_size`'s case (2) documents why the whole rectangle is
        /// compared and not just the size: `size_window` is the only thing that
        /// carries a position to an X11 client, because `map_stacked` moves the
        /// window in the space and says nothing to anybody. Case (1) compared
        /// sizes — `Hold::dragged` took one — so a bridged pane whose slot
        /// translates answered "nothing to say" and went on answering it until
        /// the button came up. `scrolling.lua`'s `widen` shifts every column
        /// sideways at an unchanged width, so that is the whole of an X11
        /// window's drag in the scrolling layout: stale geometry, and pointer
        /// coordinates routed to where the window used to be.
        ///
        /// **Asserted against `offers_size` rather than against the wire**, and
        /// forced rather than chosen, for the reason
        /// `a_layout_replacing_a_pane_where_it_already_is_tells_its_client_nothing`
        /// gives at length: the fixture speaks xdg, and smithay deduplicates a
        /// configure that repeats the last size it sent — which is every
        /// configure this test is about. Counting them would prove nothing.
        ///
        /// The existing neighbour test cannot see this: it moves the pane it is
        /// about *and* resizes it, and it moves it for the first time, so it
        /// never reaches case (1) at all.
        #[test]
        fn a_bridged_pane_that_only_moves_is_still_told_where_it_went() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (window, _toplevel, _surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            state.map_stacked(window.clone(), (400, 300), false);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&window)
                .expect("a client in the space has a pane");
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );

            let request = ResizeRequest {
                window: window.clone(),
                wanted: at(400, 300, 300, 200),
                edge_at: (700.0, 500.0),
                edges: ResizeEdge::Right,
            };
            state.begin_resize(&window);
            // The pane joins the bridge here, so what follows exercises case (1).
            tiled_frame(
                &mut state,
                &request,
                pane,
                at(400, 300, 300, 200),
                Duration::ZERO,
            );
            assert!(
                state.holding_resize(pane),
                "the pane has to be bridged, or this is testing case (2)"
            );

            // A whole interval later, so the throttle is not what is being
            // measured: the column slides 40 pixels sideways at exactly the
            // width it already had.
            assert!(
                state.offers_size(
                    pane,
                    &window,
                    at(440, 300, 300, 200),
                    crate::resizing::PATIENCE,
                ),
                "a bridged pane that translates has moved, and for an X11 client \
                 `size_window` is the only thing that will ever say so"
            );
            // And the dedup it must not have cost: the same rectangle twice is
            // still nothing to say.
            assert!(
                !state.offers_size(
                    pane,
                    &window,
                    at(440, 300, 300, 200),
                    crate::resizing::PATIENCE * 2,
                ),
                "comparing the whole rectangle must not turn into telling the \
                 client on every frame"
            );
        }

        /// **A terminal rounds; it does not refuse. `declined` could not tell
        /// the difference, so kitty never got the fill the user configured.**
        ///
        /// `Hold::note` records a decline for any answer that is not exactly
        /// the ask, and `fill` forced `Fill::Hold` for the rest of the gesture
        /// on the strength of it. A terminal answers with a whole number of
        /// character cells and so is a few pixels out on its very first answer
        /// and every one after it — which makes kitty, the ordinary tiled
        /// client, `declined` from the first frame of every seam drag.
        /// `Fill::Hold` deliberately leaves an uncovered strip while a pane
        /// grows, so what the user saw in place of their `stretch` was a band
        /// of background: a different wrong-looking frame rather than none, on
        /// the drag whose symptom is "frames with the wrong size".
        ///
        /// Both directions, because a fix that stops calling anything a refusal
        /// would ship the permanent blur this module was built to prevent.
        #[test]
        fn a_cell_grid_rounding_is_not_the_refusal_that_takes_the_stretch_away() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (window, _toplevel, surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            state.map_stacked(window.clone(), (400, 300), false);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&window)
                .expect("a client in the space has a pane");
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );

            let request = ResizeRequest {
                window: window.clone(),
                wanted: at(400, 300, 300, 200),
                edge_at: (700.0, 500.0),
                edges: ResizeEdge::Right,
            };
            state.begin_resize(&window);
            let outer = at(400, 300, 300, 200);
            tiled_frame(&mut state, &request, pane, outer, Duration::ZERO);

            // What kitty answers 300x200 with: the nearest whole number of
            // cells, which at an ordinary font is a handful of pixels short on
            // each axis.
            commit_buffer(&client, &qh, &surface, 294, 190);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert_eq!(
                window.geometry().size,
                Size::from((294, 190)),
                "the client answered with a size of its own, which is what a \
                 cell grid always does"
            );
            tiled_frame(&mut state, &request, pane, outer, Duration::from_millis(16));
            assert_eq!(
                state.resize_fill(pane),
                Some(crate::resizing::Fill::Stretch),
                "six pixels on a three-hundred pixel pane is a rounding, and \
                 the user's configured fill stands. Calling it a refusal costs \
                 the stretch on every terminal drag there will ever be"
            );

            // And the same client, refusing for real: a minimum width, which is
            // nothing like six pixels out.
            commit_buffer(&client, &qh, &surface, 120, 90);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            tiled_frame(&mut state, &request, pane, outer, Duration::from_millis(32));
            assert_eq!(
                state.resize_fill(pane),
                Some(crate::resizing::Fill::Hold),
                "a client that has walked away from the ask still loses the \
                 stretch: it will never bring the factor back to 1, and a \
                 window that never un-stretches is the trap this module is \
                 built around"
            );
        }

        /// **The drag's throttle was applied to every caller of `move_pane`.**
        ///
        /// A bridge outlives its gesture by up to `PATIENCE`, and `move_pane`
        /// is reached by a config reload, a `modes.use` from a keybinding, a
        /// workspace switch and `rescue_offscreen` as well as by a layout
        /// sweep. Case (1) looked the pane up in the bridge without asking
        /// whose sweep this was, so one of those landing inside a live bridge
        /// had its one and only configure swallowed by an interval somebody
        /// else opened — and nothing would resend it, because a keypress has no
        /// next frame. `move_pane` went on writing the slot regardless, so the
        /// pane was then drawn, stretched, at a rectangle its client had never
        /// been told about for the rest of the gesture.
        ///
        /// `resize_gesture` is set for the length of `trigger_resize` and by
        /// nothing else, which is exactly the question "is the sweep reaching
        /// this pane the live drag's own".
        #[test]
        fn a_reload_inside_a_live_bridge_is_not_throttled_by_the_drag() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (window, _toplevel, _surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            state.map_stacked(window.clone(), (400, 300), false);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&window)
                .expect("a client in the space has a pane");
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            client.configures.clear();

            let request = ResizeRequest {
                window: window.clone(),
                wanted: at(400, 300, 300, 200),
                edge_at: (700.0, 500.0),
                edges: ResizeEdge::Right,
            };
            state.begin_resize(&window);
            tiled_frame(
                &mut state,
                &request,
                pane,
                at(400, 300, 300, 200),
                Duration::ZERO,
            );

            // Sixteen milliseconds later — deep inside the interval the drag
            // just opened — something that is not the drag places this pane.
            // No `arm_resize_gesture`, because a reload does not run one: this
            // is the whole of what a `move_pane` from a key dispatch is.
            let was = state
                .pane_outer_of(pane)
                .expect("a mapped pane has a rectangle");
            state.move_pane(
                pane,
                at(400, 300, 260, 180),
                was,
                AnimationSpec::default(),
                Duration::from_millis(16),
                Standing::Tile,
            );
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert_eq!(
                client.configures.last().map(|&(_, w, h)| (w, h)),
                Some((260, 180)),
                "a reload's one configure must not be swallowed by a drag's \
                 interval: there is no second frame to resend it, and the pane \
                 has already taken the rectangle. Got {:?}",
                client.configures
            );
            assert!(
                state.holding_resize(pane),
                "and the bridge is still the bridge -- the reload was sent \
                 through the hold, not around it, so `asked` still names what \
                 the client last heard"
            );
        }

        /// **A pending change must not wait out the drag's interval.**
        ///
        /// `size_window` is a pending size *and* a `send_pending_configure`, so
        /// a throttled frame skips the flush as well. A maximise, a fullscreen
        /// or a decoration mode agreed by somebody else is then blocked behind
        /// an interval for exactly the windows a drag is touching — which is
        /// the one place case (1) short-circuited before the check that exists
        /// to catch it.
        #[test]
        fn a_pending_change_is_flushed_even_while_the_drag_is_throttled() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (window, _toplevel, _surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            state.map_stacked(window.clone(), (400, 300), false);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&window)
                .expect("a client in the space has a pane");
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            client.configures.clear();

            let request = ResizeRequest {
                window: window.clone(),
                wanted: at(400, 300, 300, 200),
                edge_at: (700.0, 500.0),
                edges: ResizeEdge::Right,
            };
            state.begin_resize(&window);
            tiled_frame(
                &mut state,
                &request,
                pane,
                at(400, 300, 300, 200),
                Duration::ZERO,
            );
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            let before = client.configures.len();

            // Somebody else agrees a state change and is waiting on the
            // configure that carries it.
            window
                .toplevel()
                .expect("the fixture's window is an xdg toplevel")
                .with_pending_state(|state| {
                    state.states.set(
                        smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Maximized,
                    );
                });
            // And a throttled frame of the drag arrives before the interval is
            // up. This is the frame that used to swallow it.
            tiled_frame(
                &mut state,
                &request,
                pane,
                at(400, 300, 310, 200),
                Duration::from_millis(16),
            );
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert!(
                client.configures.len() > before,
                "the throttle governs how often a drag asks for a size, not \
                 whether a window blocked on somebody else's change ever hears \
                 about it. Got {:?}",
                client.configures
            );
        }

        /// **A press that pauses let the previous gesture's deadline expire.**
        ///
        /// `begin_resize` reconciles the floating hold and clears
        /// `resize_ended`, and `arm_resize_gesture` rearms the bridge — but
        /// that runs on the first *motion*, and `settle_resize_bridge` runs on
        /// every frame. So pressing on a border, holding still for a quarter of
        /// a second and then dragging let the previous drag's `PATIENCE` run
        /// out with the button already down: every pane that drag had moved was
        /// adopted off its tile, which is precisely the snap `Hold::rearm`
        /// exists to prevent, arriving between the press and the first pixel of
        /// motion.
        #[test]
        fn a_press_stops_the_previous_drags_deadline_before_it_can_expire() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (window, _toplevel, _surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            state.map_stacked(window.clone(), (400, 300), false);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&window)
                .expect("a client in the space has a pane");
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );

            let request = ResizeRequest {
                window: window.clone(),
                wanted: at(400, 300, 300, 200),
                edge_at: (700.0, 500.0),
                edges: ResizeEdge::Right,
            };
            state.begin_resize(&window);
            let outer = at(400, 300, 300, 200);
            tiled_frame(&mut state, &request, pane, outer, Duration::ZERO);
            // The first gesture ends. Its holds are now waiting out `PATIENCE`
            // for an answer the fixture's client never gives.
            let ended = Duration::from_millis(100);
            state.release_bridge(&window, ended);

            // The second gesture presses, and the hand stays still. Every frame
            // of that pause runs `settle_resize_bridge`.
            state.begin_resize(&window);
            paused_frame(&mut state, ended + crate::resizing::PATIENCE);
            paused_frame(
                &mut state,
                ended + crate::resizing::PATIENCE + Duration::from_millis(50),
            );

            assert!(
                state.holding_resize(pane),
                "the new gesture owns this pane and will keep placing it, so \
                 the old gesture's deadline must stop at the press rather than \
                 at the first motion"
            );
            assert_eq!(
                state
                    .panes
                    .get(pane)
                    .expect("the pane is still here")
                    .slot(),
                outer,
                "and the pane is still on the tile the last drag left it on \
                 rather than snapped back to its client's own size"
            );
        }

        /// **A claimed/unclaimed flip cost two unthrottled configures and reset
        /// the interval.**
        ///
        /// `settle_resize` forks per frame, so a handler whose answer changes
        /// between frames hands one pane back and forth between the bridge and
        /// the floating hold. Each direction built a fresh `Hold` with a fresh
        /// `told`, and a fresh hold is told immediately by design — so an
        /// alternating handler restored the sixty configures a second
        /// `TELL_EVERY` exists to remove. Carrying the hold across the boundary
        /// is what stops that: it is the same client in the same gesture.
        ///
        /// The fixture has no scripts, so `trigger_resize` returns false and
        /// `settle_resize`'s own unclaimed branch is what `hold_resize` is
        /// reached through here; `tiled_frame` is the claimed one.
        #[test]
        fn a_layout_changing_its_mind_does_not_buy_a_configure_each_way() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (window, _toplevel, _surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            state.map_stacked(window.clone(), (400, 300), false);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&window)
                .expect("a client in the space has a pane");
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            client.configures.clear();

            let asking = |width: i32| ResizeRequest {
                window: window.clone(),
                wanted: at(400, 300, width, 200),
                edge_at: (f64::from(400 + width), 500.0),
                edges: ResizeEdge::Right,
            };
            state.begin_resize(&window);

            // Claimed: the pane joins the bridge and is told at once.
            tiled_frame(
                &mut state,
                &asking(300),
                pane,
                at(400, 300, 300, 200),
                Duration::ZERO,
            );
            // Unclaimed, sixteen milliseconds later. Every frame asks for a
            // different width, because smithay drops a configure that repeats
            // the last size it sent and a test that dragged to the same place
            // twice would be green against a compositor with no throttle at all.
            state.hold_resize(&asking(310), Duration::from_millis(16));
            // Claimed again.
            tiled_frame(
                &mut state,
                &asking(320),
                pane,
                at(400, 300, 320, 200),
                Duration::from_millis(32),
            );
            // And unclaimed again.
            state.hold_resize(&asking(330), Duration::from_millis(48));

            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert_eq!(
                client.configures.len(),
                1,
                "four frames inside one interval are one configure however many \
                 times the layout changed its mind: the pane's throttle belongs \
                 to the gesture, not to whichever authority happens to be \
                 placing it. Got {:?}",
                client.configures
            );
            assert!(
                state.holding_resize(pane),
                "and exactly one authority is holding it at the end"
            );
        }

        /// **A pane's moved edge was derived from where its *client* is, not
        /// from where the pane was.**
        ///
        /// `moved_edges` asks which of this pane's edges a placement moved, and
        /// a pane's previous rectangle is its slot. `real_geometry` is the
        /// client's — the space's position at the size the client last
        /// committed — which during a drag is frames behind the slot, so the
        /// client's latency leaked into the answer: an edge that did not move
        /// looks moved because the client has not caught up to where it already
        /// is. `Hold::pins` then hangs a held picture against the wrong side of
        /// the window, and `anchored` gives a refusal's pixels back on an edge
        /// the user never touched.
        ///
        /// The arrangement below is the ordinary one: a client that has
        /// answered nothing, a pane a seam has already widened once, and a
        /// second placement that pulls the pane's *left* edge. Measured from
        /// the slot that is a left pull and nothing else. Measured from the
        /// client's 64x64 buffer both horizontal edges look moved — which names
        /// neither — and the vertical bottom looks moved as well.
        #[test]
        fn a_panes_moved_edge_is_measured_from_its_own_slot_not_from_its_client() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (window, _toplevel, _surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            state.map_stacked(window.clone(), (400, 300), false);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&window)
                .expect("a client in the space has a pane");
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert_eq!(
                window.geometry().size,
                Size::from((64, 64)),
                "the client has answered nothing, which is what makes its \
                 rectangle the wrong thing to measure against"
            );

            let request = ResizeRequest {
                window: window.clone(),
                wanted: at(400, 300, 300, 200),
                edge_at: (700.0, 500.0),
                edges: ResizeEdge::Left,
            };
            state.begin_resize(&window);
            tiled_frame(
                &mut state,
                &request,
                pane,
                at(400, 300, 300, 200),
                Duration::ZERO,
            );

            // The layout stops placing this pane for a frame and then places it
            // again, which is what makes the edges be derived a second time.
            // Driven directly for the reason `tiled_frame` gives: a Lua handler
            // that changed its mind between frames would be testing mlua.
            state.drop_bridged(pane);
            // A left pull from the slot: the left edge moves in by 40 and the
            // right edge — at 700 — does not move at all.
            tiled_frame(
                &mut state,
                &request,
                pane,
                at(440, 300, 260, 200),
                Duration::from_millis(16),
            );

            assert_eq!(
                state.resize_pins(pane),
                Some((true, false)),
                "the left edge is the one that moved and the right one is \
                 standing still, so a held picture has to stay against the \
                 right. Measured from the client's committed 64x64 instead, \
                 both horizontal edges look moved and the pane is told it has \
                 no stationary edge at all"
            );
        }

        /// **A window the space does not have still needs the throttle.**
        ///
        /// Arming required `real_geometry` to answer, and it answers `None` for
        /// a window that is not in the space. The rectangle still counted as
        /// changed — `None != Some(client)` — so the client was told on every
        /// frame and no hold was ever armed to say when to stop: the one path
        /// left running at the pre-#123 configure rate, for the length of every
        /// gesture that touched such a window.
        ///
        /// A pane's own slot is the previous rectangle the edges want anyway,
        /// so nothing needs the space's answer to arm; only case (2)'s "is the
        /// client already here" does, and `None` there means "no idea", which
        /// is a reason to send rather than a reason not to hold.
        #[test]
        fn a_pane_whose_window_left_the_space_is_still_bridged() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (window, _toplevel, _surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            state.map_stacked(window.clone(), (400, 300), false);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&window)
                .expect("a client in the space has a pane");
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );

            state.space.unmap_elem(&window);
            assert_eq!(
                state.real_geometry(&window),
                None,
                "the space no longer has it, which is the whole precondition"
            );

            let request = ResizeRequest {
                window: window.clone(),
                wanted: at(400, 300, 300, 200),
                edge_at: (700.0, 500.0),
                edges: ResizeEdge::Right,
            };
            state.begin_resize(&window);
            tiled_frame(
                &mut state,
                &request,
                pane,
                at(400, 300, 300, 200),
                Duration::ZERO,
            );
            assert!(
                state.holding_resize(pane),
                "a pane a live gesture moved is bridged whether or not the \
                 space can say where its client was: without a hold nothing \
                 throttles it and nothing ever ends it"
            );
        }

        /// **The keyboard belongs to the lock screen, whatever the session
        /// behind it does.**
        ///
        /// One test per route by which the keyboard used to reach an
        /// application while the session was locked, each driven the way it
        /// happens: a real application client and a real lock client on two
        /// sockets, each with a `wl_keyboard` of its own, keys pressed through
        /// the same filter the backends feed. Every route ends on the same
        /// four measures, taken from both ends of the wire: the server's
        /// keyboard is on a lock surface, the application's client was never
        /// told it has the keyboard, it was never made the clipboard's client,
        /// and a key typed reaches the lock client and not the application.
        /// Then each one unlocks and checks that the keyboard comes back,
        /// because a fix that sealed the keyboard for good would pass
        /// everything before that.
        ///
        /// Each of those route tests fails against `69835bc`, the stage they
        /// were fixed on. That was checked by putting that commit's code back
        /// under them and watching each one fail -- a security test that
        /// passes either way is worse than none.
        ///
        /// Then the tests of **who holds the lock**, from `the lock's holder`
        /// on: a second lock client, an application asking for a lock of its
        /// own, an unlock from a lock that was never granted, the lock client
        /// crashing, a mode active at the lock, a lock surface dying, the key
        /// held at unlock, a capture while locked and a menu closing. Each of
        /// those drives real clients too, with a third one where the attack or
        /// the second lock screen needs one, and each fails against
        /// `2b6550e`; the doc of each says where.
        ///
        /// Then the two where #127's close meets the rule, which neither branch
        /// could write alone: `a_close_in_flight_across_a_lock_never_takes_the_keyboard`
        /// and `a_close_with_a_menu_open_takes_the_keyboard_off_the_menu`. Each
        /// fails against the merge of the two with its own fix taken out; the
        /// doc of each says where.
        ///
        /// The X11 test at the end is the exception to all of them: see its doc.
        mod lock_focus {
            use super::*;
            use smithay::backend::input::KeyState;
            use smithay::input::keyboard::Keycode;

            /// One client's end of the wire.
            struct Side {
                conn: Connection,
                queue: wayland_client::EventQueue<Client>,
                qh: QueueHandle<Client>,
                client: Client,
            }

            impl Side {
                /// A client with a keyboard and a data device, bound the way
                /// any application binds them.
                fn connect(display: &mut Display<Solium>, state: &mut Solium) -> Self {
                    let (conn, queue, client) = connect(display, state);
                    let qh = queue.handle();
                    let mut side = Self {
                        conn,
                        queue,
                        qh,
                        client,
                    };
                    let seat = side.client.seat.clone().expect("wl_seat bound");
                    side.client
                        .data_devices
                        .clone()
                        .expect("wl_data_device_manager bound")
                        .get_data_device(&seat, &side.qh, ());
                    // The keyboard is not asked for here: the seat's handler
                    // takes the one keyboard a client has, and a second would
                    // be sent every key again. That costs a second round trip,
                    // one to hear the seat's capabilities and one for the
                    // server to see the `get_keyboard` sent in answer --
                    // `a_mode_active_at_the_lock_does_not_garble_the_password`
                    // counts the keys that arrive.
                    side.pump(display, state);
                    side.pump(display, state);
                    assert!(
                        side.client.keyboard.is_some(),
                        "the seat offered a keyboard and the client took it"
                    );
                    side
                }

                fn pump(&mut self, display: &mut Display<Solium>, state: &mut Solium) {
                    pump(
                        display,
                        state,
                        &self.conn,
                        &self.qh,
                        &mut self.queue,
                        &mut self.client,
                    );
                }

                /// A round trip for a client the server may disconnect on the
                /// way, which `pump` would read as the fixture failing.
                /// Returns the protocol error it was disconnected with, if any.
                fn pump_or_error(
                    &mut self,
                    display: &mut Display<Solium>,
                    state: &mut Solium,
                ) -> Option<wayland_client::backend::protocol::ProtocolError> {
                    self.conn.display().sync(&self.qh, ());
                    // A client already disconnected cannot flush; what it was
                    // disconnected with is still read below.
                    let _flushed = self.conn.flush();
                    display
                        .dispatch_clients(state)
                        .expect("dispatching the round trip");
                    display
                        .flush_clients()
                        .expect("flushing the server's events");
                    match self.queue.blocking_dispatch(&mut self.client) {
                        Ok(_) => None,
                        Err(_) => self.conn.protocol_error(),
                    }
                }

                /// Ask for a lock, and if `cover`, a lock surface for every
                /// monitor, the way a lock screen does. Nothing is asserted:
                /// this is also how an application behind the lock asks.
                fn ask_lock(
                    &mut self,
                    display: &mut Display<Solium>,
                    state: &mut Solium,
                    cover: bool,
                ) -> (ext_session_lock_v1::ExtSessionLockV1, Vec<LockSurfaceProxy>)
                {
                    let locks = self
                        .client
                        .locks
                        .clone()
                        .expect("ext_session_lock_manager_v1 bound");
                    let compositor = self.client.compositor.clone().expect("wl_compositor bound");
                    let lock = locks.lock(&self.qh, ());
                    let mut surfaces = Vec::new();
                    if cover {
                        for output in self.client.outputs.clone() {
                            let surface = compositor.create_surface(&self.qh, ());
                            let role = lock.get_lock_surface(&surface, &output, &self.qh, ());
                            surfaces.push(LockSurfaceProxy { surface, role });
                        }
                    }
                    self.pump(display, state);
                    (lock, surfaces)
                }
            }

            /// One lock surface, from the client's side.
            struct LockSurfaceProxy {
                surface: wl_surface::WlSurface,
                role: ext_session_lock_surface_v1::ExtSessionLockSurfaceV1,
            }

            impl LockSurfaceProxy {
                /// Destroy it the way `swaylock` does when its monitor goes.
                fn destroy(&self) {
                    self.role.destroy();
                    self.surface.destroy();
                }
            }

            impl Side {
                /// Open a window, and run the frame that follows it.
                ///
                /// `sync_panes` is what the backends call once a frame, and it
                /// is where a window arriving or going is noticed -- and where
                /// `settle_focus` is called from. A test that skipped it would
                /// skip one of the routes.
                fn open(
                    &mut self,
                    display: &mut Display<Solium>,
                    state: &mut Solium,
                ) -> (
                    Window,
                    xdg_toplevel::XdgToplevel,
                    wl_surface::WlSurface,
                    xdg_surface::XdgSurface,
                ) {
                    let opened = open_xdg(display, state, &self.conn, &self.client, &self.qh);
                    state.sync_panes();
                    self.pump(display, state);
                    opened
                }

                /// Open a menu on `parent` that asks for a grab, as a context
                /// menu does: the grab before the first commit, as the
                /// protocol requires, with the serial of the keyboard's last
                /// enter.
                fn menu(
                    &mut self,
                    display: &mut Display<Solium>,
                    state: &mut Solium,
                    parent: &xdg_surface::XdgSurface,
                ) -> xdg_popup::XdgPopup {
                    let compositor = self.client.compositor.clone().expect("wl_compositor bound");
                    let wm_base = self.client.wm_base.clone().expect("xdg_wm_base bound");
                    let seat = self.client.seat.clone().expect("wl_seat bound");

                    let surface = compositor.create_surface(&self.qh, ());
                    let xdg = wm_base.get_xdg_surface(&surface, &self.qh, ());
                    let positioner = wm_base.create_positioner(&self.qh, ());
                    positioner.set_size(32, 32);
                    positioner.set_anchor_rect(0, 0, 1, 1);
                    let popup = xdg.get_popup(Some(parent), &positioner, &self.qh, ());
                    popup.grab(&seat, self.client.serial);
                    surface.commit();
                    self.pump(display, state);
                    popup
                }
            }

            /// One monitor, an application client and a lock client that has
            /// not locked anything yet.
            struct Session {
                display: Display<Solium>,
                state: Solium,
                app: Side,
                locker: Side,
                /// The lock client's surfaces from its last `lock`, one per
                /// monitor.
                lock_surfaces: Vec<LockSurfaceProxy>,
            }

            impl Session {
                fn new() -> Self {
                    Self::with_monitors(1)
                }

                /// `count` monitors side by side, each 1920x1080.
                fn with_monitors(count: i32) -> Self {
                    let mut display =
                        Display::<Solium>::new().expect("creating a test wayland display");
                    let mut state = Solium::new(display.handle());
                    // See the #99 test: a Qt scene in a process holding a
                    // libwayland connection of its own aborts the test binary.
                    state
                        .decorations
                        .set_style(&mut state.panes, Some("none".to_string()));

                    for index in 0..count {
                        let output = Output::new(
                            format!("lock-focus-test-{index}"),
                            PhysicalProperties {
                                size: (0, 0).into(),
                                subpixel: Subpixel::Unknown,
                                make: "solium".to_string(),
                                model: "lock-focus".to_string(),
                            },
                        );
                        output.change_current_state(
                            Some(Mode {
                                size: (1920, 1080).into(),
                                refresh: 60_000,
                            }),
                            None,
                            Some(Scale::Fractional(1.0)),
                            None,
                        );
                        // A global, which the other fixtures' monitors are
                        // not: a lock surface is asked for per `wl_output`, so
                        // the lock client has to be able to name one. Before
                        // either client connects, so it is in both registries.
                        let _global = output.create_global::<Solium>(&display.handle());
                        state.space.map_output(&output, (1920 * index, 0));
                    }

                    let app = Side::connect(&mut display, &mut state);
                    let locker = Side::connect(&mut display, &mut state);
                    Self {
                        display,
                        state,
                        app,
                        locker,
                        lock_surfaces: Vec::new(),
                    }
                }

                /// Whether the server's keyboard is on one of the lock
                /// client's surfaces.
                ///
                /// Asked of the lock itself and not of anything in `focus.rs`:
                /// a test that checked the gate by asking the gate would pass
                /// whatever the gate did.
                fn keyboard_on_lock(&self) -> bool {
                    let Some(focus) = self
                        .state
                        .seat
                        .get_keyboard()
                        .and_then(|keyboard| keyboard.current_focus())
                    else {
                        return false;
                    };
                    self.state
                        .lock
                        .as_ref()
                        .is_some_and(|lock| lock.surfaces().any(|each| each.wl_surface() == &focus))
                }

                /// Lock the session the way a lock screen does: lock, then a
                /// surface for the monitor.
                fn lock(&mut self) -> ext_session_lock_v1::ExtSessionLockV1 {
                    let (lock, surfaces) =
                        self.locker
                            .ask_lock(&mut self.display, &mut self.state, true);
                    self.lock_surfaces = surfaces;
                    self.app.pump(&mut self.display, &mut self.state);

                    assert!(
                        self.state.lock.is_some(),
                        "the lock client asked and the session did not lock, so \
                         nothing after this is about a locked session"
                    );
                    assert!(
                        self.locker
                            .client
                            .locked
                            .contains(&wayland_client::Proxy::id(&lock)),
                        "the lock client was not told `locked`"
                    );
                    assert!(
                        self.keyboard_on_lock(),
                        "the lock surface mapped and the keyboard is not on it: \
                         `focus_lock` is the one hand-off the gate must let through"
                    );
                    assert!(
                        self.locker.client.keyboard_on.is_some(),
                        "and the lock client was told so"
                    );
                    assert!(
                        self.app.client.keyboard_on.is_none(),
                        "locking did not take the keyboard away from the application"
                    );
                    lock
                }

                /// Type one key, and say who heard it: the application, and
                /// the lock client.
                fn type_key(&mut self) -> (bool, bool) {
                    self.app.client.keys.clear();
                    self.locker.client.keys.clear();
                    // `a`, which is evdev 30 and 38 to xkb.
                    crate::input::key(&mut self.state, Keycode::new(38), KeyState::Pressed, 1);
                    crate::input::key(&mut self.state, Keycode::new(38), KeyState::Released, 2);
                    self.app.pump(&mut self.display, &mut self.state);
                    self.locker.pump(&mut self.display, &mut self.state);
                    (
                        !self.app.client.keys.is_empty(),
                        !self.locker.client.keys.is_empty(),
                    )
                }

                /// What every route ends on: the keyboard is still the lock
                /// screen's by every measure there is.
                fn assert_sealed(&mut self, route: &str, selections_before: usize) {
                    assert!(
                        self.keyboard_on_lock(),
                        "{route}: the keyboard left the lock screen"
                    );
                    assert!(
                        self.app.client.keyboard_on.is_none(),
                        "{route}: the application's client was told it has the keyboard"
                    );
                    assert!(
                        self.locker.client.keyboard_on.is_some(),
                        "{route}: the lock client was not told it has the keyboard"
                    );
                    assert_eq!(
                        self.app.client.selections, selections_before,
                        "{route}: the application behind the lock was made the \
                         clipboard's client"
                    );
                    let (app, locker) = self.type_key();
                    assert!(
                        !app,
                        "{route}: a key typed at the lock screen reached the \
                         application behind it, and that key is a password"
                    );
                    assert!(
                        locker,
                        "{route}: the key reached nobody at all, so the line \
                         above proves nothing"
                    );
                }

                /// Unlock, and the keyboard comes back to a window without the
                /// mouse being touched.
                fn assert_unlocks(&mut self, lock: ext_session_lock_v1::ExtSessionLockV1) {
                    lock.unlock_and_destroy();
                    self.locker.pump(&mut self.display, &mut self.state);
                    self.app.pump(&mut self.display, &mut self.state);

                    assert!(
                        self.state.lock.is_none(),
                        "the lock client unlocked and the session did not"
                    );
                    let focus = self
                        .state
                        .seat
                        .get_keyboard()
                        .and_then(|keyboard| keyboard.current_focus());
                    assert!(
                        focus
                            .as_ref()
                            .is_some_and(|surface| self.state.window_for(surface).is_some()),
                        "the session unlocked with the keyboard on no window. \
                         `unlock` clears `lock` before `settle_focus` so that the \
                         gate lets it through; a session that ignores typing until \
                         you move the mouse reads as one that did not unlock"
                    );
                    assert!(
                        self.app.client.keyboard_on.is_some(),
                        "and the application was not told it has it back"
                    );
                    let (app, _) = self.type_key();
                    assert!(app, "unlocked, and typing does not reach the application");
                }
            }

            /// **A window opening while locked.** `new_toplevel` set the
            /// keyboard itself, straight onto the new window: an application
            /// that opened a window behind the lock took the password.
            #[test]
            fn a_window_that_opens_while_locked_does_not_take_the_keyboard() {
                let mut session = Session::new();
                session.app.open(&mut session.display, &mut session.state);
                assert!(
                    session.app.client.keyboard_on.is_some(),
                    "the application has the keyboard before the lock, or \
                     taking it away proves nothing"
                );
                let lock = session.lock();
                let selections = session.app.client.selections;

                session.app.open(&mut session.display, &mut session.state);

                session.assert_sealed("a window opened while locked", selections);
                session.assert_unlocks(lock);
            }

            /// **A window closing while locked.** `sync_panes` calls
            /// `settle_focus`, which returns early only if a *window* has
            /// focus. A lock surface is not a window, so it went on to its
            /// topmost arm and gave the keyboard to the top application.
            #[test]
            fn a_window_that_closes_while_locked_does_not_take_the_keyboard() {
                let mut session = Session::new();
                session.app.open(&mut session.display, &mut session.state);
                let (_window, toplevel, surface, xdg) =
                    session.app.open(&mut session.display, &mut session.state);
                let lock = session.lock();
                let selections = session.app.client.selections;

                toplevel.destroy();
                xdg.destroy();
                surface.destroy();
                session.app.pump(&mut session.display, &mut session.state);
                // The frame after it: the space lets go of the dead window and
                // `sync_panes` sees the set of windows change.
                session.state.space.refresh();
                assert!(
                    session.state.sync_panes(),
                    "closing a window did not change the set of windows, so \
                     `settle_focus` never ran and this proves nothing"
                );

                session.assert_sealed("a window closed while locked", selections);
                session.assert_unlocks(lock);
            }

            /// **A menu opened while locked.** `grab` set the keyboard itself,
            /// onto the menu, and installed a keyboard grab that ignores every
            /// later attempt to move it.
            #[test]
            fn a_menu_that_grabs_while_locked_does_not_take_the_keyboard() {
                let mut session = Session::new();
                let (_window, _toplevel, _surface, parent) =
                    session.app.open(&mut session.display, &mut session.state);
                let lock = session.lock();
                let selections = session.app.client.selections;

                let _menu = session
                    .app
                    .menu(&mut session.display, &mut session.state, &parent);

                assert!(
                    !session
                        .state
                        .seat
                        .get_keyboard()
                        .expect("the seat has a keyboard")
                        .is_grabbed(),
                    "a menu behind the lock was given a keyboard grab"
                );
                assert!(
                    session.app.client.popups_done > 0,
                    "the grab was refused and the menu was not told, so it waits \
                     on a grab that is not coming and is still open after unlock"
                );
                session.assert_sealed("a menu grabbed while locked", selections);
                session.assert_unlocks(lock);
            }

            /// **A menu already open when the session locks.** Its keyboard
            /// grab ignored `lock`'s attempt to take the keyboard away and
            /// `focus_lock`'s attempt to give it to the lock screen, then
            /// re-aimed the keyboard at the menu on every key.
            #[test]
            fn a_menu_open_when_the_session_locks_gives_the_keyboard_up() {
                let mut session = Session::new();
                let (_window, _toplevel, _surface, parent) =
                    session.app.open(&mut session.display, &mut session.state);
                let _menu = session
                    .app
                    .menu(&mut session.display, &mut session.state, &parent);
                let keyboard = session
                    .state
                    .seat
                    .get_keyboard()
                    .expect("the seat has a keyboard");
                assert!(
                    keyboard.is_grabbed(),
                    "the menu asked for a grab and did not get one, so locking \
                     over it proves nothing"
                );

                let lock = session.lock();
                assert!(
                    !keyboard.is_grabbed(),
                    "the menu's keyboard grab outlived the lock"
                );
                assert!(
                    session.app.client.popups_done > 0,
                    "the menu open at the lock was not dismissed, so its chain \
                     outlives the lock"
                );
                let selections = session.app.client.selections;
                session.assert_sealed("a menu was open when the session locked", selections);
                session.assert_unlocks(lock);

                // The chain really was ended, not just ungrabbed. Left recorded
                // on the seat, this second menu would be told it is not the
                // topmost popup -- a protocol error, which kills the client and
                // fails the round trip inside `menu`.
                let _again = session
                    .app
                    .menu(&mut session.display, &mut session.state, &parent);
                assert!(
                    keyboard.is_grabbed(),
                    "after unlock, a menu could not grab: the chain from before \
                     the lock is still in the way"
                );
            }

            /// **`focus_window`, called while locked**, as a script's
            /// `sol.focus` calls it, and as a layout's event handler, a
            /// focus-follows-view workspace and xdg-activation all do -- none
            /// of which stop running when the session locks.
            #[test]
            fn focusing_a_window_while_locked_does_not_move_the_keyboard() {
                let mut session = Session::new();
                let (window, ..) = session.app.open(&mut session.display, &mut session.state);
                let lock = session.lock();
                let selections = session.app.client.selections;

                session
                    .state
                    .focus_window(&window, SERIAL_COUNTER.next_serial());
                session.app.pump(&mut session.display, &mut session.state);

                session.assert_sealed("focus_window while locked", selections);
                session.assert_unlocks(lock);
            }

            /// **The last line: a key cannot reach a surface the gate would
            /// have refused, even when something has got the keyboard there.**
            ///
            /// Done here the only way it can be, by calling the one method
            /// the gate exists to own -- which stands for the route nobody has
            /// found yet.
            #[test]
            fn a_key_does_not_reach_an_application_that_got_past_the_gate() {
                let mut session = Session::new();
                let (window, ..) = session.app.open(&mut session.display, &mut session.state);
                let lock = session.lock();

                let surface = window
                    .wl_surface()
                    .expect("the window has a surface")
                    .into_owned();
                let keyboard = session
                    .state
                    .seat
                    .get_keyboard()
                    .expect("the seat has a keyboard");
                #[expect(
                    clippy::disallowed_methods,
                    reason = "going round the gate on purpose: this is the test of what holds when something does"
                )]
                keyboard.set_focus(
                    &mut session.state,
                    Some(surface),
                    SERIAL_COUNTER.next_serial(),
                );
                session.app.pump(&mut session.display, &mut session.state);
                assert!(
                    session.app.client.keyboard_on.is_some(),
                    "the keyboard was not moved onto the application, so this \
                     proves nothing"
                );

                let (app, _) = session.type_key();
                assert!(
                    !app,
                    "a key typed while locked reached an application that held \
                     the keyboard: nothing at delivery checks the rule"
                );
                session.assert_unlocks(lock);
            }

            /// **The lock's holder: a second lock is refused, and the first
            /// keeps working.** `swayidle`'s `before-sleep` starting a second
            /// `swaylock` while the first is up. The second lock was granted
            /// and *replaced* the first: the real lock screen's surfaces were
            /// dropped and the keyboard taken off them, and the newcomer --
            /// which, told `finished`, would have exited -- was left the only
            /// lock. The user was locked out of their own session.
            ///
            /// Against `2b6550e`, fails at "can no longer be typed into".
            #[test]
            fn a_second_lock_is_refused_and_the_first_keeps_working() {
                let mut session = Session::new();
                session.app.open(&mut session.display, &mut session.state);
                let lock = session.lock();
                let selections = session.app.client.selections;

                let mut second = Side::connect(&mut session.display, &mut session.state);
                let (again, _) = second.ask_lock(&mut session.display, &mut session.state, false);
                let again_id = wayland_client::Proxy::id(&again);
                let refused = second.client.finished.contains(&again_id)
                    && !second.client.locked.contains(&again_id);
                // What `swaylock` does with `finished`: destroy the lock, and
                // exit.
                again.destroy();
                let _gone = second.pump_or_error(&mut session.display, &mut session.state);
                drop(second);
                session
                    .locker
                    .pump(&mut session.display, &mut session.state);
                session.app.pump(&mut session.display, &mut session.state);

                let (app, locker) = session.type_key();
                assert!(
                    locker,
                    "after a second lock was asked for, the real lock screen can \
                     no longer be typed into: the user is locked out of their own \
                     session"
                );
                assert!(!app, "and the key reached the application");
                assert!(
                    refused,
                    "a second lock was granted while the lock client holding the \
                     session was still there"
                );
                session.assert_sealed("a second lock was asked for", selections);
                session.assert_unlocks(lock);
            }

            /// **An application cannot put up a lock screen of its own.** It
            /// asked for a lock -- which replaced the real one -- and a lock
            /// surface on its own `wl_output`, which smithay's duplicate-output
            /// check lets through because it compares one client's resources.
            /// The gate saw a surface in `Lock::surfaces` and gave it the
            /// keyboard: an application drawing a lock screen, and the
            /// password typed into it.
            ///
            /// Against `2b6550e`, fails at "was given the keyboard".
            #[test]
            fn an_application_cannot_put_up_a_lock_screen_of_its_own() {
                let mut session = Session::new();
                session.app.open(&mut session.display, &mut session.state);
                let lock = session.lock();
                let selections = session.app.client.selections;

                let (theirs, fake) =
                    session
                        .app
                        .ask_lock(&mut session.display, &mut session.state, true);
                session
                    .locker
                    .pump(&mut session.display, &mut session.state);
                assert!(
                    !fake.is_empty(),
                    "the application asked for no lock surface, so this proves nothing"
                );
                assert!(
                    session.app.client.keyboard_on.is_none(),
                    "an application's own lock surface was given the keyboard: a \
                     lock screen that is not the lock screen, and the password goes \
                     into it"
                );
                assert!(
                    session
                        .app
                        .client
                        .finished
                        .contains(&wayland_client::Proxy::id(&theirs)),
                    "the application's lock was not refused"
                );
                session.assert_sealed("an application put up a lock screen of its own", selections);
                session.assert_unlocks(lock);
            }

            /// **Nor in the moment before the real one maps.** The session is
            /// locked the instant the lock client asks, before it has drawn
            /// anything -- and a monitor plugged in while locked is another
            /// monitor the real lock screen has not covered yet.
            ///
            /// Against `2b6550e`, fails at "before the lock screen mapped".
            #[test]
            fn an_application_cannot_put_up_a_lock_screen_before_the_real_one_maps() {
                let mut session = Session::new();
                session.app.open(&mut session.display, &mut session.state);
                let (lock, _) =
                    session
                        .locker
                        .ask_lock(&mut session.display, &mut session.state, false);
                assert!(
                    session.state.lock.is_some(),
                    "the lock client asked and the session did not lock, so this \
                     proves nothing"
                );

                let (_theirs, _fake) =
                    session
                        .app
                        .ask_lock(&mut session.display, &mut session.state, true);
                session
                    .locker
                    .pump(&mut session.display, &mut session.state);
                assert!(
                    session.app.client.keyboard_on.is_none(),
                    "in the moment before the lock screen mapped, an application's \
                     own lock surface was given the keyboard"
                );

                // The real lock screen arrives.
                let compositor = session
                    .locker
                    .client
                    .compositor
                    .clone()
                    .expect("wl_compositor bound");
                let output = session
                    .locker
                    .client
                    .output
                    .clone()
                    .expect("wl_output bound");
                let surface = compositor.create_surface(&session.locker.qh, ());
                let role = lock.get_lock_surface(&surface, &output, &session.locker.qh, ());
                session.lock_surfaces = vec![LockSurfaceProxy { surface, role }];
                session
                    .locker
                    .pump(&mut session.display, &mut session.state);
                session.app.pump(&mut session.display, &mut session.state);

                let selections = session.app.client.selections;
                session.assert_sealed(
                    "an application's lock surface came before the real one",
                    selections,
                );
                session.assert_unlocks(lock);
            }

            /// **An unlock from a lock that was never granted unlocks
            /// nothing.** `unlock` is not told which lock asked, and smithay
            /// 0.7 calls it for `unlock_and_destroy` on any lock object: it
            /// posts `invalid_unlock` on one that was never told `locked`, and
            /// then, with no `return` after the error, unlocks anyway. So any
            /// client could lock and then unlock -- two requests.
            ///
            /// Against `2b6550e`, fails at "in two requests". Against a fix
            /// that only refused the second lock, it fails at the same line,
            /// through smithay's missing `return`.
            #[test]
            fn an_unlock_from_a_lock_that_was_never_granted_unlocks_nothing() {
                let mut session = Session::new();
                session.app.open(&mut session.display, &mut session.state);
                let lock = session.lock();
                let selections = session.app.client.selections;

                let mut intruder = Side::connect(&mut session.display, &mut session.state);
                let (theirs, _) =
                    intruder.ask_lock(&mut session.display, &mut session.state, false);
                theirs.unlock_and_destroy();
                let error = intruder.pump_or_error(&mut session.display, &mut session.state);

                assert!(
                    session.state.lock.is_some(),
                    "a client that does not hold the lock unlocked the session in \
                     two requests"
                );
                let error = error.expect(
                    "the intruder was not disconnected: an unlock on a lock that \
                     was never told `locked` is `invalid_unlock`",
                );
                // The code only. `unlock_and_destroy` is a destructor, so by
                // the time the error arrives the client has already forgotten
                // the object it is about and cannot say which interface it was.
                assert_eq!(
                    error.code,
                    ext_session_lock_v1::Error::InvalidUnlock as u32,
                    "disconnected, but not for `invalid_unlock`: {error:?}"
                );
                session
                    .locker
                    .pump(&mut session.display, &mut session.state);
                session.app.pump(&mut session.display, &mut session.state);
                session.assert_sealed("an unlock from a lock that was never granted", selections);
                // And the holder's own unlock is untouched by all of it.
                session.assert_unlocks(lock);
            }

            /// **The lock client crashing leaves the session locked**, and a
            /// new lock client can take over. The protocol says a lock client
            /// dying must not unlock the session. What holds the lock then is
            /// nobody, and that must not turn into anybody: a lock refused
            /// while the holder lived stays refused after it has died.
            ///
            /// Against `2b6550e`, fails at "was granted": the rival's lock
            /// simply replaced the real one, and its unlock then worked
            /// whether the real lock client crashed or not.
            #[test]
            fn the_lock_client_crashing_leaves_the_session_locked() {
                let mut session = Session::new();
                session.app.open(&mut session.display, &mut session.state);
                let _crashing = session.lock();
                let selections = session.app.client.selections;

                let mut rival = Side::connect(&mut session.display, &mut session.state);
                let (refused, _) = rival.ask_lock(&mut session.display, &mut session.state, false);
                assert!(
                    rival
                        .client
                        .finished
                        .contains(&wayland_client::Proxy::id(&refused)),
                    "a second lock was granted while the lock client holding the \
                     session was still there"
                );

                // The lock client crashes: its connection closes with the
                // session still locked. Its replacement is connected first,
                // the way one is started from another terminal.
                let recovery = Side::connect(&mut session.display, &mut session.state);
                let crashed = std::mem::replace(&mut session.locker, recovery);
                session.lock_surfaces.clear();
                drop(crashed);
                session.app.pump(&mut session.display, &mut session.state);
                session
                    .locker
                    .pump(&mut session.display, &mut session.state);

                assert!(
                    session.state.lock.is_some(),
                    "the lock client crashed and the session unlocked"
                );
                assert!(
                    session.app.client.keyboard_on.is_none(),
                    "the lock client crashed and the application was given the keyboard"
                );
                let (app, _) = session.type_key();
                assert!(
                    !app,
                    "the lock client crashed and a key reached the application"
                );

                refused.unlock_and_destroy();
                let _gone = rival.pump_or_error(&mut session.display, &mut session.state);
                assert!(
                    session.state.lock.is_some(),
                    "with the lock client dead, a lock refused while it lived \
                     unlocked the session"
                );

                // Recovered: a new lock client takes over, and unlocks.
                let lock = session.lock();
                session.assert_sealed("a new lock client took over from a crashed one", selections);
                session.assert_unlocks(lock);
            }

            /// **A mode active when the session locks does not garble the
            /// password.** Presses reached the lock screen, but the filter
            /// intercepted every release while a mode was active, so the lock
            /// client auto-repeated each key it was never told came up -- and
            /// with bindings off while locked, the mode could not be left.
            ///
            /// Against `2b6550e`, fails at "never told it came up".
            #[test]
            fn a_mode_active_at_the_lock_does_not_garble_the_password() {
                let mut session = Session::new();
                session.app.open(&mut session.display, &mut session.state);
                // A mode owns input, the way a script's overview or resize
                // mode takes it.
                session.state.script_grab = true;
                let lock = session.lock();

                session.locker.client.key_events.clear();
                session.app.client.keys.clear();
                // `a`, which is evdev 30 and 38 to xkb.
                crate::input::key(&mut session.state, Keycode::new(38), KeyState::Pressed, 1);
                crate::input::key(&mut session.state, Keycode::new(38), KeyState::Released, 2);
                session
                    .locker
                    .pump(&mut session.display, &mut session.state);
                session.app.pump(&mut session.display, &mut session.state);
                assert_eq!(
                    session.locker.client.key_events,
                    vec![(30, true), (30, false)],
                    "the lock screen was told a key went down and never told it \
                     came up, so it repeats it: a mode active at the lock garbles \
                     every key of the password"
                );
                assert!(
                    session.app.client.keys.is_empty(),
                    "and the key reached the application"
                );

                // The mode ends; the rest is the ordinary unlock.
                session.state.script_grab = false;
                session.assert_unlocks(lock);
            }

            /// **A lock surface that dies hands the keyboard to one that did
            /// not.** A monitor unplugged or a laptop undocked while locked:
            /// the lock client destroys that monitor's surface when its output
            /// goes. If the keyboard was on it, nothing moved it -- `focus_lock`
            /// runs only when a surface maps, and `settle_focus` refused every
            /// window without offering the lock screen instead -- so keys went
            /// nowhere until another surface arrived.
            ///
            /// Against `2b6550e`, fails at "went nowhere".
            #[test]
            fn a_lock_surface_that_dies_hands_the_keyboard_to_one_that_did_not() {
                let mut session = Session::with_monitors(2);
                session.app.open(&mut session.display, &mut session.state);
                let lock = session.lock();
                assert_eq!(
                    session.lock_surfaces.len(),
                    2,
                    "not a lock surface on each of two monitors, so this proves nothing"
                );

                let focused = session
                    .locker
                    .client
                    .keyboard_on
                    .clone()
                    .expect("the lock screen has the keyboard");
                let (gone, left): (Vec<_>, Vec<_>) = std::mem::take(&mut session.lock_surfaces)
                    .into_iter()
                    .partition(|each| wayland_client::Proxy::id(&each.surface) == focused);
                let gone = gone
                    .into_iter()
                    .next()
                    .expect("the keyboard is on one of the lock screen's surfaces");
                gone.destroy();
                session
                    .locker
                    .pump(&mut session.display, &mut session.state);
                session.app.pump(&mut session.display, &mut session.state);

                let survivor = left
                    .first()
                    .map(|each| wayland_client::Proxy::id(&each.surface));
                assert_eq!(
                    session.locker.client.keyboard_on, survivor,
                    "the lock surface the keyboard was on went away and the \
                     keyboard went nowhere: typing at the lock screen does nothing \
                     until another surface maps"
                );
                session.lock_surfaces = left;
                let selections = session.app.client.selections;
                session.assert_sealed("the focused lock surface died", selections);
                session.assert_unlocks(lock);
            }

            /// **The key that unlocks is not handed to the window.** The Enter
            /// that submits the password is still held when the lock client
            /// unlocks, and the keyboard went straight back to the window,
            /// whose `wl_keyboard.enter` then said Enter was held -- a key it
            /// never saw pressed, typed at the lock screen.
            ///
            /// Against `2b6550e`, fails at "told a key is held".
            #[test]
            fn the_key_that_unlocks_is_not_handed_to_the_window() {
                let mut session = Session::new();
                session.app.open(&mut session.display, &mut session.state);
                let lock = session.lock();

                // Enter, which is evdev 28 and 36 to xkb, down at the lock
                // screen when the lock client unlocks.
                crate::input::key(&mut session.state, Keycode::new(36), KeyState::Pressed, 1);
                lock.unlock_and_destroy();
                session
                    .locker
                    .pump(&mut session.display, &mut session.state);
                session.app.pump(&mut session.display, &mut session.state);
                assert!(
                    session.state.lock.is_none(),
                    "the lock client unlocked and the session did not"
                );
                assert!(
                    session.app.client.keyboard_on.is_none()
                        || !session.app.client.enter_keys.contains(&28),
                    "the window was told a key is held that it never saw pressed: \
                     the Enter typed at the lock screen"
                );

                session.app.client.keys.clear();
                crate::input::key(&mut session.state, Keycode::new(36), KeyState::Released, 2);
                session.app.pump(&mut session.display, &mut session.state);
                assert!(
                    session.app.client.keyboard_on.is_some(),
                    "the key came up and the keyboard did not go back to the window"
                );
                assert!(
                    !session.app.client.enter_keys.contains(&28),
                    "the window was told a key is held that it never saw pressed"
                );
                assert!(
                    !session.app.client.keys.contains(&28),
                    "the window was sent the release of a key it never saw pressed"
                );
                let (app, _) = session.type_key();
                assert!(app, "unlocked, and typing does not reach the application");
            }

            /// **Nothing captures the screen while the session is locked.**
            /// What is on it then is the lock screen, and a recording of that
            /// is the password's length and the rhythm it was typed at. Every
            /// client but the lock client is behind the lock and any of them
            /// can ask. Both ways in are driven: a capture asked for while
            /// locked, and one asked for before the lock and copied after. A
            /// capture already *queued* when the session locks is refused in
            /// `screencopy::settle`, which needs a renderer and is not reached
            /// from here.
            ///
            /// Against `2b6550e`, fails at "was offered a buffer".
            #[test]
            fn nothing_captures_the_screen_while_the_session_is_locked() {
                let mut session = Session::new();
                session.app.open(&mut session.display, &mut session.state);
                let manager = session
                    .app
                    .client
                    .screencopy
                    .clone()
                    .expect("zwlr_screencopy_manager_v1 bound");
                let output = session.app.client.output.clone().expect("wl_output bound");
                let before = manager.capture_output(0, &output, &session.app.qh, ());
                session.app.pump(&mut session.display, &mut session.state);
                assert_eq!(
                    session.app.client.captures_offered, 1,
                    "unlocked, a capture was not offered a buffer, so refusing one \
                     proves nothing"
                );
                let lock = session.lock();

                let _during = manager.capture_output(0, &output, &session.app.qh, ());
                session.app.pump(&mut session.display, &mut session.state);
                assert_eq!(
                    session.app.client.captures_offered, 1,
                    "a capture asked for while locked was offered a buffer: an \
                     application can record the lock screen"
                );
                assert_eq!(
                    session.app.client.captures_failed, 1,
                    "and it was never told it failed"
                );

                let shm = session.app.client.shm.clone().expect("wl_shm bound");
                let fd = anon_file(64 * 64 * 4);
                let pool = shm.create_pool(fd.as_fd(), 64 * 64 * 4, &session.app.qh, ());
                let buffer = pool.create_buffer(
                    0,
                    64,
                    64,
                    64 * 4,
                    wl_shm::Format::Xrgb8888,
                    &session.app.qh,
                    (),
                );
                before.copy(&buffer);
                session.app.pump(&mut session.display, &mut session.state);
                assert_eq!(
                    session.app.client.captures_failed, 2,
                    "a capture asked for before the lock and copied after it was \
                     not refused"
                );

                session.assert_unlocks(lock);
            }

            /// **A menu that closes lets go of its grab.** `popup_grab` was set
            /// when a menu grabbed and cleared only by `release_grabs` at the
            /// next lock, so a menu closed an hour before was still recorded
            /// as the chain holding the seat when the session next locked.
            /// Harmless as things stand -- see `popup_destroyed` -- and tested
            /// because the field's one reader is the lock.
            ///
            /// Against `2b6550e`, fails at "still recorded".
            #[test]
            fn a_menu_that_closes_lets_go_of_its_grab() {
                let mut session = Session::new();
                let (_window, _toplevel, _surface, parent) =
                    session.app.open(&mut session.display, &mut session.state);
                let menu = session
                    .app
                    .menu(&mut session.display, &mut session.state, &parent);
                assert!(
                    session.state.popup_grab.is_some(),
                    "the menu's grab was never recorded, so this proves nothing"
                );

                menu.destroy();
                session.app.pump(&mut session.display, &mut session.state);
                assert!(
                    session.state.popup_grab.is_none(),
                    "a menu closed and its grab is still recorded as the one \
                     holding the seat"
                );
            }

            /// **A close in flight across a lock and its unlock never hands a
            /// window the keyboard.** #127 gives a closing window back from
            /// three places -- a refusal's deadline, a dialog answering the
            /// close, and `settle_closing`'s retry -- and every one of them
            /// ends in `settle_focus`, the call that once handed a locked
            /// session's keyboard to the top application.
            ///
            /// Locked, each step is measured the way every route in this module
            /// is (`assert_sealed`): the close's deadline, a dialog's answer, a
            /// refusal's deadline. Then the same two closes are in flight when
            /// the lock client unlocks with Enter still down, which is
            /// `refocus_on_release`'s tenth of a second. A dialog and a
            /// refusal both land in it, and neither may give a window the
            /// keyboard until the key is up, or that window is told in
            /// `wl_keyboard.enter` that Enter is held. The retry goes through
            /// the same `give_back` and needs a busy transform slot to reach;
            /// it is not driven here.
            ///
            /// Against this merge without `settle_focus`'s
            /// `refocus_on_release` return, fails at "a dialog's answer was
            /// handed the keyboard while the unlock key was held".
            #[test]
            fn a_close_in_flight_across_a_lock_never_takes_the_keyboard() {
                let mut session = Session::new();
                let (refused, _refused_top, ..) =
                    session.app.open(&mut session.display, &mut session.state);
                let (answered, answered_top, ..) =
                    session.app.open(&mut session.display, &mut session.state);
                let refused = session
                    .state
                    .panes
                    .id_of(&refused)
                    .expect("an open window has a pane");
                let answered = session
                    .state
                    .panes
                    .id_of(&answered)
                    .expect("an open window has a pane");
                let leaving = |state: &Solium, pane: crate::pane::PaneId| {
                    state.panes.get(pane).is_some_and(Pane::leaving)
                };

                // super+q on both, and the lock screen straight after.
                session.state.close_pane(refused);
                session.state.close_pane(answered);
                let lock = session.lock();
                let selections = session.app.client.selections;

                // The close's deadline: the requests go out.
                session
                    .state
                    .clock
                    .advance(present::CLOSING + Duration::from_millis(10));
                let asked = session.state.clock.now();
                session.state.settle_closing(asked);
                session.app.pump(&mut session.display, &mut session.state);
                assert!(
                    [refused, answered].into_iter().all(|pane| {
                        session
                            .state
                            .panes
                            .get(pane)
                            .is_some_and(|pane| pane.asked_at().is_some())
                    }),
                    "both closes were asked, or nothing below is a close in flight"
                );
                session.assert_sealed("a close's deadline passed while locked", selections);

                // One of them answered with a dialog, which gives it back.
                let (_dialog, dialog_top, ..) =
                    session.app.open(&mut session.display, &mut session.state);
                dialog_top.set_parent(Some(&answered_top));
                session.app.pump(&mut session.display, &mut session.state);
                assert!(
                    !leaving(&session.state, answered),
                    "the dialog did not give its parent back, so this proves nothing"
                );
                session.assert_sealed("a dialog answered a close while locked", selections);

                // The other said nothing, and its deadline gives it back.
                session.state.clock.advance(Duration::from_millis(1100));
                let now = session.state.clock.now();
                session.state.settle_refused(now);
                session.app.pump(&mut session.display, &mut session.state);
                assert!(
                    !leaving(&session.state, refused),
                    "the refusal's deadline did not give the window back, so this \
                     proves nothing"
                );
                session.assert_sealed("a refused close was given back while locked", selections);

                // Both closed again and asked, a second dialog open to answer
                // one of them -- opened while locked, so it takes no keyboard --
                // and Enter (evdev 28, 36 to xkb) down at the lock screen when
                // it unlocks.
                let (_second, second_top, ..) =
                    session.app.open(&mut session.display, &mut session.state);
                session.state.close_pane(refused);
                session.state.close_pane(answered);
                session
                    .state
                    .clock
                    .advance(present::CLOSING + Duration::from_millis(10));
                let asked = session.state.clock.now();
                session.state.settle_closing(asked);
                crate::input::key(&mut session.state, Keycode::new(36), KeyState::Pressed, 3);
                lock.unlock_and_destroy();
                session
                    .locker
                    .pump(&mut session.display, &mut session.state);
                session.app.pump(&mut session.display, &mut session.state);
                assert!(
                    session.state.lock.is_none() && session.state.refocus_on_release,
                    "the premise: unlocked, with the keyboard waiting for the key \
                     that unlocked to come up"
                );
                assert!(
                    session.app.client.keyboard_on.is_none(),
                    "the premise: nothing has the keyboard while the key is down"
                );

                second_top.set_parent(Some(&answered_top));
                session.app.pump(&mut session.display, &mut session.state);
                assert!(
                    !leaving(&session.state, answered),
                    "the second dialog did not give its parent back, so this \
                     proves nothing"
                );
                assert!(
                    session.app.client.keyboard_on.is_none(),
                    "a dialog's answer was handed the keyboard while the unlock key \
                     was held, and its window told Enter is down: {:?}",
                    session.app.client.enter_keys
                );

                session.state.clock.advance(Duration::from_millis(1100));
                let now = session.state.clock.now();
                session.state.settle_refused(now);
                session.app.pump(&mut session.display, &mut session.state);
                assert!(
                    !leaving(&session.state, refused),
                    "the refusal's deadline did not give the window back, so this \
                     proves nothing"
                );
                assert!(
                    session.app.client.keyboard_on.is_none(),
                    "a refusal's deadline was handed the keyboard while the unlock \
                     key was held, and its window told Enter is down: {:?}",
                    session.app.client.enter_keys
                );

                // The key comes up, and only now does a window take the keyboard.
                session.app.client.keys.clear();
                crate::input::key(&mut session.state, Keycode::new(36), KeyState::Released, 4);
                session.app.pump(&mut session.display, &mut session.state);
                assert!(
                    session.app.client.keyboard_on.is_some(),
                    "the key came up and the keyboard did not go back to a window"
                );
                assert!(
                    !session.app.client.enter_keys.contains(&28),
                    "the window was told a key is held that it never saw pressed"
                );
                assert!(
                    !session.app.client.keys.contains(&28),
                    "the window was sent the release of a key it never saw pressed"
                );
                let (app, _) = session.type_key();
                assert!(app, "unlocked, and typing does not reach the application");
            }

            /// **A window closed with a menu open takes the menu's grab with
            /// it.** #127 takes the keyboard off a closing window once its
            /// animation lands, by giving the keyboard to nothing. With a menu
            /// open that did nothing: the menu's keyboard grab ignores every
            /// `set_focus` until its chain ends, and `focused_window` answers
            /// for the menu with its window, so `settle_focus` saw a focused
            /// window and left it. The keyboard stayed on the closed window's
            /// menu for the whole grace period, and every key typed after the
            /// close went to it.
            /// `release_grabs_of` is the fix, and `release_grabs` -- which
            /// does the same for everyone when the session locks -- the model.
            ///
            /// Against this merge without `release_grabs_of` in
            /// `hand_off_keyboard`, fails at "the menu's keyboard grab
            /// outlived its window's close".
            #[test]
            fn a_close_with_a_menu_open_takes_the_keyboard_off_the_menu() {
                let mut session = Session::new();
                let (kept, ..) = session.app.open(&mut session.display, &mut session.state);
                let (closing, _toplevel, _surface, parent) =
                    session.app.open(&mut session.display, &mut session.state);
                let _menu = session
                    .app
                    .menu(&mut session.display, &mut session.state, &parent);
                let keyboard = session
                    .state
                    .seat
                    .get_keyboard()
                    .expect("the seat has a keyboard");
                assert!(
                    keyboard.is_grabbed() && session.state.is_focused(&closing),
                    "the premise: a menu open on the focused window, holding a grab"
                );

                let pane = session
                    .state
                    .panes
                    .id_of(&closing)
                    .expect("an open window has a pane");
                session.state.close_pane(pane);
                session
                    .state
                    .clock
                    .advance(present::CLOSING + Duration::from_millis(10));
                let asked = session.state.clock.now();
                session.state.settle_closing(asked);
                session.app.pump(&mut session.display, &mut session.state);

                assert!(
                    !keyboard.is_grabbed(),
                    "the menu's keyboard grab outlived its window's close"
                );
                assert!(
                    session.app.client.popups_done > 0,
                    "the closed window's menu was left open"
                );
                assert_eq!(
                    session.state.focused_window().as_ref(),
                    Some(&kept),
                    "the keyboard did not move to the window that is still drawn"
                );
                let (app, _) = session.type_key();
                assert!(app, "the key reached nobody at all");
                assert_eq!(
                    session
                        .app
                        .client
                        .typed
                        .last()
                        .and_then(|(surface, _)| *surface),
                    Some(surface_id(&kept)),
                    "a key typed after the close did not reach the window that is \
                     drawn"
                );
            }

            /// **The rule the X11 clipboard bridge asks: no X11 client may
            /// read the selection while the session is locked.** There is no
            /// focus to gate on the X11 side of the bridge, and every X11
            /// client is behind the lock.
            ///
            /// What this does *not* cover, said plainly because its first
            /// version claimed it did: the bridge itself. It asks
            /// `Solium::x11_may_read_selection` under a real lock taken by a
            /// real lock client, and that is all. The one line that connects
            /// the rule to X11 -- `XwmHandler::allow_selection_access` in
            /// `xwayland.rs`, which returns it -- is not called here, because
            /// it cannot be: it takes an `XwmId`, which smithay constructs only
            /// inside a running X11 window manager, and there is no X server in
            /// a unit test. Replacing that line with `true` would leave this
            /// green. There is no X11 client here either.
            #[test]
            fn the_x11_selection_rule_refuses_while_the_session_is_locked() {
                let mut session = Session::new();
                session.app.open(&mut session.display, &mut session.state);
                assert!(
                    session.state.x11_may_read_selection(),
                    "unlocked, the two halves of the session paste into each other"
                );
                let lock = session.lock();
                assert!(
                    !session.state.x11_may_read_selection(),
                    "locked, an X11 client could read whatever the lock screen copied"
                );
                session.assert_unlocks(lock);
                assert!(
                    session.state.x11_may_read_selection(),
                    "and unlocked again, it can"
                );
            }
        }

        /// A window opened, mapped at a known place, and known to the panes.
        ///
        /// The three closing tests below all start here, and all three need a
        /// *real* client rather than a loading pane: `settle_closing` removes a
        /// pane with no client outright — "the one case where closing is
        /// entirely ours to decide" — so a fixture without one never reaches
        /// `send_close`, never stamps `asked_at`, and so cannot see either of
        /// the two faults that live after the request goes out.
        fn opened_at(
            display: &mut Display<Solium>,
            state: &mut Solium,
            conn: &Connection,
            client: &Client,
            qh: &QueueHandle<Client>,
            place: (i32, i32),
        ) -> (Window, crate::pane::PaneId) {
            let (window, _toplevel) = open_window(display, state, conn, client, qh);
            state.map_stacked(window.clone(), place, false);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&window)
                .expect("a client in the space has a pane");
            (window, pane)
        }

        /// How this pane is drawn at `now`, through the same call the renderer
        /// makes.
        fn drawn_now(state: &Solium, pane: crate::pane::PaneId, now: Duration) -> Frame {
            let outer = state
                .pane_outer_of(pane)
                .expect("a mapped pane has a rectangle");
            state.drawn_id_at(pane, outer, now)
        }

        /// **Issue #127, fault 1: a layout sweep during a close put the dying
        /// window back at full opacity, and it then vanished with no
        /// animation.**
        ///
        /// `Pane::closing_at` had three production readers and `move_pane` was
        /// not one of them, so the unconditional `present::from` at the end of
        /// every placement overwrote the closing transform with a *released*
        /// one aimed at `Frame::real` — full size, full opacity. Any sweep
        /// inside the 190 ms `CLOSING` window did it, and a sweep inside that
        /// window is ordinary: another window opening, a layer surface's first
        /// configure, a GTK4 `set_parent`. That is the "sometimes close
        /// animations does not even play" the issue was filed for.
        ///
        /// **Three assertions, and they are three different claims.** The
        /// opacity is the fault itself. The position is the *decision* — a
        /// closing pane animates out from where it was, and does not slide to
        /// the slot the layout has just given it; see `move_pane`, which argues
        /// it. And `placed` is the half that must keep working: only the
        /// transform is suppressed, the layout's bookkeeping is untouched, and
        /// #124 reads `placed`.
        #[test]
        fn a_layout_sweep_does_not_cancel_a_close_that_is_already_playing() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (_window, pane) =
                opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );

            let was = state
                .pane_outer_of(pane)
                .expect("a mapped pane has a rectangle");
            // Sampled before the call, which samples it again for itself: the
            // gap is microseconds and every instant below is offset by enough
            // milliseconds that it cannot matter.
            let pressed = state.clock.now();
            state.close_pane(pane);
            assert!(
                drawn_now(&state, pane, pressed + Duration::from_millis(95)).opacity < 0.9,
                "the close animation is under way; if it were not, nothing \
                 below is testing anything"
            );

            // The sweep. A window opening on the other half of the screen is
            // the commonest way to get one, and `tiling.apply` re-places every
            // leaf on every visible monitor — including this one, which is
            // still in the tree because its client has not gone yet.
            let elsewhere = at(1000, 300, 64, 64);
            state.move_pane(
                pane,
                elsewhere,
                was,
                AnimationSpec::default(),
                pressed + Duration::from_millis(95),
                Standing::Tile,
            );

            let landed = drawn_now(&state, pane, pressed + Duration::from_millis(400));
            assert!(
                landed.opacity.abs() < f32::EPSILON,
                "a pane the layout moved mid-close must still finish its \
                 close: it was drawn at opacity {} instead",
                landed.opacity
            );
            // 400 + 64 * (1 - 0.86) / 2: `Frame::scaled` is about the centre,
            // so the shrunk rectangle sits inside the one the window was
            // closed at. The claim is which of the two places it is near, and
            // 600 logical pixels separate them.
            assert!(
                landed.rect.loc.x < 500.0,
                "a closing pane animates out from where it was, not from the \
                 slot it will never occupy: it was drawn at x={}",
                landed.rect.loc.x
            );

            let placed = state
                .panes
                .get(pane)
                .and_then(Pane::placed)
                .expect("the sweep placed this pane");
            assert_eq!(
                placed, elsewhere,
                "only the transform is suppressed. The layout's own answer is \
                 still written, because the layout is still right about where \
                 this pane lives and #124 reads it"
            );

            // **And the close still completes.** The assertions above are all
            // about the frame at T+400, which a suppression that also swallowed
            // the request would satisfy perfectly: the window would sit at
            // opacity 0 for ever and no client would ever be told. Driving
            // `settle_closing` past the deadline is what separates "still
            // animating out" from "stuck invisible", and it is the claim the
            // sentence "a swept close still sends its one request and stays
            // gone" was making with nothing behind it.
            state.settle_closing(pressed + Duration::from_millis(400));
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert_eq!(
                client.closes.len(),
                1,
                "a close the layout swept still sends its one request"
            );
            assert!(
                drawn_now(&state, pane, pressed + Duration::from_millis(500))
                    .opacity
                    .abs()
                    < f32::EPSILON,
                "and the window stays gone afterwards rather than being handed \
                 back by the sweep"
            );
        }

        /// **Issue #127, fault 2: a second close restarted an invisible
        /// animation and sent a second `send_close`.**
        ///
        /// `close_pane`'s guard was `closing_at().is_some()`, and
        /// `settle_closing` clears `closing_at` before it stamps `asked_at` —
        /// so from the moment the request goes out until `settle_refused` gives
        /// up on it, a pane already on its way out answered "not closing" to
        /// the one question that was asked about it. A second `super+q` in that
        /// window restarted `present::close` from the held opacity-0 frame
        /// (invisible to invisible, so nothing to see) and asked the client
        /// again.
        ///
        /// Asking twice is not harmless. A client showing "save your work?" is
        /// a client that received the first request and is acting on it; a
        /// second one is a second dialog.
        #[test]
        fn a_second_close_inside_the_grace_neither_restarts_nor_asks_again() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (_window, pane) =
                opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert!(
                client.closes.is_empty(),
                "opening a window asks nothing to close"
            );

            let pressed = state.clock.now();
            state.close_pane(pane);
            let asked = pressed + present::CLOSING + Duration::from_millis(10);
            state.settle_closing(asked);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert_eq!(client.closes.len(), 1, "the press asked, once");
            assert!(
                drawn_now(&state, pane, asked).opacity.abs() < f32::EPSILON,
                "and the window is held invisible while the client decides, \
                 which is what makes a second press invisible too"
            );

            // The second `super+q`, well inside the grace period. There is
            // nothing on screen for the user to have aimed it at, which is
            // exactly why it happens: the window went and the client has not.
            state.close_pane(pane);
            assert!(
                state.panes.get(pane).and_then(Pane::closing_at).is_none(),
                "a pane already on its way out must not have its animation \
                 restarted -- and restarting it from an invisible frame to an \
                 invisible frame is 190ms of nothing"
            );

            // Far enough past a second `CLOSING` that a restarted timer would
            // have come due and sent its request.
            state.settle_closing(asked + present::CLOSING + Duration::from_millis(10));
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert_eq!(
                client.closes.len(),
                1,
                "and the client is asked once per window, not once per press"
            );
        }

        /// **Issue #127, fault 3: a slow client faded out, faded back in, then
        /// popped.**
        ///
        /// `settle_refused` measures its grace from the instant the request
        /// went out and brings the window back when it expires. At 400 ms that
        /// is not a deadline on *refusal* — it is a deadline on slowness, and
        /// Electron's `before-quit`, the JVM's window listeners and Firefox's
        /// session flush all run past it. What the user saw was the window fade
        /// away, come back, and then vanish with no animation at all when the
        /// client finally did close.
        ///
        /// **Written against a client latency, not against the constant.** 600
        /// ms is the claim: a client that takes that long to honour a close is
        /// never shown again. A test that restated `GRACE` would pass at any
        /// value including the one that caused the bug — this fails for every
        /// grace period shorter than 600 ms, whatever it is called.
        ///
        /// **And the other direction, in the same test**, because the recovery
        /// is right and deleting it would otherwise turn this green: a window
        /// that never answers is still brought back, and soon enough that it
        /// reads as an answer to the press. Together the two halves pin the
        /// grace period into a range rather than onto a number.
        #[test]
        fn a_client_that_takes_six_hundred_milliseconds_to_close_is_never_shown_again() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (_window, pane) =
                opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );

            let pressed = state.clock.now();
            state.close_pane(pane);
            let asked = pressed + present::CLOSING + Duration::from_millis(10);
            state.settle_closing(asked);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert_eq!(client.closes.len(), 1, "the request went out");

            /// What a heavy client costs between receiving `xdg_toplevel.close`
            /// and destroying its toplevel. The fixture's client never destroys
            /// anything, so this is simply how long the test waits before
            /// declaring the window safely gone.
            const SLOW: Duration = Duration::from_millis(600);
            /// A frame at 60Hz. The fault is a frame the user can see, so the
            /// assertion is made at the rate frames are drawn.
            const FRAME: Duration = Duration::from_millis(16);

            /// By when a window that never answers must be back on screen and
            /// fully opaque. Not arithmetic: it is the span in which a window
            /// reappearing still reads as caused by the press rather than as
            /// the session doing something by itself, and it has to cover the
            /// recovery's own fade as well as the wait before it.
            const LOST: Duration = Duration::from_millis(1500);

            let mut waited = Duration::ZERO;
            // The loop that ships: `Solium::settle` calls this once a frame.
            // Sampling the frame at the same instant is what makes the
            // assertion about what is on screen rather than about a timer.
            while waited <= LOST {
                let now = asked + waited;
                state.settle_refused(now);
                let opacity = drawn_now(&state, pane, now).opacity;
                if waited <= SLOW {
                    assert!(
                        opacity.abs() < f32::EPSILON,
                        "{}ms after the request the window was drawn at opacity \
                         {opacity}; a client this slow is closing, not \
                         refusing, and bringing it back means fade out, fade \
                         in, pop",
                        waited.as_millis()
                    );
                }
                waited += FRAME;
            }

            // And the other direction. The recovery is right and must survive:
            // a window animated away that then refuses to close would
            // otherwise be invisible and alive, holding its place in the
            // layout, with nothing to bring it back.
            let back = drawn_now(&state, pane, asked + LOST).opacity;
            assert!(
                (back - 1.0).abs() < f32::EPSILON,
                "a window that never answers is still brought back, and within \
                 a second and a half of being asked: it was drawn at opacity \
                 {back}"
            );
        }

        /// **#127 review, finding 1: an invisible closing pane went on winning
        /// every hit test at the rectangle it used to occupy.**
        ///
        /// `window_under`, `surface_under`, `pane_chrome` and `decorated_under`
        /// all asked `drawn_at(..).rect.contains(location)` and nothing else.
        /// `present::close` ends at opacity 0 and is written with
        /// `release: false` on purpose, so the transform *holds* there — for
        /// `CLOSING` plus the whole grace period, which #127 took from about
        /// 590 ms to about 1190 ms. For all of it the dead window was the
        /// topmost thing at a rectangle the layout had already given to
        /// somebody else.
        ///
        /// **The trade #127 made without noticing.** It removed a cosmetic
        /// flicker — the window fading back in and popping — and what that
        /// flicker had been doing was *telling the user the window was still
        /// there*. Silencing it while leaving the hit test alone turns a
        /// visible glitch into an invisible one: the sibling has reflowed into
        /// the space and is what is on screen, a click there lands in the dead
        /// window, and every keystroke after it follows the focus that click
        /// set. For exactly the slow-but-honest clients the grace bump was
        /// written for.
        ///
        /// **Both halves of a press, because they are two walks and either
        /// alone would leave the other broken.** `window_under` is what
        /// click-to-focus raises and focuses — the keystroke half, since focus
        /// is what the typing follows — and `surface_under` is what the pointer
        /// event is actually delivered to. `chrome_under` is asserted beside
        /// them because an invisible titlebar is still a titlebar to a walk
        /// that only measures rectangles.
        ///
        /// The pane being closed is opened *second* so that it is above the
        /// survivor in the stack: a test where the right answer is also the
        /// topmost one is not testing the walk.
        #[test]
        fn a_press_where_a_closed_window_used_to_be_reaches_what_is_drawn_there() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (kept, survivor) =
                opened_at(&mut display, &mut state, &conn, &client, &qh, (1000, 300));
            let (doomed, closing) =
                opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );

            // Past both windows' opening animations, and *settled*, which is
            // what the render loop does once a frame. Two reasons, and the test
            // needs both. `present::open` starts at opacity 0 and eases up, so
            // a window in its first frame is genuinely not on screen yet and
            // these walks correctly decline it. And `open`'s target was
            // captured before `opened_at` moved the window, so until the
            // released transform is retired the pane is still drawn at the
            // rectangle it mapped at rather than the one it now lives at.
            state.clock.advance(Duration::from_millis(300));
            state.settle(state.clock.now());

            let vacated = state
                .pane_outer_of(closing)
                .expect("a mapped pane has a rectangle");
            let was = state
                .pane_outer_of(survivor)
                .expect("a mapped pane has a rectangle");
            // The middle of the window the user is about to close, which is
            // where the window that replaces it will be too.
            let point = Point::<f64, Logical>::from((
                f64::from(vacated.loc.x) + f64::from(vacated.size.w) / 2.0,
                f64::from(vacated.loc.y) + f64::from(vacated.size.h) / 2.0,
            ));

            // The premise. If the closing window did not own this point to
            // begin with, nothing below is about anything.
            assert_eq!(
                state.window_under(point).map(|(window, _)| window),
                Some(doomed.clone()),
                "before the close, the point belongs to the window that is \
                 about to be closed"
            );

            state.close_pane(closing);
            // Past `CLOSING`: the animation has landed, the request has gone
            // out, and the fixture's client never destroys anything -- which is
            // what a client still running its quit handlers looks like from
            // here.
            state
                .clock
                .advance(present::CLOSING + Duration::from_millis(10));
            let asked = state.clock.now();
            state.settle_closing(asked);
            assert_eq!(client.closes.len(), 0, "the client has not answered yet");

            // The layout reflows into the space, which is the ordinary
            // consequence of a close and the reason the stale rectangle matters
            // at all.
            state.move_pane(
                survivor,
                vacated,
                was,
                AnimationSpec::default(),
                asked,
                Standing::Tile,
            );
            // Past the survivor's own move, so what is drawn at the point is
            // the survivor itself rather than a frame of it in transit.
            state.clock.advance(Duration::from_millis(400));

            // The premise for the second half: the closing pane is invisible
            // and its rectangle still covers the point. Without this the walk
            // could be answering correctly for the wrong reason.
            let dead = drawn_now(&state, closing, state.clock.now());
            assert!(
                dead.opacity.abs() < f32::EPSILON && dead.rect.contains(point),
                "the closing pane is held at opacity 0 over the point -- \
                 opacity {}, rect {:?}. That is the situation under test",
                dead.opacity,
                dead.rect
            );

            assert_eq!(
                state.window_under(point).map(|(window, _)| window),
                Some(kept.clone()),
                "a click where a closed window used to be belongs to the \
                 window that is drawn there now. `window_under` is what \
                 click-to-focus focuses, so answering the dead window sends \
                 every keystroke after the click into a window that is not on \
                 screen"
            );

            let surface = state
                .surface_under(point)
                .map(|(surface, _)| surface)
                .expect("the survivor is drawn at this point and has a surface");
            assert_eq!(
                Some(&surface),
                kept.wl_surface().as_deref(),
                "and the pointer event is delivered to that window's surface, \
                 not to the dead one's"
            );

            // The compositor's own chrome, by the same rule: an invisible
            // titlebar has no buttons and an invisible edge cannot be dragged.
            // A `Halo` would be wrong here too, which is why `chrome_offered`
            // is gated and not only `covers`.
            assert!(
                state
                    .chrome_under(point)
                    .is_none_or(|under| under.pane != closing),
                "no chrome of the closed window is under the point either"
            );
        }

        /// **#127 review, finding 2: the guard covered the transform and left
        /// the client-facing half of a placement running.**
        ///
        /// `move_pane` suppressed `present::from` for a leaving pane and went on
        /// calling `offers_size`, `size_window` and `map_stacked`. The configure
        /// is the visible one: it asks a client that is tearing itself down to
        /// re-lay-out at a size nobody will ever see, and if the client answers,
        /// `real_geometry` moves under a `frame.rect` that `present::close` has
        /// pinned — which is exactly the pair `resizing::factor` divides, so the
        /// dying buffer is stretched to fill a rectangle it was never painted
        /// for, mid-fade.
        ///
        /// Counted at the client, for the reason `Client::configures` gives:
        /// from the server's own side a compositor that sends a configure looks
        /// identical to one that does not.
        #[test]
        fn a_layout_sweep_does_not_configure_a_window_that_is_closing() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (_window, pane) =
                opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );

            let was = state
                .pane_outer_of(pane)
                .expect("a mapped pane has a rectangle");
            let pressed = state.clock.now();
            state.close_pane(pane);

            let before = client.configures.len();
            // A different size, not just a different place: `offers_size`
            // deduplicates on the whole rectangle, so a sweep that only moved
            // the pane would send nothing even without the guard and the test
            // would pass against the bug.
            state.move_pane(
                pane,
                at(1000, 300, 250, 180),
                was,
                AnimationSpec::default(),
                pressed + Duration::from_millis(95),
                Standing::Tile,
            );
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );

            assert_eq!(
                client.configures.len(),
                before,
                "a client that has been asked to close is not asked to \
                 re-lay-out at a size that will never be drawn"
            );

            // And the half that must keep working, for the same reason the
            // sweep test asserts it: only what a client can observe is
            // suppressed, and #124 reads `placed`.
            let placed = state
                .panes
                .get(pane)
                .and_then(Pane::placed)
                .expect("the sweep placed this pane");
            assert_eq!(
                placed,
                at(1000, 300, 250, 180),
                "the layout's own answer is still written for a leaving pane"
            );
        }

        /// **#127 review, finding 3: `GRACE` inverts for the client that
        /// refuses on purpose.**
        ///
        /// The grace period was lengthened on the argument that too long only
        /// makes a genuinely refused window wait. That holds for the
        /// honest-but-slow client and reverses for this one: "save your changes
        /// before closing?" is a refusal delivered as a question, and under a
        /// flat deadline the parent was a hole for the whole second with the
        /// dialog floating over nothing to read. A second `super+q` could not
        /// clear it either — correctly, since `Pane::leaving` declines to start
        /// a second close on a pane already in one.
        ///
        /// A new window parented to the one being closed is an answer, and it
        /// is the safe kind to act on: being wrong gives a window back that was
        /// going to leave anyway. See `Solium::refused_with_a_dialog`.
        ///
        /// **Asserted well inside `GRACE`**, which is the whole claim. Sampling
        /// after it would pass against the plain deadline and test nothing.
        #[test]
        fn a_window_that_answers_a_close_with_a_dialog_comes_straight_back() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            // `opened_at`'s body, inlined for the one thing it discards: the
            // client-side toplevel proxy, which is the only end `set_parent`
            // can be sent from.
            let (parent, parent_toplevel) =
                open_window(&mut display, &mut state, &conn, &client, &qh);
            state.map_stacked(parent.clone(), (400, 300), false);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&parent)
                .expect("a client in the space has a pane");
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );

            state.close_pane(pane);
            state
                .clock
                .advance(present::CLOSING + Duration::from_millis(10));
            let asked = state.clock.now();
            state.settle_closing(asked);
            assert!(
                state
                    .panes
                    .get(pane)
                    .is_some_and(|pane| pane.asked_at().is_some()),
                "the request has gone out and the client has not answered"
            );
            assert!(
                drawn_now(&state, pane, asked).opacity.abs() < f32::EPSILON,
                "and the window is invisible, which is the hole the dialog \
                 would otherwise float over"
            );

            // The client's answer: not a destroy, a dialog. `set_parent` is
            // what says the dialog is about *this* window, and it is the
            // request `parent_changed` fires on.
            let (dialog, dialog_toplevel) =
                open_window(&mut display, &mut state, &conn, &client, &qh);
            dialog_toplevel.set_parent(Some(&parent_toplevel));
            conn.flush().expect("flushing set_parent");
            display
                .dispatch_clients(&mut state)
                .expect("dispatching set_parent");

            // The fixture is doing what it claims: the compositor read the
            // parent back, so this is a dialog about the window being closed
            // and not merely another window.
            assert_eq!(
                state.parent_of(&dialog),
                Parentage::Window(pane.get()),
                "the client called set_parent and the compositor did not read \
                 it back, so nothing below is about a dialog for this window"
            );

            let back = drawn_now(&state, pane, state.clock.now() + Duration::from_millis(200));
            assert!(
                (back.opacity - 1.0).abs() < f32::EPSILON,
                "a window whose client answered with a dialog is back on \
                 screen without waiting out the grace period: it was drawn at \
                 opacity {}",
                back.opacity
            );
            assert!(
                state.panes.get(pane).is_some_and(|pane| !pane.leaving()),
                "and it is no longer leaving, so a second super+q can close it"
            );
        }

        /// **#127 third review, finding 1: an X11 tooltip could cancel a
        /// close.**
        ///
        /// The rule is `refused_with_a_dialog`'s own: a window that places
        /// itself — a menu, a tooltip, a splash, a notification, an
        /// override-redirect window — appearing over a window that is closing
        /// says nothing about whether the close was refused. It stood as a
        /// comment at one of the three call sites, and the `TransientFor` hook
        /// added beside it did not repeat it. Every one of those windows is in
        /// `self.space` and receives `PROPERTY_CHANGE`, so any of them that set
        /// `WM_TRANSIENT_FOR` after mapping brought its parent back: `super+q`,
        /// the window fades out, returns, and never closes.
        ///
        /// **Asserted at rule level, and the limit is stated rather than
        /// implied.** An `X11Surface` cannot be built without a live XWayland
        /// and nothing in this suite has one, so the X11 event that carries the
        /// case cannot be produced here. What *can* be produced is the fact the
        /// gate turns on, which is not an X11 fact at all: `Pane::managed`,
        /// false for exactly those windows and set in one place —
        /// `take_unmanaged_pane`, which both of XWayland's self-placing branches
        /// call. So the child here is given the unmanaged pane a tooltip gets,
        /// and driven through the one caller this fixture can drive. That pins
        /// the gate; it does not pin the X11 plumbing above it.
        #[test]
        fn a_window_that_places_itself_does_not_cancel_a_close() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (parent, parent_toplevel) =
                open_window(&mut display, &mut state, &conn, &client, &qh);
            state.map_stacked(parent.clone(), (400, 300), false);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&parent)
                .expect("a client in the space has a pane");
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );

            state.close_pane(pane);
            state
                .clock
                .advance(present::CLOSING + Duration::from_millis(10));
            let asked = state.clock.now();
            state.settle_closing(asked);
            assert!(
                state
                    .panes
                    .get(pane)
                    .is_some_and(|pane| pane.asked_at().is_some()),
                "the request has gone out and the client has not answered, \
                 which is the state a tooltip must not end"
            );

            // The tooltip. Everything about it is ordinary except its pane,
            // which is the whole of what makes it one.
            let (tip, tip_toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
            let tip_pane = state
                .panes
                .id_of(&tip)
                .expect("a client in the space has a pane");
            if let Some(unmanaged) = state.panes.get_mut(tip_pane) {
                unmanaged.unmanage();
            }
            assert!(
                state
                    .panes
                    .get(tip_pane)
                    .is_some_and(|pane| !pane.managed()),
                "the premise: this child holds the unmanaged pane every menu, \
                 tooltip, splash and override-redirect window is given, and \
                 without it this test is about an ordinary dialog"
            );

            tip_toplevel.set_parent(Some(&parent_toplevel));
            conn.flush().expect("flushing set_parent");
            display
                .dispatch_clients(&mut state)
                .expect("dispatching set_parent");

            // The second premise: the compositor really did read the parent
            // back, so the gate is what declined and not a parent nobody found.
            assert_eq!(
                state.parent_of(&tip),
                Parentage::Window(pane.get()),
                "the tooltip named the closing window as its parent and the \
                 compositor did not read it back, so nothing below is about \
                 the gate"
            );

            assert!(
                state.panes.get(pane).is_some_and(Pane::leaving),
                "a window that places itself is not an answer to anything: the \
                 close must still be running"
            );
            let still = drawn_now(&state, pane, state.clock.now() + Duration::from_millis(200));
            assert!(
                still.opacity.abs() < f32::EPSILON,
                "and the closing window must still be held invisible rather \
                 than faded back up by a tooltip: it was drawn at opacity {}",
                still.opacity
            );
        }

        /// **#127 third review, finding 3: `refused_with_a_dialog` threw away
        /// `give_back`'s answer.**
        ///
        /// `give_back` reports whether `present::clear` took, because
        /// `with_slot` declines rather than panics when the transform slot is
        /// already borrowed. On a declined frame nothing is retired — and
        /// inside `CLOSING` there is no `asked_at`, so `settle_refused` is not
        /// looking at this pane and never will be. The close then ran to its
        /// deadline and `settle_closing` sent the request, closing the parent
        /// out from under the dialog that had just answered for it.
        ///
        /// **The observation is at the client, because that is the only end
        /// that can tell.** `send_close` is a call into smithay and a
        /// compositor that made it looks, from its own side, exactly like one
        /// that did not. See [`Client::closes`].
        ///
        /// **What the jam costs this test, said plainly.** `present::jam_slot`
        /// is one-way, so the give-back retries for ever here and there is no
        /// frame on which it succeeds; `present::frame` also falls back to real
        /// geometry while the slot is busy, so opacity says nothing either.
        /// What this asserts is the half that was actually lost — the request
        /// that must not go out, and the close staying owed rather than
        /// forgotten. The other half, a give-back on a free slot bringing the
        /// window back, is
        /// `a_window_that_answers_a_close_with_a_dialog_comes_straight_back`.
        #[test]
        fn a_dialog_whose_give_back_is_declined_does_not_lose_its_parent() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (parent, parent_toplevel) =
                open_window(&mut display, &mut state, &conn, &client, &qh);
            state.map_stacked(parent.clone(), (400, 300), false);
            state.sync_panes();
            let pane = state
                .panes
                .id_of(&parent)
                .expect("a client in the space has a pane");
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );

            state.close_pane(pane);
            // Inside `CLOSING`, which is where a file chooser arrives and is
            // the state `settle_refused` cannot see: the request has not gone
            // out, so there is no `asked_at` for it to be due on.
            state.clock.advance(Duration::from_millis(50));
            assert!(
                state
                    .panes
                    .get(pane)
                    .is_some_and(|pane| pane.closing_at().is_some() && pane.asked_at().is_none()),
                "the premise: the animation is playing and the request has not \
                 gone out, which is the only window in which this fault exists"
            );

            // The busy frame. Nothing un-jams this, which is why it is taken
            // after the close has started and before the dialog arrives.
            if let Some(busy) = state.panes.get(pane) {
                present::jam_slot(busy);
            }

            let (dialog, dialog_toplevel) =
                open_window(&mut display, &mut state, &conn, &client, &qh);
            dialog_toplevel.set_parent(Some(&parent_toplevel));
            conn.flush().expect("flushing set_parent");
            display
                .dispatch_clients(&mut state)
                .expect("dispatching set_parent");
            assert_eq!(
                state.parent_of(&dialog),
                Parentage::Window(pane.get()),
                "the client called set_parent and the compositor did not read \
                 it back, so nothing below is about a dialog for this window"
            );

            // Past the deadline the request would go out on.
            state
                .clock
                .advance(present::CLOSING + Duration::from_millis(10));
            let due = state.clock.now();
            let still_going = state.settle_closing(due);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );

            assert!(
                client.closes.is_empty(),
                "a client that answered a close with a dialog was asked to \
                 close anyway, on the frame its give-back was declined: the \
                 parent is shut out from under its own prompt"
            );
            assert!(
                still_going,
                "and the close is still owed, so the backend keeps drawing and \
                 the give-back is retried -- a deadline that answered false \
                 here would strand the window instead"
            );
            assert!(
                state.panes.get(pane).is_some_and(Pane::leaving),
                "the pane is still leaving, so a second super+q cannot start a \
                 second close over the top of this one"
            );

            // And it is a retry rather than one reprieve: another frame, and
            // the request still does not go out.
            state.clock.advance(Duration::from_millis(100));
            let later = state.clock.now();
            assert!(
                state.settle_closing(later),
                "the retry is still live a frame later"
            );
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert!(
                client.closes.is_empty(),
                "the reprieve lasts as long as the give-back is owed, rather \
                 than for one frame"
            );
        }

        /// **#127 third review, finding 2: focus could land on a window nobody
        /// can see.**
        ///
        /// `settle_focus`'s topmost arm was taught to skip a pane that is
        /// `leaving` and nothing else, while the pointer arm it falls back from
        /// goes through `window_under` and so asks `Frame::covers` — opacity
        /// *and* the rectangle. A hidden workspace is parked a screen away
        /// rather than unmapped (`workspaces.lua`: a switch moves the view, not
        /// the windows), so its panes are perfectly visible to a filter that
        /// only asks about the close.
        ///
        /// Closing the only window on the workspace in view therefore handed
        /// the keyboard to a desk the user cannot see, and with it
        /// `focus_window`'s `trigger_focus`, which is what a workspace script
        /// acts on. The window that then refused to close came back to a
        /// session where `give_back`'s `settle_focus` declines — something is
        /// focused — so it was permanent. That is #127's own symptom by a
        /// second route.
        ///
        /// **Asserted at the client**, for the reason
        /// `typing_after_a_close_reaches_the_window_that_is_drawn` gives: from
        /// the compositor's own side a seat holding an off-stage surface looks
        /// exactly like one holding a visible one. Here the right answer is
        /// that the keystroke goes *nowhere* — an idle keyboard loses no
        /// characters to the wrong application. This test stops at the
        /// request; that the window which then comes back takes the keyboard is
        /// `a_refused_close_gives_the_keyboard_back_to_the_window_it_brings_back`'s
        /// to show, and until #127's fourth review nothing did.
        #[test]
        fn a_close_does_not_hand_the_keyboard_to_a_workspace_nobody_can_see() {
            tiled_fixture!(display, state, conn, queue, client, qh);

            // One monitor, because "off screen" is a question with no answer
            // without one -- `nothing_on_stage` says so itself.
            let screen = Output::new(
                "hidden-desk-test".to_string(),
                PhysicalProperties {
                    size: (0, 0).into(),
                    subpixel: Subpixel::Unknown,
                    make: "solium".to_string(),
                    model: "test".to_string(),
                },
            );
            screen.change_current_state(
                Some(Mode {
                    size: (1920, 1080).into(),
                    refresh: 60_000,
                }),
                None,
                Some(Scale::Fractional(1.0)),
                None,
            );
            state.space.map_output(&screen, (0, 0));

            let (parked, parked_pane) =
                opened_at(&mut display, &mut state, &conn, &client, &qh, (1000, 300));
            let (visible, closing) =
                opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
            // Twice, for the reason `typing_after_a_close...` gives: the
            // keyboard is a request the client makes in answer to the seat's
            // capabilities.
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert!(
                client.keyboard.is_some(),
                "the client bound a keyboard; without one this test cannot \
                 observe anything"
            );

            // Past both opening animations and settled, so no pane is still
            // drawn back near the origin where the untouched pointer is.
            state.clock.advance(Duration::from_millis(300));
            state.settle(state.clock.now());
            assert!(
                state.window_under((0.0, 0.0).into()).is_none(),
                "the pointer has not been moved and there is nothing under it, \
                 which is what makes this the keyboard's question"
            );

            // Workspace 2 goes away: one selection, carried a screen and a bit
            // to the left. `workspaces.lua`'s own arrangement and its own
            // numbers.
            let desk = state.window_id(&parked);
            let switch = state.clock.now();
            state.groups.declare(
                "desk-2",
                crate::group::Selection {
                    members: vec![crate::group::Member::Window(desk)],
                    on: None,
                },
                switch,
            );
            state.groups.present(
                "desk-2",
                crate::group::Shift {
                    dx: -1920.0 * 1.06,
                    ..crate::group::Shift::NONE
                },
                switch,
                Duration::from_millis(300),
                present::Curve::OutCubic,
            );
            state.clock.advance(Duration::from_millis(400));
            state.settle(state.clock.now());

            // The premise, both halves of it: the parked window is fully
            // opaque -- it is not hidden by being faded out, which the
            // `shows()` half of the gate would have caught on its own -- and
            // its own rectangle is still on the monitor, because a hidden
            // workspace is parked rather than unmapped. Only the *drawn*
            // rectangle knows it is gone.
            let landed = state.clock.now();
            let away = drawn_now(&state, parked_pane, landed);
            assert!(
                (away.opacity - 1.0).abs() < f32::EPSILON,
                "the parked window is fully opaque, so opacity alone cannot be \
                 what declines it"
            );
            let real = state
                .pane_outer_of(parked_pane)
                .expect("a mapped pane has a rectangle");
            assert!(
                state.on_any_output(real),
                "and it still lives on the monitor -- a workspace switch moves \
                 the view, not the windows -- so `pane_outer` cannot be what \
                 declines it either"
            );
            let screens: Vec<Rectangle<i32, Logical>> = state
                .space
                .outputs()
                .filter_map(|output| state.space.output_geometry(output))
                .collect();
            assert_eq!(
                nothing_on_stage([away.rect], &screens),
                Some(true),
                "the parked window is drawn off every monitor, which is the \
                 one thing about it that is true"
            );

            // The window the user is working in, and the premise that this
            // fixture can see where typing goes at all.
            state.focus_window(&visible, SERIAL_COUNTER.next_serial());
            types(&mut state, KEY_A);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert_eq!(
                client.typed,
                vec![(Some(surface_id(&visible)), KEY_A)],
                "the premise: typing reaches the focused window"
            );

            // And the user closes the only window they can see.
            state.close_pane(closing);
            state
                .clock
                .advance(present::CLOSING + Duration::from_millis(10));
            let asked = state.clock.now();
            state.settle_closing(asked);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );

            // The client first, because it is the only end that can tell, and
            // the compositor's own view of the seat second.
            let before = client.typed.len();
            types(&mut state, KEY_A);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert_eq!(
                client.typed.len(),
                before,
                "a keystroke after the last visible window closed goes \
                 nowhere, rather than onto a workspace the user cannot see: it \
                 reached {:?}, and the parked window's surface is {}",
                client.typed.last(),
                surface_id(&parked)
            );
            assert!(
                state.focused_window().is_none(),
                "and the seat is holding nothing at all -- it must, because \
                 `give_back`'s `settle_focus` declines when something already \
                 has focus, which is what would make this permanent rather \
                 than a wrong answer for one second"
            );
        }

        /// Whether any of this pane's drawn rectangle at `at` reaches a screen,
        /// and whether it paints anything there — the two halves of "on stage",
        /// measured the way the premises below need them measured.
        fn drawn_on_stage(state: &Solium, pane: crate::pane::PaneId, at: Duration) -> bool {
            let frame = drawn_now(state, pane, at);
            let screens: Vec<Rectangle<i32, Logical>> = state
                .space
                .outputs()
                .filter_map(|output| state.space.output_geometry(output))
                .collect();
            frame.shows() && nothing_on_stage([frame.rect], &screens) == Some(false)
        }

        /// **#127 fourth review, NEW-1: a refused window came back with the
        /// keyboard on nothing.**
        ///
        /// `give_back` calls `settle_focus` straight after `present::clear`, so
        /// the restore it has just started is at progress zero, and at progress
        /// zero a transform answers its `from` — which past `CLOSING` is
        /// `present::close`'s opacity-zero end. Judged by the frame being
        /// drawn, the window being given back was invisible, so the topmost arm
        /// declined the only window there was to focus, and the window stood
        /// back up at full opacity with the seat holding nothing.
        ///
        /// **Deterministic, where the fault was not.** In the running
        /// compositor `settle_focus` reads the clock a little after the frame's
        /// `now`, and the window escaped whenever enough real time had passed
        /// between the two for the fade to clear one step of eight bits. This
        /// hands `settle_refused` an instant the clock has not reached, so
        /// every reading `settle_focus` takes falls at or before the restore's
        /// start, where `Animation::progress` is exactly zero. The failing case,
        /// made certain rather than likely.
        ///
        /// Nothing else is open, so `hand_off_keyboard` empties the seat at the
        /// request — asserted, because that is the path the fault needs.
        /// Asserted at the client as well as the seat, for the reason
        /// `typing_after_a_close_reaches_the_window_that_is_drawn` gives.
        #[test]
        fn a_refused_close_gives_the_keyboard_back_to_the_window_it_brings_back() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            one_screen(&mut state);
            let (window, pane) =
                opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
            // Twice: the keyboard is a request the client makes in answer to
            // the seat's capabilities. See `typing_after_a_close...`.
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert!(
                client.keyboard.is_some(),
                "the client bound a keyboard; without one this test cannot \
                 observe anything"
            );
            // Past the opening animation and settled, so the untouched pointer
            // at the origin is over nothing and the topmost arm is the one
            // asked.
            state.clock.advance(Duration::from_millis(300));
            state.settle(state.clock.now());
            assert!(
                state.window_under((0.0, 0.0).into()).is_none(),
                "the pointer is over nothing, which makes this the keyboard's \
                 question and not the mouse's"
            );

            state.focus_window(&window, SERIAL_COUNTER.next_serial());
            state.close_pane(pane);
            state
                .clock
                .advance(present::CLOSING + Duration::from_millis(10));
            let asked = state.clock.now();
            state.settle_closing(asked);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert_eq!(client.closes.len(), 1, "the request went out");
            assert!(
                state.focused_window().is_none(),
                "the premise: with nothing else open, handing the keyboard off \
                 at the request leaves the seat holding nothing"
            );

            // The refusal, at an instant the clock has not reached. Past the
            // grace period, so the window is due.
            let refused = asked + Duration::from_millis(1500);
            state.settle_refused(refused);
            assert!(
                !drawn_now(&state, pane, state.clock.now()).shows(),
                "the premise: at every instant `settle_focus` could have read, \
                 the window being given back is still drawn at nothing"
            );
            assert!(
                drawn_on_stage(&state, pane, refused + Duration::from_millis(200)),
                "and it is on its way back: once the restore lands it is on \
                 screen and opaque"
            );

            assert_eq!(
                state.focused_window().as_ref(),
                Some(&window),
                "a window given back to a session whose keyboard is idle takes \
                 the keyboard, even on the frame its restore starts"
            );
            let before = client.typed.len();
            types(&mut state, KEY_A);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert_eq!(
                client.typed.get(before),
                Some(&(Some(surface_id(&window)), KEY_A)),
                "and typing reaches it, rather than going nowhere until the \
                 user clicks"
            );
        }

        /// How far a hidden desk is carried: a screen and a bit to the left,
        /// `workspaces.lua`'s own arrangement and its own numbers.
        const AWAY: f64 = -1920.0 * 1.06;

        /// Two desks with one window each, and a switch between them on its
        /// first frame.
        ///
        /// Returns `(leaving, arriving)`. `leaving` is on the desk in view and
        /// is opened second, so it is the topmost pane: a walk whose right
        /// answer is also the topmost one is not testing the walk. `arriving`
        /// starts parked `AWAY` and settled there.
        ///
        /// **The switch starts at an instant the clock has not reached**, so
        /// every reading `settle_focus` takes falls at or before its start,
        /// where `Animation::progress` is exactly zero — the first frame, made
        /// certain rather than likely. Its premises are asserted here, both
        /// ends of them: on that frame `leaving` is drawn on stage and
        /// `arriving` off it, and once the switch lands it is the other way
        /// round. Focus that follows the drawn frame and focus that follows the
        /// destination give different answers only because of this.
        ///
        /// The seat is emptied by hand at the end. How it came to be empty is
        /// not the question — a close, a lock, a window going — what
        /// `settle_focus` does about it is.
        fn a_switch_on_its_first_frame(
            display: &mut Display<Solium>,
            state: &mut Solium,
            conn: &Connection,
            qh: &QueueHandle<Client>,
            queue: &mut wayland_client::EventQueue<Client>,
            client: &mut Client,
        ) -> ((Window, crate::pane::PaneId), (Window, crate::pane::PaneId)) {
            one_screen(state);
            let arriving = opened_at(display, state, conn, client, qh, (1000, 300));
            let leaving = opened_at(display, state, conn, client, qh, (400, 300));
            pump(display, state, conn, qh, queue, client);
            pump(display, state, conn, qh, queue, client);
            assert!(
                client.keyboard.is_some(),
                "the client bound a keyboard; without one this test cannot \
                 observe anything"
            );
            state.clock.advance(Duration::from_millis(300));
            state.settle(state.clock.now());

            let parked = state.clock.now();
            for (name, window) in [("desk-1", &leaving.0), ("desk-2", &arriving.0)] {
                let id = state.window_id(window);
                state.groups.declare(
                    name,
                    crate::group::Selection {
                        members: vec![crate::group::Member::Window(id)],
                        on: None,
                    },
                    parked,
                );
            }
            state.groups.present(
                "desk-2",
                crate::group::Shift {
                    dx: AWAY,
                    ..crate::group::Shift::NONE
                },
                parked,
                Duration::from_millis(300),
                present::Curve::OutCubic,
            );
            state.clock.advance(Duration::from_millis(400));
            state.settle(state.clock.now());

            let switch = state.clock.now() + Duration::from_millis(50);
            state.groups.present(
                "desk-1",
                crate::group::Shift {
                    dx: AWAY,
                    ..crate::group::Shift::NONE
                },
                switch,
                Duration::from_millis(300),
                present::Curve::OutCubic,
            );
            state.groups.present(
                "desk-2",
                crate::group::Shift::NONE,
                switch,
                Duration::from_millis(300),
                present::Curve::OutCubic,
            );

            let first = state.clock.now();
            let landed = switch + Duration::from_millis(400);
            assert!(
                drawn_on_stage(state, leaving.1, first)
                    && !drawn_on_stage(state, arriving.1, first),
                "the premise: on the switch's first frame the desk being left is \
                 still the one drawn"
            );
            assert!(
                !drawn_on_stage(state, leaving.1, landed)
                    && drawn_on_stage(state, arriving.1, landed),
                "and once it lands the desk switched to is"
            );

            assert!(
                state.seat.get_keyboard().is_some(),
                "the fixture's seat has a keyboard; without one there is no \
                 focus to be wrong about"
            );
            state.give_keyboard(None, SERIAL_COUNTER.next_serial());
            assert!(state.focused_window().is_none(), "the seat is empty");
            (leaving, arriving)
        }

        /// **#127 fourth review, NEW-2: on a workspace switch's first frame,
        /// focus judged by where the windows had been.**
        ///
        /// `on_stage` sampled the instant it was called, while
        /// `everything_is_off_stage` — the one other caller asking the same
        /// question — samples `SETTLED` ahead, because a group transform that
        /// has just started is at progress zero. So `settle_focus` inside the
        /// first frames of a switch handed the keyboard to the desk being left
        /// and passed over the desk being switched to. The pointer is over
        /// nothing, so this is the topmost arm.
        #[test]
        fn the_first_frame_of_a_workspace_switch_focuses_the_desk_switched_to() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let ((leaving, _), (arriving, _)) = a_switch_on_its_first_frame(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert!(
                state.window_under((0.0, 0.0).into()).is_none(),
                "the pointer is over nothing, so the topmost arm is the one \
                 asked"
            );

            state.settle_focus();
            assert_eq!(
                state.focused_window().as_ref(),
                Some(&arriving),
                "focus goes to the desk being switched to, not to the one being \
                 left (surface {}), which is topmost and still drawn",
                surface_id(&leaving)
            );
            let before = client.typed.len();
            types(&mut state, KEY_A);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert_eq!(
                client.typed.get(before),
                Some(&(Some(surface_id(&arriving)), KEY_A)),
                "and typing reaches it"
            );
        }

        /// **The pointer arm of the same question, and where the two rules
        /// meet.**
        ///
        /// `settle_focus` asks the window under the pointer first, because with
        /// focus-follows-mouse that is where focus would land the moment the
        /// pointer moved — and *the moment it moved* is after the switch has
        /// landed, not on this frame. So a pointer resting where the desk being
        /// left is drawn must not hand that desk the keyboard.
        ///
        /// **And the click on the same pixel still goes to what is drawn**,
        /// which is the half that must not change: `window_under` is the hit
        /// test, a press lands on what is on screen this frame, and on this
        /// frame that is the window being left. Both are asserted at the same
        /// point on the same frame, so a fix that moved the hit test instead
        /// fails here.
        #[test]
        fn a_pointer_over_the_desk_being_left_does_not_hand_it_the_keyboard() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let ((leaving, _), (arriving, _)) = a_switch_on_its_first_frame(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            // The middle of the window being left, as it is drawn this frame.
            let point: Point<f64, Logical> = (432.0, 332.0).into();
            let pointer = state.seat.get_pointer().expect(
                "the fixture's seat has a pointer; without one there is no \
                 pointer arm to test",
            );
            pointer.motion(
                &mut state,
                None,
                &smithay::input::pointer::MotionEvent {
                    location: point,
                    serial: SERIAL_COUNTER.next_serial(),
                    time: 0,
                },
            );
            pointer.frame(&mut state);
            assert_eq!(
                state.window_under(point).map(|(window, _)| window).as_ref(),
                Some(&leaving),
                "a press on this pixel this frame lands on the window drawn \
                 there, which is the desk being left -- the hit test judges \
                 the present frame, and that is right"
            );

            state.settle_focus();
            assert_eq!(
                state.focused_window().as_ref(),
                Some(&arriving),
                "but the keyboard goes where the desks are settling: the window \
                 under the pointer is sliding off stage (surface {}), so the \
                 desk switched to takes it",
                surface_id(&leaving)
            );
        }

        /// **A closing window under the pointer, while it is still fading.**
        ///
        /// The case [`SETTLED`] lists first. `settle_focus`'s pointer arm used
        /// to get "never a window on its way out" from `window_under`, which
        /// declines a pane only once it shows nothing — so for the 190 ms a
        /// close is still playing, a seat left empty by anything else handed
        /// the keyboard to the window being closed, if the pointer was resting
        /// on it. The topmost arm's `leaving()` filter never reached it: the
        /// pointer arm answers first.
        ///
        /// Judged at the destination, a closing pane is at opacity zero from
        /// the press, so it is no candidate; and the same press on the same
        /// frame still lands on it, because it is still drawn and a half-faded
        /// window keeps its clicks. Both are asserted.
        #[test]
        fn a_window_mid_close_under_the_pointer_is_not_handed_the_keyboard() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            one_screen(&mut state);
            let (kept, _) = opened_at(&mut display, &mut state, &conn, &client, &qh, (1000, 300));
            let (doomed, closing) =
                opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            state.clock.advance(Duration::from_millis(300));
            state.settle(state.clock.now());

            let point: Point<f64, Logical> = (432.0, 332.0).into();
            let pointer = state.seat.get_pointer().expect(
                "the fixture's seat has a pointer; without one there is no \
                 pointer arm to test",
            );
            pointer.motion(
                &mut state,
                None,
                &smithay::input::pointer::MotionEvent {
                    location: point,
                    serial: SERIAL_COUNTER.next_serial(),
                    time: 0,
                },
            );
            pointer.frame(&mut state);

            // A quarter of the way into the fade: drawn, visibly, and still
            // taking its clicks.
            state.close_pane(closing);
            state.clock.advance(Duration::from_millis(50));
            assert!(
                drawn_now(&state, closing, state.clock.now()).shows(),
                "the premise: the window being closed is still on screen"
            );
            assert_eq!(
                state.window_under(point).map(|(window, _)| window).as_ref(),
                Some(&doomed),
                "and a press on it this frame lands on it, which is right"
            );

            assert!(
                state.seat.get_keyboard().is_some(),
                "the fixture's seat has a keyboard; without one there is no \
                 focus to be wrong about"
            );
            state.give_keyboard(None, SERIAL_COUNTER.next_serial());
            state.settle_focus();
            assert_eq!(
                state.focused_window().as_ref(),
                Some(&kept),
                "but the keyboard is not handed to a window that is on its way \
                 out (surface {}): the one staying takes it",
                surface_id(&doomed)
            );
        }

        /// The protocol id of a window's surface.
        ///
        /// The one number the two ends of this fixture share. A `wl_surface` is
        /// created by the client, so the id it picked is the id the server
        /// knows it by, and a test holding the server's `Window` can say which
        /// surface a `wl_keyboard.enter` named without threading the client's
        /// proxy through every helper.
        fn surface_id(window: &Window) -> u32 {
            window
                .wl_surface()
                .expect("a mapped client has a surface")
                .id()
                .protocol_id()
        }

        /// One keystroke, through the same `KeyboardHandle::input` that
        /// `crate::input::keyboard` ends in.
        ///
        /// `Forward` unconditionally: whether a combination is a binding is
        /// `input::keyboard`'s question and not this one's.
        ///
        /// `key` is the evdev code, which is what an input backend reports and
        /// what the client is sent. The `+ 8` in the middle is the X11 offset
        /// libxkbcommon works in and smithay unwinds again on the wire, so the
        /// number that goes in here is the number that comes out at the other
        /// end — and a test asserting on a different one would be asserting on
        /// this fixture's arithmetic rather than on the compositor's.
        fn types(state: &mut Solium, key: u32) {
            let keyboard = state.seat.get_keyboard().expect(
                "the fixture's seat has a keyboard; without one there is no \
                 typing to be wrong about",
            );
            keyboard.input::<(), _>(
                state,
                smithay::input::keyboard::Keycode::new(key + 8),
                smithay::backend::input::KeyState::Pressed,
                SERIAL_COUNTER.next_serial(),
                0,
                |_, _, _| smithay::input::keyboard::FilterResult::Forward,
            );
        }

        /// `KEY_A`, as evdev and the client both spell it. See [`types`].
        const KEY_A: u32 = 30;

        /// **#127 second review, finding 1: the keystroke fix only worked via a
        /// click.**
        ///
        /// The review before this one stopped an invisible closing pane from
        /// winning `window_under`, and `window_under` is what click-to-focus
        /// focuses — so a press where the dead window used to be goes to the
        /// window that is drawn there, and the typing after it follows. That is
        /// the whole of the fix, and it requires the user to touch the mouse.
        ///
        /// **Nothing moved focus otherwise.** `close_pane` does not, and
        /// `settle_focus` runs from `sync_panes` only when the pane set
        /// changes, which a close that has been asked and not yet answered does
        /// not do. So `super+q` and carry on typing — the ordinary way anyone
        /// meets this — put every character into a window at opacity zero for
        /// the 190 ms animation *and* the 1000 ms grace after it, while the
        /// sibling that reflowed into the space sat on screen looking like the
        /// thing being typed into.
        ///
        /// **Asserted at the client, which is the only end that can tell.**
        /// From the compositor's own side a seat holding an invisible surface
        /// looks exactly like one holding a visible one; `give_keyboard`'s
        /// `true` says only that the lock's rule allowed the surface, and
        /// nothing complains about opacity. So this binds a `wl_keyboard` and
        /// asks where the key came out. See [`Client::typed`].
        ///
        /// The premise is typed first, before the close: a test whose second
        /// half passes because the keyboard was never wired at all would be
        /// worth nothing, and this makes that failure loud instead.
        #[test]
        fn typing_after_a_close_reaches_the_window_that_is_drawn() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (kept, _survivor) =
                opened_at(&mut display, &mut state, &conn, &client, &qh, (1000, 300));
            let (doomed, closing) =
                opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
            // Twice: the first round trip carries the seat's capabilities to
            // the client, and `get_keyboard` is a request it makes in answer to
            // them, so the keyboard does not exist on the server until the
            // second.
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert!(
                client.keyboard.is_some(),
                "the client bound a keyboard; without one this test cannot \
                 observe anything"
            );

            // Past both opening animations, and *settled*, which is what the
            // render loop does once a frame.
            //
            // Not housekeeping: `present::open`'s target was captured before
            // `opened_at` moved each window, so until that released transform
            // is retired a pane is still drawn near the origin — where the
            // pointer is sitting, having never been moved. `settle_focus` asks
            // `window_under` first, so without this it answers from the pointer
            // and the topmost-pane arm that this test is about is never
            // reached. It passed that way, for a reason that had nothing to do
            // with the fix.
            state.clock.advance(Duration::from_millis(300));
            state.settle(state.clock.now());
            assert!(
                state.window_under((0.0, 0.0).into()).is_none(),
                "the pointer has not been moved and there is nothing under it, \
                 which is what makes this the keyboard's question and not the \
                 mouse's"
            );

            // The window the user is working in, which is the one they are
            // about to close.
            state.focus_window(&doomed, SERIAL_COUNTER.next_serial());
            types(&mut state, KEY_A);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert_eq!(
                client.typed,
                vec![(Some(surface_id(&doomed)), KEY_A)],
                "the premise: typing reaches the focused window, and this \
                 fixture can see it happen"
            );

            state.close_pane(closing);
            // Past `CLOSING`: the animation has landed, the request has gone
            // out, and the fixture's client never destroys anything — a client
            // still running its quit handlers, which is what the longer grace
            // period exists for.
            state
                .clock
                .advance(present::CLOSING + Duration::from_millis(10));
            let asked = state.clock.now();
            state.settle_closing(asked);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );

            // The situation under test, stated rather than assumed: the window
            // that was closed is drawn at nothing, and the user has not touched
            // the mouse — there has been no press for `window_under` to answer.
            assert!(
                drawn_now(&state, closing, asked).opacity.abs() < f32::EPSILON,
                "the closed window is invisible, which is what makes typing \
                 into it silent"
            );

            // And the user keeps typing, which is the whole of the case.
            types(&mut state, KEY_A);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert_eq!(
                client.typed.last(),
                Some(&(Some(surface_id(&kept)), KEY_A)),
                "a keystroke after a close belongs to the window that is drawn, \
                 not to the one that was closed: it went to surface {:?}, and \
                 the closed window's is {}",
                client.typed.last().and_then(|(surface, _)| *surface),
                surface_id(&doomed)
            );
        }

        /// **#127 second review, finding 2: suppressing `map_stacked` left the
        /// three copies of a pane's position disagreeing.**
        ///
        /// The guard `move_pane` needs is over the *configure*: a client that
        /// answers a new size while `present::close` holds `frame.rect` pinned
        /// gets its last buffer stretched mid-fade, because those two
        /// rectangles are the pair `resizing::factor` divides. `map_stacked`
        /// carries a location and no size, so it is no part of that — and
        /// suppressing it anyway cost what `move_pane`'s own opening paragraph
        /// warns about: *setting the slot without telling the space is undone
        /// before the next frame is drawn, silently*. `pane_geometry` reads
        /// `real_geometry` for a mapped client, so `sync_panes` copied the
        /// stale rectangle straight back over `set_slot`.
        ///
        /// **`sync_panes` is the line that made it a defect**, and a test that
        /// only looked at the frame `move_pane` returns on would pass against
        /// the bug. The sweep is a workspace switch, a `rescue_offscreen` or a
        /// config reload; the refusal is a client that goes on living. Between
        /// them the window came back where it had been *closed*, while
        /// `Pane::placed` said the layout had moved it — which is the pair
        /// `pane_laid_out` hands #124's edge drag.
        ///
        /// **And the last assertion is the one this replaces a comment with.**
        /// The client's *size* really is one configure behind while the pane is
        /// leaving, because that configure is deliberately never sent. The
        /// claim made for that is that the next sweep after the window comes
        /// back sends it — the suppression happened before `offers_size` could
        /// record the size as told, so re-placing at the same rectangle is
        /// still a change. That is asserted here rather than promised there.
        #[test]
        fn a_window_the_layout_moved_mid_close_comes_back_where_it_was_put() {
            tiled_fixture!(display, state, conn, queue, client, qh);
            let (window, pane) =
                opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );

            let was = state
                .pane_outer_of(pane)
                .expect("a mapped pane has a rectangle");
            state.close_pane(pane);
            state
                .clock
                .advance(present::CLOSING + Duration::from_millis(10));
            let asked = state.clock.now();
            state.settle_closing(asked);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert_eq!(client.closes.len(), 1, "the request went out");

            // The sweep, mid-close. A different size as well as a different
            // place, so that the configure guard is still being exercised and
            // this is not quietly testing a move nobody suppressed.
            let elsewhere = at(1000, 500, 250, 180);
            let configures = client.configures.len();
            state.move_pane(
                pane,
                elsewhere,
                was,
                AnimationSpec::default(),
                asked,
                Standing::Tile,
            );
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert_eq!(
                client.configures.len(),
                configures,
                "the leaving client is still told nothing, which is the guard \
                 that belongs here"
            );

            // The next frame. This is where the defect lived: the space was
            // never told, so the space's stale rectangle came back over the
            // slot the sweep had just written.
            state.sync_panes();

            let outer = state
                .pane_outer_of(pane)
                .expect("a mapped pane has a rectangle");
            assert_eq!(
                outer.loc, elsewhere.loc,
                "a pane the layout moved during a close lives where the layout \
                 put it, one frame later as well as on the frame it was moved"
            );
            let laid_out = state
                .pane_laid_out(&window)
                .expect("a pane the layout has placed answers this");
            assert_eq!(
                laid_out.0.loc, outer.loc,
                "and the rectangle #124's edge drag starts from is one the \
                 window is actually at: `pane_laid_out` says {:?} and the pane \
                 is at {:?}",
                laid_out.0.loc, outer.loc
            );

            // The refusal. The client never destroyed anything, so the deadline
            // is what brings the window back.
            let refused = asked + Duration::from_millis(1500);
            state.settle_refused(refused);
            let back = drawn_now(&state, pane, refused + Duration::from_millis(200));
            assert!(
                (back.opacity - 1.0).abs() < f32::EPSILON,
                "the premise for the rest: the window is back on screen, at \
                 opacity {}",
                back.opacity
            );
            assert_eq!(
                state
                    .pane_outer_of(pane)
                    .expect("a mapped pane has a rectangle")
                    .loc,
                elsewhere.loc,
                "and it comes back where the layout left it rather than where \
                 it was closed"
            );

            // The residue, and the claim made about it. The size is still the
            // one the client last committed, because the configure that would
            // have changed it was suppressed — and the next sweep sends it.
            let configures = client.configures.len();
            state.move_pane(
                pane,
                elsewhere,
                elsewhere,
                AnimationSpec::default(),
                refused + Duration::from_millis(200),
                Standing::Tile,
            );
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert!(
                client.configures.len() > configures,
                "the configure the close swallowed is sent by the first sweep \
                 after the window comes back, which is what makes the \
                 suppression a delay rather than a loss"
            );
        }
    }

    /// **#100, the Wayland half: a menu near a screen edge was drawn off it.**
    ///
    /// `new_popup` took the positioner as `_positioner` and dropped it, which
    /// left Smithay's `get_geometry()` standing — anchor and gravity honoured,
    /// `constraint_adjustment` ignored. This walks the arithmetic of the case
    /// that produces: a window whose right edge is near the right edge of a
    /// 1920-wide screen, and a context menu anchored at that edge opening
    /// rightwards.
    ///
    /// Both directions are asserted. The first assertion is the bug — the
    /// placement the old code produced is *outside* the screen — and the
    /// second is the fix. Without the first, a `popup_target` that returned
    /// something absurdly large would pass the test by making every placement
    /// look fine.
    ///
    /// It is a unit test of arithmetic rather than of `place_popup`, because
    /// `place_popup` needs a live `PopupSurface`, which needs a client. What
    /// it does pin is the part that was wrong: the translation between the
    /// compositor's coordinates and the positioner's, which is `popup_target`,
    /// and the fact that we ask for the *unconstrained* geometry.
    #[test]
    fn a_menu_at_the_screen_edge_is_flipped_back_onto_it() {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_positioner::{
            Anchor, ConstraintAdjustment, Gravity,
        };

        let screen = at(0, 0, 1920, 1080);
        // A window whose right edge is at x = 1900, twenty pixels short of the
        // screen's.
        let window = at(1400, 100, 500, 800);

        // What a toolkit sends for a menu hung off a control near that edge:
        // anchored on the right of a small widget, opening rightwards, and
        // allowed to flip on either axis if that does not fit.
        let positioner = PositionerState {
            rect_size: (200, 300).into(),
            anchor_rect: at(450, 200, 10, 10),
            anchor_edges: Anchor::Right,
            gravity: Gravity::Right,
            constraint_adjustment: ConstraintAdjustment::FlipX | ConstraintAdjustment::FlipY,
            ..PositionerState::default()
        };

        // No parent popups: this menu hangs off the toplevel itself.
        let parents = Point::<i32, Logical>::from((0, 0));
        let target = popup_target(screen, window.loc, parents);

        // The placement before the fix, in compositor coordinates.
        let unconstrained = positioner.get_geometry();
        let on_screen = |geometry: Rectangle<i32, Logical>| {
            Rectangle::new(window.loc + parents + geometry.loc, geometry.size)
        };
        assert!(
            !screen.contains_rect(on_screen(unconstrained)),
            "the test case has stopped reaching off the screen, so it no \
             longer pins anything: {:?}",
            on_screen(unconstrained)
        );

        // And after it.
        let constrained = positioner.get_unconstrained_geometry(target);
        assert!(
            screen.contains_rect(on_screen(constrained)),
            "a popup that was allowed to flip is still off the screen: {:?}",
            on_screen(constrained)
        );
        // Flipped rather than merely shrunk: the size the client asked for is
        // the size it gets, which is the difference between a menu with its
        // entries in it and a menu with a scrollbar.
        assert_eq!(constrained.size, positioner.rect_size);
    }

    /// A submenu is measured from its parent popup, not from the window.
    ///
    /// `get_popup_toplevel_coords` is the second of the two translations in
    /// `popup_target`, and it is the one with nothing else to catch it: a
    /// first-level menu has a zero offset, so dropping the term entirely would
    /// leave every test that uses one passing. The screen a submenu is
    /// constrained against has to be moved by how far down the chain it hangs,
    /// or the deeper it goes the more room it thinks it has.
    #[test]
    fn a_submenu_is_offset_by_the_chain_above_it() {
        let screen = at(0, 0, 1920, 1080);
        let root = Point::<i32, Logical>::from((1400, 100));
        let parents = Point::<i32, Logical>::from((250, 60));

        let direct = popup_target(screen, root, (0, 0).into());
        let nested = popup_target(screen, root, parents);

        assert_eq!(nested.loc, direct.loc - parents);
        assert_eq!(nested.size, screen.size);
        // A point that is the top-left of the screen in compositor
        // coordinates is the top-left of the target in either popup's own.
        assert_eq!(root + parents + nested.loc, screen.loc);
    }

    /// **The refusal path a stale serial takes.**
    ///
    /// A client may ask for a popup grab with any serial it likes, including
    /// one from an event that is long gone or one it never received. The
    /// compositor's answer has to be "no" — `grab` declines and returns —
    /// rather than an assertion or an unwrap, because there is nothing above a
    /// compositor to restart it.
    ///
    /// `may_grab` is the half of that decision that can be pinned without a
    /// client: the seat answers three booleans about a device and this decides
    /// whether a popup may take it. The other half — `grab_popup` returning
    /// `Err` for a popup that is already mapped, orphaned, or not the topmost
    /// — is Smithay's, and `grab` handles it by logging and returning; see the
    /// `match` there.
    #[test]
    fn a_grab_held_by_a_stranger_is_refused() {
        // Nothing holds the device: the ordinary case, a menu opening while
        // the compositor is idle.
        assert!(may_grab(false, false, false));
        // This chain already holds it. A submenu opening inside its parent's
        // grab arrives here, and refusing it would break every nested menu.
        assert!(may_grab(true, true, false));
        assert!(may_grab(true, false, true));
        // Somebody else holds it -- one of Solium's own move or resize grabs,
        // or a serial this client made up. Declined.
        assert!(!may_grab(true, false, false));
    }
    /// **A drag owns the pointer, so the compositor stops describing what is
    /// under it.**
    ///
    /// A client that starts a drag gets a `DnDGrab` installed on the pointer,
    /// so `PointerHandle::is_grabbed` is true for the whole gesture -- checked
    /// against smithay 0.7's `selection/data_device/device.rs`, which calls
    /// `set_grab` with it, and `input/pointer/mod.rs`, where `is_grabbed` is
    /// `!matches!(guard.grab, GrabStatus::None)`. That is the machinery, and it
    /// is why a drag needs no flag of its own: [`Solium::assert_cursor`]
    /// already declines to recompute while the pointer is grabbed.
    ///
    /// What it buys is that dragging a file across a window's edge does not
    /// make the pointer offer a resize. The press that would perform that
    /// resize cannot happen -- the button is already down and belongs to the
    /// drag -- so a resize arrow there is #108's symptom again, arrived at from
    /// a fourth direction, and it would flicker on and off along every edge the
    /// drag crosses.
    ///
    /// The `false` half is what makes this able to fail: the same call with no
    /// grab clears the shape, so an `assert_cursor` that had lost its early
    /// return would clear it in both.
    #[test]
    fn a_grabbed_pointer_keeps_the_shape_it_had() {
        let display = smithay::reexports::wayland_server::Display::<Solium>::new()
            .expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());

        // Where a border drag leaves the pointer: `input::pointer_button`
        // asserts the shape as it starts the grab, precisely so that it holds
        // for the drag. A DnD grab arrives at the same place by a different
        // road -- whatever was showing when the buttons went down.
        assert!(state.pointer.assert(Some(CursorIcon::NwseResize)));

        // Nothing is mapped, so the hit test under this point claims nothing
        // and the compositor's answer for it is "say nothing" -- which is a
        // *write*, and the one the grab has to suppress.
        let location = Point::<f64, Logical>::from((300.0, 300.0));
        assert_eq!(state.claim_under(location), Claim::Nothing);
        assert_eq!(state.claim_under(location).cursor(), None);

        state.assert_cursor(location, true);
        assert_eq!(
            state.pointer.showing(),
            CursorImageStatus::Named(CursorIcon::NwseResize),
            "a grab owns the pointer until it ends, so crossing anything \
             underneath must not change the shape"
        );

        state.assert_cursor(location, false);
        assert_eq!(
            state.pointer.showing(),
            CursorImageStatus::default_named(),
            "and the first motion after the grab ends hands the pointer back \
             to the ordinary hit test"
        );
    }
    /// **Issue #57's state machine: the icon is kept for exactly one drag.**
    ///
    /// Needs a real `WlSurface`, and one cannot be conjured: smithay offers no
    /// constructor, and `wl_surface`'s server-side user data type is private,
    /// so `Client::create_resource` cannot name it either. A client has to ask
    /// over the wire. That is the same conclusion `scale_resend` above reaches
    /// for `ToplevelSurface`, and this fixture is deliberately its smaller
    /// half: one global, one surface, no `xdg_shell`, no buffer, no output.
    ///
    /// **Nothing here may build a Qt scene**, which is why no toplevel is
    /// opened and none is needed. A window mapped inside a process that is
    /// already holding a raw libwayland connection aborts the whole test
    /// binary -- see the long note in `scale_resend`, which found that the hard
    /// way. A bare `wl_surface` never reaches `new_toplevel`, so no decoration
    /// is ever built for it.
    mod drag_icon {
        use super::*;
        use smithay::reexports::wayland_server::Display;
        use std::os::unix::net::UnixStream;
        use wayland_client::protocol::{wl_compositor, wl_registry, wl_surface};
        use wayland_client::{Connection, Dispatch, Proxy as _, QueueHandle};

        /// The client side: `wl_compositor` and nothing else, because a
        /// surface is the whole of what is wanted.
        #[derive(Debug, Default)]
        struct Client {
            compositor: Option<wl_compositor::WlCompositor>,
        }

        impl Dispatch<wl_registry::WlRegistry, ()> for Client {
            fn event(
                state: &mut Self,
                registry: &wl_registry::WlRegistry,
                event: wl_registry::Event,
                (): &(),
                _conn: &Connection,
                qh: &QueueHandle<Self>,
            ) {
                let wl_registry::Event::Global {
                    name, interface, ..
                } = event
                else {
                    return;
                };
                if interface == "wl_compositor" {
                    state.compositor = Some(registry.bind(name, 1, qh, ()));
                }
            }
        }

        wayland_client::delegate_noop!(Client: ignore wl_compositor::WlCompositor);
        wayland_client::delegate_noop!(Client: ignore wl_surface::WlSurface);

        /// Start a drag, drop it, start a second one, and destroy the surface
        /// under that one.
        ///
        /// One test rather than three because the fixture is the expensive
        /// part -- a display, a socket pair and a round trip -- and because
        /// each step is the next step's precondition: "cleared on drop" says
        /// nothing unless something was there to be cleared.
        #[test]
        fn an_icon_lasts_one_drag_and_outlives_neither_the_drop_nor_its_surface() {
            let mut display = Display::<Solium>::new().expect("creating a test wayland display");
            let mut state = Solium::new(display.handle());

            let (server_side, client_side) =
                UnixStream::pair().expect("a socket pair for the test client");
            let served = display
                .handle()
                .insert_client(server_side, std::sync::Arc::new(ClientState::default()))
                .expect("inserting the test client");
            let conn = Connection::from_socket(client_side).expect("wrapping the client socket");
            let mut event_queue = conn.new_event_queue::<Client>();
            let qh = event_queue.handle();
            let mut client = Client::default();

            conn.display().get_registry(&qh, ());
            conn.flush().expect("flushing get_registry");
            display
                .dispatch_clients(&mut state)
                .expect("dispatching get_registry");
            display
                .flush_clients()
                .expect("flushing the registry snapshot");
            // Safe to block: the server wrote the whole registry on the line
            // above and nothing but this thread drives it, so these bytes are
            // already in the kernel buffer. Same argument as `scale_resend`'s
            // one blocking read.
            event_queue
                .blocking_dispatch(&mut client)
                .expect("reading the registry snapshot");

            let compositor = client.compositor.clone().expect("wl_compositor bound");
            let asked = compositor.create_surface(&qh, ());
            conn.flush().expect("flushing create_surface");
            display
                .dispatch_clients(&mut state)
                .expect("dispatching create_surface");

            // The compositor's own handle on the surface the client just made.
            // The protocol id is the same number on both sides of one
            // connection, which is what makes this lookup exact rather than a
            // search for "the only surface around".
            let icon: WlSurface = served
                .object_from_protocol_id(&display.handle(), asked.id().protocol_id())
                .expect("the compositor made a wl_surface for the request");

            // Taken once: every call below needs it, and it cannot be read
            // out of `state` in the same expression that borrows `state`
            // mutably.
            let seat = state.seat.clone();

            // A drag with no icon is ordinary -- a text selection dragged
            // inside one window often has none -- and must leave nothing
            // behind to be drawn.
            ClientDndGrabHandler::started(&mut state, None, None, seat.clone());
            assert!(
                state.dnd_icon().is_none(),
                "a drag the client chose not to illustrate draws nothing"
            );

            // The drag the issue is about.
            state.redraw = false;
            ClientDndGrabHandler::started(&mut state, None, Some(icon.clone()), seat.clone());
            assert_eq!(
                state.dnd_icon().as_ref(),
                Some(&icon),
                "the icon is offered exactly once, at the start of the drag, \
                 and keeping it is the whole of #57"
            );
            assert!(
                state.redraw,
                "the icon appears at a pointer that has not moved, so nothing \
                 else on screen damages the region it is about to occupy"
            );

            // The buttons come up. `DnDGrab::unset` calls its own `drop`,
            // which calls this, so a cancelled or stolen grab arrives here
            // too -- checked against smithay 0.7's `dnd_grab.rs`.
            state.redraw = false;
            ClientDndGrabHandler::dropped(&mut state, None, true, seat.clone());
            assert!(
                state.dnd_icon.is_none(),
                "the drag is over, so the icon stops being drawn -- otherwise \
                 it stays painted over the session that outlived it"
            );
            assert!(state.redraw, "and the frame that removes it has to happen");

            // A drop that nobody accepted ends the drag just as thoroughly.
            ClientDndGrabHandler::started(&mut state, None, Some(icon.clone()), seat.clone());
            ClientDndGrabHandler::dropped(&mut state, None, false, seat.clone());
            assert!(state.dnd_icon.is_none());

            // **The client goes away mid-drag**, which never reaches `dropped`:
            // the grab is only unset when the buttons come up, and an
            // application that is gone will not be raising any. Destroying the
            // surface is the same road a disconnect takes -- every object the
            // client owned is destroyed -- and it is the deterministic half.
            ClientDndGrabHandler::started(&mut state, None, Some(icon.clone()), seat.clone());
            assert!(state.dnd_icon().is_some());
            asked.destroy();
            conn.flush().expect("flushing the surface destroy");
            display
                .dispatch_clients(&mut state)
                .expect("dispatching the surface destroy");
            assert!(!icon.alive(), "the fixture destroyed the surface");
            assert!(
                state.dnd_icon().is_none(),
                "a dead surface produces no render elements, so keeping one \
                 is a drag icon that is never drawn and never cleared"
            );
            assert!(
                state.dnd_icon.is_none(),
                "and the reader clears the field rather than filtering it on \
                 every frame for the rest of the session"
            );
        }
    }
}
