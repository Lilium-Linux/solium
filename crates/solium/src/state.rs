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
        LayerSurface, PopupKeyboardGrab, PopupKind, PopupManager, PopupPointerGrab,
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
            set_data_device_focus, set_data_device_selection,
        },
        selection::primary_selection::{
            PrimarySelectionHandler, PrimarySelectionState, clear_primary_selection,
            request_primary_client_selection, set_primary_focus, set_primary_selection,
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
    script::{AnimationSpec, Command, Outcome, Rect, Scripts, Snapshot, WindowInfo},
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

    /// A resize asked for by an edge drag, not yet applied.
    ///
    /// Offered to layouts first: in a tiled or scrolling arrangement a window
    /// does not have a size of its own to change — dragging its edge moves the
    /// seam it shares with its neighbour, or the width of its column. Only a
    /// floating window is resized directly.
    pub(crate) pending_resize: Option<ResizeRequest>,

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

/// An edge drag in progress.
///
/// Carries where the pointer *is* rather than how far it moved. A layout sets
/// its seam from the position directly, so dragging to the same place twice
/// gives the same result; feeding it deltas fed the layout's own response back
/// in as the next input.
#[derive(Clone, Debug)]
pub(crate) struct ResizeRequest {
    pub(crate) window: Window,
    /// Where a floating window would be put, for when no layout claims it.
    pub(crate) wanted: Rectangle<i32, Logical>,
    pub(crate) at: (f64, f64),
    pub(crate) horizontal: bool,
    pub(crate) vertical: bool,
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
/// Two gates, and both are about what a press there could actually *do*:
///
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
    managed: bool,
    window: bool,
    framed: bool,
    edges: ResizeEdge,
) -> Option<Chrome> {
    if !managed {
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
            seat,
            clock: Clock::new(),
            profile: Profile::from_env(),
            scripts: None,
            status: String::new(),
            script_grab: false,
            socket_name: String::new(),
            decorations: Decorations::default(),
            pointer: crate::cursor::Pointer::default(),
            programs: crate::pass::Programs::default(),
            published_windows: String::new(),
            focusing: false,
            pending_drop: None,
            pending_resize: None,
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
    pub(crate) fn pane_geometry(&self, pane: &Pane) -> Option<Rectangle<i32, Logical>> {
        let Some(window) = pane.client() else {
            return Some(pane.slot());
        };
        // A client that has mapped and not yet answered the size it was asked
        // for has a window of no size at all. Taking the space's word for that
        // collapses the pane to nothing for as long as it lasts -- which is
        // most of the moment an application is starting, and is exactly the
        // blank gap between the scene and the client. The slot is what the
        // layout said, and it is still the truth. Same rule as `Panes::sync`.
        match self.real_geometry(window) {
            Some(real) if real.size.w > 0 && real.size.h > 0 => Some(real),
            _ => Some(pane.slot()),
        }
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

    /// A pane as drawn, frame included. Every presentation transform is
    /// expressed against this.
    pub(crate) fn pane_outer(&self, pane: &Pane) -> Option<Rectangle<i32, Logical>> {
        Some(grown(self.pane_geometry(pane)?, self.insets_of(pane.id())))
    }

    /// The same, for a caller that holds only the pane's id.
    pub(crate) fn pane_outer_of(&self, id: crate::pane::PaneId) -> Option<Rectangle<i32, Logical>> {
        self.pane_outer(self.panes.get(id)?)
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
            let Some(outer) = self.pane_outer(pane) else {
                continue;
            };
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
                self.drawn(pane.id(), self.pane_outer(pane)?).rect
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
    pub(crate) fn pointer_over(&self, id: crate::pane::PaneId) -> bool {
        let Some(outer) = self.panes.get(id).and_then(|pane| self.pane_outer(pane)) else {
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
            self.space.map_element(window, slot.loc, false);
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

        let stack: Vec<(Window, Rectangle<i32, Logical>)> = self
            .space
            .elements()
            .filter_map(|window| Some((window.clone(), self.real_geometry(window)?)))
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
                let outer = self.pane_outer(pane)?;
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
                    let Some(outer) = self.pane_outer(pane) else {
                        continue;
                    };
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
                    let Some(outer) = self.pane_outer(pane) else {
                        continue;
                    };
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
                    let Some(outer) = self.pane_outer(pane) else {
                        continue;
                    };
                    present::clear(pane, outer, now, animation.duration, animation.easing);
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
                } => self.place(id, rect, animation, now),
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
    fn everything_is_off_stage(&self) -> Option<bool> {
        if self.groups.is_empty() {
            return None;
        }
        let screens: Vec<Rectangle<i32, Logical>> = self
            .space
            .outputs()
            .filter_map(|output| self.space.output_geometry(output))
            .collect();
        if screens.is_empty() {
            return None;
        }
        let now = self.clock.now();
        let mut any = false;
        for pane in self.panes.iter() {
            if !pane.managed() {
                continue;
            }
            let Some(outer) = self.pane_outer(pane) else {
                continue;
            };
            any = true;
            let drawn = self.drawn_at(pane, outer, now).rect;
            if screens.iter().any(|screen| screen.to_f64().overlaps(drawn)) {
                return Some(false);
            }
        }
        any.then_some(true)
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
        let Some(outer) = self.pane_outer(pane) else {
            return PaneHit::Miss;
        };
        let drawn = self.drawn_at(pane, outer, now);
        let in_outer = present::to_window_space(drawn, outer, location) - outer.loc.to_f64();

        // Only a *built* frame has a band to press. A pane reserving room for
        // one that has not arrived reports insets -- `insets_of` answers for
        // `Frame::Pending` on purpose, so the window does not change shape the
        // moment its frame appears -- but there is no titlebar there yet for a
        // click to land on, and there never was: this is the `decoration()?`
        // that gated `frame_under`.
        let framed = pane.decoration().is_some()
            && drawn.rect.contains(location)
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
        // What this pane is allowed to offer -- the `managed` and `window`
        // gates -- is `chrome_offered`'s, and both of them decline by answering
        // `None` here rather than by returning out of the function. That is the
        // #111-shaped difference: a loading window, and a client-placed menu,
        // both still cover what is behind them, and a press on either is its
        // own and nobody else's. Occluding is a fact about pixels; offering
        // chrome is a claim about what a press would do.
        let window = pane.client().cloned();
        let chrome = chrome_offered(
            pane.managed(),
            window.is_some(),
            framed,
            resize::border_edges(drawn_rect, location),
        );

        pane_hit_of(chrome, drawn.rect.contains(location)).map(|chrome| Under {
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
        self.space.map_element(window.clone(), client.loc, false);
    }

    /// Move and resize a window for real, gliding it there from where it was.
    ///
    /// This is the layout's authority: it changes the geometry everything else
    /// reads. The animation is a *transform* on top — the window is drawn from
    /// its old rectangle and lands on the new one — so a layout change and a
    /// mode use the same machinery and cannot disagree about where a window is
    /// going.
    fn place(&mut self, id: u64, rect: Rect, animation: AnimationSpec, now: Duration) {
        let Some(pane) = self.panes.by_script_id(id).map(Pane::id) else {
            return;
        };
        // Captured before anything moves: this is where the animation starts.
        let Some(was) = self.panes.get(pane).and_then(|held| self.pane_outer(held)) else {
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

        self.move_pane(pane, outer, was, animation, now);
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
    fn move_pane(
        &mut self,
        pane: crate::pane::PaneId,
        outer: Rectangle<i32, Logical>,
        was: Rectangle<i32, Logical>,
        animation: AnimationSpec,
        now: Duration,
    ) {
        // The frame's share comes off whichever sides it reserved; what is
        // left is the client's.
        let client = inner(outer, self.insets_of(pane));

        // A client is moved and resized for real, and the space is told,
        // because the space is the authority for a mapped window.
        if let Some(window) = self.panes.get(pane).and_then(Pane::client).cloned() {
            size_window(&window, client);
            // `false`: laying out must not restack. A tiling arrangement that
            // reordered windows every time it ran would fight the user's focus.
            self.space.map_element(window, client.loc, false);
        }
        // And the pane is told either way. For a mapped window this is what
        // `sync_panes` would write next frame anyway; for a pane whose
        // application has not arrived it is the whole of the move, because
        // there is nothing else holding its geometry.
        if let Some(held) = self.panes.get_mut(pane) {
            held.set_slot(client);
        }

        if let Some(held) = self.panes.get(pane) {
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
        // Locked, so there is no window under the pointer however many are
        // still mapped. Everything built on this -- click to focus, focus
        // follows mouse, drag, resize -- stops at once, in one place.
        if self.lock.is_some() {
            return None;
        }
        let now = self.clock.now();
        for pane in self.panes.iter().rev() {
            let Some(outer) = self.pane_outer(pane) else {
                continue;
            };
            if !self.drawn_at(pane, outer, now).rect.contains(location) {
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
            let Some(outer) = self.pane_outer(pane) else {
                continue;
            };
            let frame = self.drawn_at(pane, outer, now);
            if !frame.rect.contains(location) {
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
    /// chance that it might be the next to leave.
    pub(crate) fn close_pane(&mut self, id: crate::pane::PaneId) {
        // The pane is looked up before the "already leaving" guard rather than
        // after it, which the `closing` map could not do. Same answer either
        // way: an id with no pane returned early on the second check before
        // and returns early on the first one now, and a pane already on its
        // way out must not have its animation restarted.
        let Some(pane) = self.panes.get(id) else {
            return;
        };
        if pane.closing_at().is_some() {
            return;
        }
        let Some(outer) = self.pane_outer(pane) else {
            return;
        };
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
            // window was previously animated away and then asked *nothing*, so
            // it never closed and never came back. From the other side of the
            // screen that is a window that vanished.
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
    /// Returns whether anything was resized.
    pub(crate) fn settle_resize(&mut self) -> bool {
        let Some(request) = self.pending_resize.take() else {
            return false;
        };
        if !self.trigger_resize(&request) {
            self.resize_to(&request.window, request.wanted);
        }
        self.redraw = true;
        true
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
    pub(crate) fn settle_focus(&mut self) {
        if self.focused_window().is_some() {
            return;
        }
        let at = self
            .seat
            .get_pointer()
            .map(|pointer| pointer.current_location());
        let next = at
            .and_then(|at| self.window_under(at))
            .map(|(window, _)| window)
            .or_else(|| {
                self.panes
                    .iter()
                    .rev()
                    .find_map(|pane| pane.client().cloned())
            });
        if let Some(window) = next {
            tracing::debug!("a window went and the keyboard had nowhere to be");
            self.focus_window(&window, SERIAL_COUNTER.next_serial());
        }
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
    /// still being here a moment later. It comes back.
    ///
    /// Returns whether anything is still being waited on.
    pub(crate) fn settle_refused(&mut self, now: std::time::Duration) -> bool {
        /// Long enough that a client which is closing is not interrupted part
        /// way; short enough that coming back reads as an answer to the press
        /// rather than as a window reappearing by itself.
        const GRACE: std::time::Duration = std::time::Duration::from_millis(400);

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
            // Stopped waiting first, then acted on -- the same order the map
            // did it in, so a pane that goes while this runs is not waited on
            // for ever.
            if let Some(pane) = self.panes.get_mut(id) {
                pane.forget_asked();
            }
            let Some(pane) = self.panes.get(id) else {
                continue;
            };
            let Some(outer) = self.pane_outer(pane) else {
                continue;
            };
            tracing::debug!(
                pane = id.get(),
                "a window refused to close; bringing it back"
            );
            present::clear(
                pane,
                outer,
                now,
                std::time::Duration::from_millis(150),
                solium_animation::Curve::OutCubic,
            );
            self.redraw = true;
        }
        self.panes.iter().any(|pane| pane.asked_at().is_some())
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
    ///
    /// The frame's height comes out of the client's share, which is the same
    /// arithmetic as placement: a maximised window and its frame together fill
    /// the work area exactly.
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
        // The monitor this window is on, not the one the pointer is on: a
        // window maximised while you point at the other screen must fill its
        // own, and jumping across is the last thing a maximise should do.
        let Some(work_area) = self.work_area_of(current) else {
            return;
        };

        let insets = self.frame_insets(window);
        let restore = self
            .panes
            .get_mut(id)
            .and_then(Pane::decoration_mut)
            .map(|decoration| decoration.restore.take());

        let (location, size, maximized) = match restore {
            // Restoring: back to exactly where it was, because that rect was
            // stored rather than recomputed.
            Some(Some(previous)) => (previous.loc, previous.size, false),
            _ => (
                (work_area.loc.x + insets.left, work_area.loc.y + insets.top).into(),
                (
                    (work_area.size.w - insets.horizontal()).max(1),
                    (work_area.size.h - insets.vertical()).max(1),
                )
                    .into(),
                true,
            ),
        };

        if maximized && let Some(decoration) = self.panes.get_mut(id).and_then(Pane::decoration_mut)
        {
            decoration.restore = Some(current);
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
        self.space.map_element(window.clone(), location, true);
        tracing::debug!(maximized, "window maximise toggled");
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
            let outer = self.pane_outer(pane)?;
            let drawn = self.drawn_at(pane, outer, now);
            if !drawn.rect.contains(location) {
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

    /// Point the seat's selections at whoever holds focus.
    ///
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
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, Some(surface.clone()), SERIAL_COUNTER.next_serial());
            self.focus_selection(Some(&surface));
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
                    && self.pane_outer(pane).is_some_and(|outer| {
                        let drawn = self.drawn_at(pane, outer, now).rect;
                        self.space.outputs().any(|output| {
                            self.space
                                .output_geometry(output)
                                .is_some_and(|geometry| geometry.to_f64().overlaps(drawn))
                        })
                    })
            })
        })
    }

    /// Give the selection to nobody, for the moments where the keyboard has
    /// gone somewhere that is not a client -- or nowhere at all.
    pub(crate) fn clear_selection_focus(&mut self) {
        self.focus_selection(None);
    }

    /// A client may only read a selection while it holds the seat's data
    /// device focus, and that is a separate thing from keyboard focus. Set one
    /// and not the other and every paste hangs forever: the client asks for
    /// the selection and no offer ever arrives, which presents as a broken
    /// clipboard in the *pasting* application.
    fn focus_selection(&mut self, surface: Option<&WlSurface>) {
        let client = surface.and_then(|surface| self.display_handle.get_client(surface.id()).ok());
        set_data_device_focus(&self.display_handle, &self.seat, client.clone());
        set_primary_focus(&self.display_handle, &self.seat, client);
    }

    pub(crate) fn focus_window(&mut self, window: &Window, serial: Serial) {
        let Some(location) = self.space.element_location(window) else {
            return;
        };
        // Frames are drawn differently focused and unfocused, and restacking
        // changes what covers what. Both are the screen changing.
        self.redraw = true;
        // `true` restacks: a clicked window comes to the front.
        self.space.map_element(window.clone(), location, true);

        if let Some(keyboard) = self.seat.get_keyboard() {
            // The window's own surface, so this works for an X11 window as
            // well as an xdg one.
            let surface = window.wl_surface().map(|surface| surface.into_owned());
            keyboard.set_focus(self, surface.clone(), serial);
            self.focus_selection(surface.as_ref());
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
            self.space.map_element(window.clone(), slot.loc, false);
            return;
        }

        // Sized to fit before it is placed, because where a window goes
        // depends on how big it is.
        let size = self.fitted_size(window);
        let location = self.initial_placement(window, size);
        if size != window.geometry().size {
            size_window(window, Rectangle::new(location, size));
        }
        self.space.map_element(window.clone(), location, true);

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
                // And then look at what that produced. See
                // `everything_is_off_stage` for why this is asked here and
                // nowhere else -- briefly, a reload is both the keypress that
                // lost the desktop and the keypress anybody reaches for when
                // it is gone, so it is the one moment where the answer is
                // worth having whichever way it comes out.
                if self.everything_is_off_stage() == Some(true) {
                    tracing::warn!(
                        "after this reload every window is drawn outside every screen. If that \
                         is not simply a workspace with nothing on it, a selection is carrying \
                         the desktop off-stage and only something that names that selection can \
                         carry it back: switch workspace away and back again, which re-states \
                         where every desk sits"
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
    /// The delta is what the dragged edge moved by, which is what a layout can
    /// act on; the absolute rectangle would only be useful to something that
    /// already agreed the window has its own size.
    pub(crate) fn trigger_resize(&mut self, request: &ResizeRequest) -> bool {
        let id = self.window_id(&request.window);
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return false;
        };
        let outcome = scripts.resized(
            id,
            request.at,
            (request.horizontal, request.vertical),
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
        let screens: Vec<Rectangle<i32, Logical>> = self
            .space
            .outputs()
            .filter_map(|output| self.space.output_geometry(output))
            .collect();
        // No screens at all: every window is off-screen and there is nowhere
        // to put it. Leaving the slots alone means they are still where they
        // were when a monitor comes back, which is the best available answer.
        if screens.is_empty() {
            return;
        }

        let stranded: Vec<(crate::pane::PaneId, Rectangle<i32, Logical>)> = self
            .panes
            .iter()
            .filter_map(|pane| {
                let outer = self.pane_outer(pane)?;
                screens
                    .iter()
                    .all(|screen| !screen.overlaps(outer))
                    .then_some((pane.id(), outer))
            })
            .collect();

        for (pane, outer) in stranded {
            let centre = (
                f64::from(outer.loc.x) + f64::from(outer.size.w) / 2.0,
                f64::from(outer.loc.y) + f64::from(outer.size.h) / 2.0,
            );
            let Some(screen) = monitor::nearest(&self.space, centre.into())
                .and_then(|output| self.space.output_geometry(&output))
            else {
                continue;
            };
            // Onto the nearest screen, keeping its size, clamped so the whole
            // window is on it when it fits. Not centred: a window that was in
            // the top-left of the monitor that went should still feel like the
            // window that was in the top-left.
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
            let moved = Rectangle::new((x, y).into(), (size.0, size.1).into());
            // Through the same move every layout uses. Setting the slot alone
            // looks like it works and does not: the space still holds the old
            // position and writes it back the next frame.
            //
            // Animated from where it was, which is off screen -- so it flies
            // in from the edge the monitor was on rather than appearing. That
            // is worth the two lines: a window that teleports is one the user
            // has to find again.
            let now = self.clock.now();
            self.move_pane(pane, moved, outer, AnimationSpec::default(), now);
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
        let snapshot = self.snapshot();
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
        self.space.map_element(window.clone(), (0, 0), true);
        // On the same line as the map, so nothing can observe a mapped window
        // that has no pane -- `trigger_open` is about to ask for its id.
        self.adopt_or_open(window);

        // Focus follows the newest window. #12 turns this into a policy.
        if let Some(keyboard) = self.seat.get_keyboard() {
            let focused = surface.wl_surface().clone();
            keyboard.set_focus(self, Some(focused.clone()), SERIAL_COUNTER.next_serial());
            self.focus_selection(Some(&focused));
        }
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

        // A stale serial is a refusal, not a crash. `grab_popup` returns
        // `Err` for a popup that is already mapped, one whose parent was
        // dismissed, and one that is not the topmost — and posts the protocol
        // error itself where the protocol calls for one, so there is nothing
        // to do here but decline and say so.
        let mut grab = match self.popups.grab_popup(root, popup, &seat, serial) {
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

        if let Some(keyboard) = keyboard {
            // Keyboard before pointer, and the order matters. Installing the
            // pointer grab runs the *previous* pointer grab's `unset`, which
            // for a parent popup's `PopupPointerGrab` tries to take the
            // keyboard grab off again. It only does so if the keyboard grab's
            // serial is the parent's, so setting ours first is what makes a
            // submenu keep the keyboard instead of handing it back to the
            // window while its menu is still open.
            let focus = grab.current_grab();
            keyboard.set_focus(self, focus.clone(), serial);
            // Solium moves the selection focus with the keyboard focus
            // everywhere else it sets one — see `focus_window` — and a menu
            // opened from an unfocused window is exactly the case where the
            // two would otherwise part company: the popup would take the
            // keyboard while the clipboard still answered to whoever had it
            // before. Both are per-client, so for the ordinary case of a menu
            // in the already-focused window this changes nothing.
            self.focus_selection(focus.as_ref());
            keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        }
        if let Some(pointer) = pointer {
            // `Focus::Keep`, not `Focus::Clear` as Solium's move and resize
            // grabs use: those want the pointer to stop pointing at anything
            // for the duration, whereas a menu is being pointed *at* and must
            // keep receiving enter/motion so its entries highlight.
            pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
        }
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

        // Where to come back to, kept before anything moves. The same slot
        // `restore` holds for a maximised window, and for the same reason: a
        // rect that was stored is a rect that comes back exactly, where one
        // recomputed afterwards is a guess.
        if let Some(real) = self.real_geometry(&window)
            && let Some(decoration) = self.panes.get_mut(id).and_then(Pane::decoration_mut)
            && decoration.restore.is_none()
        {
            decoration.restore = Some(real);
        }

        // The whole monitor, and no frame over it. Marked bare rather than
        // having its decoration destroyed, so leaving fullscreen can build it
        // again from the style that is current then.
        //
        // Except that `remove` on the line below destroys the decoration the
        // two lines above just wrote `restore` into, so the rect never comes
        // back. Filed as #92, and left alone here: this commit moved where a
        // decoration lives, and lifting `restore` onto the pane would fix a
        // visible bug inside a change whose contract is that nothing changes.
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
        self.space.map_element(window, screen.loc, true);
        self.redraw = true;
        tracing::debug!(?screen, "a window went fullscreen");
    }

    /// And asking for it back.
    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        let Some(window) = self.window_for(surface.wl_surface()) else {
            return;
        };
        let Some(id) = self.panes.id_of(&window) else {
            return;
        };

        surface.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Fullscreen);
            state.size = None;
        });
        if surface.is_initial_configure_sent() {
            surface.send_pending_configure();
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

        if let Some(back) = self
            .panes
            .get_mut(id)
            .and_then(Pane::decoration_mut)
            .and_then(|decoration| decoration.restore.take())
        {
            surface.with_pending_state(|state| state.size = Some(back.size));
            surface.send_pending_configure();
            if let Some(pane) = self.panes.get_mut(id) {
                pane.set_slot(back);
            }
            self.space.map_element(window, back.loc, true);
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

delegate_compositor!(Solium);
delegate_shm!(Solium);
delegate_xdg_shell!(Solium);
delegate_xdg_decoration!(Solium);
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
            let covers = self.outer.to_f64().contains(location);
            let framed = covers
                && on_frame(
                    self.outer.size,
                    self.insets,
                    location - self.outer.loc.to_f64(),
                );
            pane_hit_of(
                chrome_offered(
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
    /// This needs a real client, not a bare `WlSurface`: `Window` only wraps
    /// a real `ToplevelSurface`, and Smithay gives no way to fabricate one
    /// except a client asking for it over the wire. `wl-probe` (see its own
    /// `Cargo.toml`) exists in this workspace for the identical reason -- some
    /// protocol claims can only be checked from the client's side -- and its
    /// dependencies are what make this affordable here: `wayland-client` and
    /// `wayland-protocols`'s `client` feature were already in the lockfile.
    mod scale_resend {
        use super::*;
        use smithay::output::{Mode, PhysicalProperties, Subpixel};
        use smithay::reexports::wayland_server::Display;
        use std::os::unix::io::{AsFd, OwnedFd};
        use std::os::unix::net::UnixStream;
        use wayland_client::protocol::{
            wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
        };
        use wayland_client::{Connection, Dispatch, QueueHandle};
        use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

        /// The client side of the fixture. Binds exactly the three globals a
        /// window needs and nothing else -- there is no renderer on this end
        /// to answer anything more, and none of what follows needs one.
        #[derive(Debug, Default)]
        struct Client {
            compositor: Option<wl_compositor::WlCompositor>,
            wm_base: Option<xdg_wm_base::XdgWmBase>,
            shm: Option<wl_shm::WlShm>,
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
                    _ => {}
                }
            }
        }

        wayland_client::delegate_noop!(Client: ignore wl_compositor::WlCompositor);
        wayland_client::delegate_noop!(Client: ignore wl_surface::WlSurface);
        wayland_client::delegate_noop!(Client: ignore wl_shm::WlShm);
        wayland_client::delegate_noop!(Client: ignore wl_shm_pool::WlShmPool);
        wayland_client::delegate_noop!(Client: ignore wl_buffer::WlBuffer);
        wayland_client::delegate_noop!(Client: ignore xdg_wm_base::XdgWmBase);
        wayland_client::delegate_noop!(Client: ignore xdg_surface::XdgSurface);
        wayland_client::delegate_noop!(Client: ignore xdg_toplevel::XdgToplevel);

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

        /// Opens one window through the real protocol: a surface, an
        /// `xdg_toplevel`, and a tiny committed buffer, so `new_toplevel` maps
        /// a `Window` with a real, non-zero bounding box at `(0, 0)` -- where
        /// every window is first mapped; the caller repositions it from
        /// there. Returns the newly-mapped `Window`.
        fn open_window(
            display: &mut Display<Solium>,
            state: &mut Solium,
            conn: &Connection,
            client: &Client,
            qh: &QueueHandle<Client>,
        ) -> Window {
            let compositor = client.compositor.clone().expect("wl_compositor bound");
            let wm_base = client.wm_base.clone().expect("xdg_wm_base bound");
            let shm = client.shm.clone().expect("wl_shm bound");

            let before: Vec<Window> = state.space.elements().cloned().collect();

            let surface = compositor.create_surface(qh, ());
            let xdg_surface = wm_base.get_xdg_surface(&surface, qh, ());
            let _toplevel = xdg_surface.get_toplevel(qh, ());

            const SIDE: i32 = 64;
            const STRIDE: i32 = SIDE * 4;
            let fd = anon_file(STRIDE * SIDE);
            let pool = shm.create_pool(fd.as_fd(), STRIDE * SIDE, qh, ());
            let buffer =
                pool.create_buffer(0, SIDE, SIDE, STRIDE, wl_shm::Format::Argb8888, qh, ());
            surface.attach(Some(&buffer), 0, 0);
            surface.commit();

            conn.flush().expect("flushing the window-open requests");
            display
                .dispatch_clients(state)
                .expect("dispatching the window-open requests");

            state
                .space
                .elements()
                .find(|window| !before.contains(window))
                .cloned()
                .expect("new_toplevel mapped a window")
        }

        /// The scenario issue #99 describes: two monitors, a `super+shift+r`
        /// rescale of one of them, and a window already open on each.
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
                .dispatch_clients(&mut state)
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

            let window_a = open_window(&mut display, &mut state, &conn, &client, &qh);
            let window_b = open_window(&mut display, &mut state, &conn, &client, &qh);

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
