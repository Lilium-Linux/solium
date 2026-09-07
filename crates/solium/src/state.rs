//! Compositor state and the Wayland protocol handlers.
//!
//! Smithay hands each protocol a state object and a handler trait; this module
//! owns both. Layout and presentation deliberately do not live here — see
//! `docs/architecture.md`.

use std::time::Duration;

use smithay::output::Output;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Serial, Size};
use smithay::wayland::fractional_scale::{
    FractionalScaleHandler, FractionalScaleManagerState, with_fractional_scale,
};
use smithay::wayland::pointer_constraints::{
    PointerConstraintsHandler, PointerConstraintsState, with_pointer_constraint,
};
use smithay::wayland::relative_pointer::RelativePointerManagerState;
use smithay::wayland::viewporter::ViewporterState;
use std::collections::HashMap;

use smithay::{
    backend::{allocator::dmabuf::Dmabuf, renderer::utils::on_commit_buffer_handler},
    delegate_compositor, delegate_data_device, delegate_dmabuf, delegate_layer_shell,
    delegate_output, delegate_seat, delegate_shm, delegate_xdg_decoration, delegate_xdg_shell,
    desktop::{LayerSurface, PopupManager, Space, Window, WindowSurfaceType, layer_map_for_output},
    input::{
        Seat, SeatHandler, SeatState,
        pointer::{CursorImageStatus, Focus, GrabStartData},
    },
    reexports::{
        wayland_protocols::xdg::{
            decoration::zv1::server::zxdg_toplevel_decoration_v1,
            shell::server::{xdg_toplevel, xdg_toplevel::ResizeEdge},
        },
        wayland_server::{
            Client, DisplayHandle,
            protocol::{wl_seat::WlSeat, wl_surface::WlSurface},
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
        selection::SelectionHandler,
        selection::data_device::{
            ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
            set_data_device_focus,
        },
        selection::primary_selection::{
            PrimarySelectionHandler, PrimarySelectionState, set_primary_focus,
        },
        shell::{
            wlr_layer::{
                Layer, LayerSurface as WlrLayerSurface, LayerSurfaceConfigure,
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
    layer,
    pane::Pane,
    present::{self, Clock, Frame},
    script::{AnimationSpec, Command, Outcome, Rect, Scripts, Snapshot, WindowInfo},
};

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
    #[expect(
        dead_code,
        reason = "registers the xdg-decoration global; dropping it would remove it"
    )]
    pub(crate) xdg_decoration_state: XdgDecorationState,
    /// The shell's way in: bars, docks, wallpapers and notification areas are
    /// ordinary clients that anchor to an output edge. See `layer.rs`.
    pub(crate) layer_shell_state: WlrLayerShellState,

    pub(crate) space: Space<Window>,

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
    /// The Developer Tweaks panel, when `--debug-mode` asked for one.
    pub(crate) tweaks: Option<crate::surface::ShellSurface>,
    /// Whether it is on screen. Hiding keeps the scene alive, so showing it
    /// again is a flag rather than a rebuild.
    pub(crate) tweaks_shown: bool,
    /// Which frame the pointer was last over, so the one it leaves can be
    /// told. QML hover is positional: a frame never told the pointer left
    /// stays lit forever.
    pub(crate) hovered_frame: Option<crate::pane::PaneId>,
    /// Windows on their way out, and when to tell them so. See `close_pane`.
    closing: HashMap<crate::pane::PaneId, std::time::Duration>,
    /// Windows that have been asked to close, and when they were asked.
    ///
    /// A close is a request. A client may put up "are you sure?" and stay, and
    /// nothing in the protocol says so -- the only evidence is the window
    /// still being here. See `settle_refused`.
    asked: HashMap<crate::pane::PaneId, std::time::Duration>,
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

    /// The middle-click clipboard. A separate selection with its own protocol,
    /// and its absence is not subtle: a terminal that pastes on middle click
    /// pastes nothing at all.
    pub(crate) primary_selection_state: PrimarySelectionState,

    /// Every decorated window's frame, drawn by us from QML.
    pub(crate) decorations: Decorations,

    /// What the pointer should look like right now.
    ///
    /// Nested, the host compositor drew the cursor and this could be ignored.
    /// On the hardware nothing else will draw it, so a compositor that does not
    /// track this has an invisible pointer — which is indistinguishable, to
    /// whoever is sitting there, from input being broken.
    pub(crate) pointer: crate::cursor::Pointer,

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
    pub(crate) shell: Option<crate::surface::ShellSurface>,

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
fn size_window(window: &Window, client: Rectangle<i32, Logical>) {
    if let Some(toplevel) = window.toplevel() {
        toplevel.with_pending_state(|state| state.size = Some(client.size));
        toplevel.send_pending_configure();
        return;
    }
    if let Some(x11) = window.x11_surface()
        && let Err(err) = x11.configure(Some(client))
    {
        tracing::warn!(?err, "could not size an X11 window");
    }
}

impl Solium {
    pub(crate) fn new(display_handle: DisplayHandle) -> Self {
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&display_handle, "winit");

        // Every capability is advertised on every form factor. Which of them a
        // machine actually has is a hardware question; how it behaves is the
        // profile's, and advertising a capability that never sends events is
        // cheaper than a client that cannot discover a device that appears.
        let _ = seat.add_keyboard(Default::default(), 200, 25);
        let _ = seat.add_pointer();
        let _ = seat.add_touch();

        Self {
            compositor_state: CompositorState::new::<Self>(&display_handle),
            xdg_shell_state: XdgShellState::new::<Self>(&display_handle),
            shm_state: ShmState::new::<Self>(&display_handle, Vec::new()),
            output_manager_state: OutputManagerState::new_with_xdg_output::<Self>(&display_handle),
            data_device_state: DataDeviceState::new::<Self>(&display_handle),
            loading: crate::script::Loading::default(),
            tweaks: None,
            tweaks_shown: true,
            hovered_frame: None,
            closing: HashMap::new(),
            asked: HashMap::new(),
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
            viewporter_state: ViewporterState::new::<Self>(&display_handle),
            fractional_scale_state: FractionalScaleManagerState::new::<Self>(&display_handle),
            xdg_decoration_state: XdgDecorationState::new::<Self>(&display_handle),
            layer_shell_state: WlrLayerShellState::new::<Self>(&display_handle),
            seat_state,
            space: Space::default(),
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
            published_windows: String::new(),
            shell: None,
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
    pub(crate) fn drawn(&self, id: crate::pane::PaneId, real: Rectangle<i32, Logical>) -> Frame {
        self.panes.get(id).map_or_else(
            || Frame::real(real),
            |pane| present::frame(pane, real, self.clock.now()),
        )
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
    pub(crate) fn is_decorated(&self, window: &Window) -> bool {
        self.panes
            .id_of(window)
            .is_some_and(|id| self.decorations.contains(id))
    }

    /// The output area windows may use.
    ///
    /// Whatever is left once every anchored surface has taken its exclusive
    /// zone — a number the *shell* chooses and may change at runtime, not a
    /// constant here. Every placement decision reads this rather than the raw
    /// output.
    pub(crate) fn work_area(&self) -> Option<Rectangle<i32, Logical>> {
        Some(layer::work_area(self.space.outputs().next()?))
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
    pub(crate) fn focused_window(&self) -> Option<Window> {
        let surface = self.seat.get_keyboard()?.current_focus()?;
        self.window_for(&surface)
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

        // Everything keyed by a pane goes when the pane does. Keyed by surface
        // this could not have happened here, because nothing knew the set of
        // live windows -- so it was done where a window was seen leaving
        // tidily, and a client that crashed left its frame behind forever.
        let live: std::collections::HashSet<crate::pane::PaneId> =
            self.panes.iter().map(Pane::id).collect();
        self.decorations.retain(|id| live.contains(&id));
        self.closing.retain(|id, _| live.contains(id));
        self.asked.retain(|id, _| live.contains(id));

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
                let outer = self.pane_outer(pane)?;
                let drawn = present::frame(pane, outer, now);
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
                })
            })
            .collect();

        Snapshot {
            windows,
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
                        deform,
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
                    if self.decorations.set_style(name) {
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
                Command::TweaksToggle => {
                    self.tweaks_shown = !self.tweaks_shown;
                    self.redraw = true;
                }
                Command::Spawn { program, args } => self.spawn(&program, &args),
                Command::Reload => self.request = Some(Request::Reload),
                Command::Quit => {
                    tracing::info!("a script asked to stop");
                    self.request = Some(Request::Quit);
                }
            }
        }
    }

    /// The window and edges a press at `location` would resize, if any.
    ///
    /// Searched topmost first, and only over the edges: the middle of a window
    /// belongs to the client.
    pub(crate) fn resize_target(
        &self,
        location: Point<f64, Logical>,
    ) -> Option<(Window, ResizeEdge, Rectangle<i32, Logical>)> {
        let now = self.clock.now();

        self.panes.iter().rev().find_map(|pane| {
            let window = pane.client()?;
            let outer = self.outer_geometry(window)?;
            // Against where the window is *drawn*: a window in a mode should be
            // resized by its thumbnail's edge or not at all, never by an edge
            // that is somewhere else on screen.
            let drawn = present::frame(pane, outer, now).rect;
            let grown = Rectangle::new(
                (
                    drawn.loc.x.round() as i32 - resize::RESIZE_BORDER,
                    drawn.loc.y.round() as i32 - resize::RESIZE_BORDER,
                )
                    .into(),
                (
                    drawn.size.w.round() as i32 + resize::RESIZE_BORDER * 2,
                    drawn.size.h.round() as i32 + resize::RESIZE_BORDER * 2,
                )
                    .into(),
            );
            if !grown.to_f64().contains(location) {
                return None;
            }

            #[expect(
                clippy::cast_possible_truncation,
                reason = "a drawn rect is screen-sized"
            )]
            let drawn_rect = Rectangle::new(
                (drawn.loc.x.round() as i32, drawn.loc.y.round() as i32).into(),
                (drawn.size.w.round() as i32, drawn.size.h.round() as i32).into(),
            );
            match resize::edges_at(drawn_rect, location) {
                ResizeEdge::None => None,
                edges => Some((window.clone(), edges, outer)),
            }
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
    pub(crate) fn window_under(
        &self,
        location: Point<f64, Logical>,
    ) -> Option<(Window, Rectangle<i32, Logical>)> {
        let now = self.clock.now();
        self.panes.iter().rev().find_map(|pane| {
            let window = pane.client()?;
            let outer = self.outer_geometry(window)?;
            if !present::frame(pane, outer, now).rect.contains(location) {
                return None;
            }
            Some((window.clone(), self.real_geometry(window)?))
        })
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
        if let Some(output) = self.space.outputs().next()
            && let Some(found) = layer::surface_under(output, location)
        {
            return Some(found);
        }

        let now = self.clock.now();

        for pane in self.panes.iter().rev() {
            let Some(outer) = self.pane_outer(pane) else {
                continue;
            };
            let frame = present::frame(pane, outer, now);
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

            if let Some((surface, surface_offset)) =
                window.surface_under(in_window, WindowSurfaceType::ALL)
            {
                let in_surface = in_window - surface_offset.to_f64();
                return Some((surface, location - in_surface));
            }
        }

        None
    }

    /// The frame at a point, with the point in the frame's own coordinates.
    ///
    /// Frame-local rather than compositor coordinates because the frame is
    /// rasterised at its unscaled size: a titlebar drawn at two-thirds size in
    /// overview must still be hit-tested against the QML that was drawn at full
    /// size, or its buttons move out from under the cursor.
    pub(crate) fn frame_under(
        &self,
        location: Point<f64, Logical>,
    ) -> Option<(crate::pane::PaneId, Option<Window>, Point<f64, Logical>)> {
        let now = self.clock.now();

        self.panes.iter().rev().find_map(|pane| {
            if !self.decorations.contains(pane.id()) {
                return None;
            }
            let outer = self.pane_outer(pane)?;
            let drawn = present::frame(pane, outer, now);
            if !drawn.rect.contains(location) {
                return None;
            }

            let in_outer = present::to_window_space(drawn, outer, location) - outer.loc.to_f64();
            let insets = self.insets_of(pane.id());
            // The frame is the band between the outer rect and the client: a
            // point inside the client is the client's, wherever the frame put
            // its bar. A decoration that reserves nothing owns no band at all,
            // and its clicks belong to the window under it.
            let client = Rectangle::new(
                (f64::from(insets.left), f64::from(insets.top)).into(),
                (
                    f64::from(outer.size.w - insets.horizontal()),
                    f64::from(outer.size.h - insets.vertical()),
                )
                    .into(),
            );
            if client.contains(in_outer) {
                return None;
            }
            Some((pane.id(), pane.client().cloned(), in_outer))
        })
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
        if self.closing.contains_key(&id) {
            return;
        }
        let Some(pane) = self.panes.get(id) else {
            return;
        };
        let Some(outer) = self.pane_outer(pane) else {
            return;
        };
        let now = self.clock.now();
        present::close(pane, outer, now);
        self.closing.insert(id, now + present::CLOSING);
        self.redraw = true;
    }

    /// Send the close to every window whose leaving animation has landed.
    ///
    /// Returns whether any window is still on its way out, so the backend
    /// keeps drawing until they are gone.
    pub(crate) fn settle_closing(&mut self, now: std::time::Duration) -> bool {
        if self.closing.is_empty() {
            return false;
        }
        let due: Vec<crate::pane::PaneId> = self
            .closing
            .iter()
            .filter(|(_, at)| now >= **at)
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            self.closing.remove(&id);
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
            self.asked.insert(id, now);
        }
        !self.closing.is_empty()
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
    fn settle_focus(&mut self) {
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
        if self.asked.is_empty() {
            return false;
        }
        /// Long enough that a client which is closing is not interrupted part
        /// way; short enough that coming back reads as an answer to the press
        /// rather than as a window reappearing by itself.
        const GRACE: std::time::Duration = std::time::Duration::from_millis(400);

        let due: Vec<crate::pane::PaneId> = self
            .asked
            .iter()
            .filter(|(_, at)| now.saturating_sub(**at) >= GRACE)
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            self.asked.remove(&id);
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
        !self.asked.is_empty()
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
        let Some(work_area) = self.work_area() else {
            return;
        };
        let Some(current) = self.real_geometry(window) else {
            return;
        };

        let insets = self.frame_insets(window);
        let restore = self
            .decorations
            .get_mut(id)
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

        if maximized && let Some(decoration) = self.decorations.get_mut(id) {
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
    pub(crate) fn insets_of(&self, id: crate::pane::PaneId) -> Insets {
        self.decorations.get(id).map_or(
            Insets {
                top: TITLEBAR_HEIGHT,
                ..Insets::NONE
            },
            super::decoration::Decoration::insets,
        )
    }

    /// Raise a window and give it the keyboard.
    /// Report what the compositor is holding, once a second, when asked.
    ///
    /// Enabled with `SOLIUM_MEMDIAG=1`. A leak hunt needs to know *which*
    /// number is growing: resident memory alone cannot tell a forgotten
    /// decoration from a Lua heap that never shrinks from an allocator that
    /// simply keeps what it has.
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
            decorations = self.decorations.len(),
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
    /// Wider than `frame_under` on purpose: a decoration that glows where the
    /// cursor is has to be told about the cursor while it is over the client,
    /// which is the client's surface and reports nothing to us. Ownership of
    /// clicks is still decided by `frame_under`; this is only for looking.
    pub(crate) fn decorated_under(
        &self,
        location: Point<f64, Logical>,
    ) -> Option<(crate::pane::PaneId, Point<f64, Logical>)> {
        let now = self.clock.now();
        self.panes.iter().rev().find_map(|pane| {
            if !self.decorations.contains(pane.id()) {
                return None;
            }
            let outer = self.pane_outer(pane)?;
            let drawn = present::frame(pane, outer, now);
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
        self.decorations.insert(id, area.size.w, area.size.h);
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
            // around a window that is already gone. Everything else keyed by
            // the pane goes with it at the next `sync_panes`.
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
    pub(crate) fn reload(&mut self) {
        let path = Scripts::config_path();
        match Scripts::load(&path) {
            Ok(scripts) => {
                crate::qml::clear_cache();
                let style = self.decorations.style().map(ToOwned::to_owned);
                self.decorations.set_style(None);
                self.decorations.set_style(style);
                self.start_scripts(Some(scripts));
                self.redraw = true;
                tracing::info!(config = %path.display(), "configuration reloaded");
            }
            Err(err) => {
                tracing::error!(?err, config = %path.display(), "reload failed, keeping what was running");
            }
        }
    }

    /// Take the scripts, and act on whatever they asked for while loading.
    pub(crate) fn start_scripts(&mut self, scripts: Option<Scripts>) {
        let Some(mut scripts) = scripts else {
            self.scripts = None;
            return;
        };
        let outcome = scripts.startup();
        self.scripts = Some(scripts);
        self.apply(outcome);
    }

    /// The shell surface, built on first use from `SOLIUM_SHELL_SCENE`.
    pub(crate) fn shell(&mut self) -> Option<&mut crate::surface::ShellSurface> {
        if self.shell.is_none() {
            let scene = std::env::var_os("SOLIUM_SHELL_SCENE")?;
            let area = self.work_area()?;
            let name = self
                .space
                .outputs()
                .next()
                .map(smithay::output::Output::name)
                .unwrap_or_default();
            // What shell components ask for about the screen they are on.
            let properties = format!(
                "{{\"screenInfo\":{{\"name\":\"{name}\",\"x\":{},\"y\":{},\"width\":{},\"height\":{},\"scale\":1}}}}",
                area.loc.x, area.loc.y, area.size.w, area.size.h
            );
            match crate::surface::ShellSurface::new(scene.into(), &properties) {
                Ok(surface) => self.shell = Some(surface),
                Err(err) => {
                    tracing::error!(?err, "the shell scene would not load");
                    return None;
                }
            }
        }
        self.shell.as_mut()
    }

    /// The Developer Tweaks panel, built on first use.
    ///
    /// A second shell surface rather than anything new: it is QML hosted in
    /// the compositor, which is a thing that already exists here. What it
    /// offers comes from the scripts, so the panel is a list of whatever
    /// `tweaks.lua` declares.
    pub(crate) fn tweaks_panel(&mut self) -> Option<&mut crate::surface::ShellSurface> {
        if !crate::dev::debug_mode() {
            return None;
        }
        if self.tweaks.is_none() {
            let entries = self
                .scripts
                .as_ref()
                .and_then(super::script::Scripts::tweaks)
                .unwrap_or_else(|| "[]".to_owned());
            let source =
                std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/qml/tweaks.qml"));
            let properties = format!("{{\"entries\":{entries}}}");
            match crate::surface::ShellSurface::new(source, &properties) {
                Ok(panel) => self.tweaks = Some(panel),
                Err(err) => {
                    tracing::error!(?err, "the Developer Tweaks panel would not load");
                    return None;
                }
            }
        }
        self.tweaks.as_mut()
    }

    /// Where the panel sits, or `None` when there is not one to show.
    ///
    /// The single gate: drawing and input both ask for the area, so hidden is
    /// hidden for both without either of them knowing why.
    pub(crate) fn tweaks_area(&self) -> Option<Rectangle<i32, Logical>> {
        if !self.tweaks_shown || !crate::dev::debug_mode() {
            return None;
        }
        let area = self.work_area()?;
        let width = 320.min((area.size.w / 3).max(200));
        Some(Rectangle::new(
            (area.loc.x + area.size.w - width, area.loc.y).into(),
            (width, area.size.h).into(),
        ))
    }

    /// Act on whatever the panel was pressed for.
    pub(crate) fn settle_tweaks(&mut self) {
        let Some(panel) = self.tweaks.as_mut() else {
            return;
        };
        let Some(id) = panel.taken_action() else {
            return;
        };
        let Some(mut scripts) = self.scripts.take() else {
            return;
        };
        let outcome = scripts.tweak(&id, self.snapshot());
        self.scripts = Some(scripts);
        self.apply(outcome);
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
        if self.shell.is_none() {
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
        let Some(area) = self.work_area() else {
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

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        // Tracking failure here is not fatal: the popup simply will not be
        // positioned, which is better than ending the session.
        if let Err(err) = self.popups.track_popup(surface.into()) {
            tracing::warn!(?err, "failed to track popup");
        }
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: WlSeat, _serial: Serial) {}

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

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        surface.send_repositioned(token);
    }
}

impl Solium {
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
        // A surface may name an output or leave the choice to us. With one
        // output the distinction does not bite yet, but honouring the request
        // now means a shell written against Solium is not written against a
        // simplification.
        let output = wl_output
            .as_ref()
            .and_then(Output::from_resource)
            .or_else(|| self.space.outputs().next().cloned());
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
        tracing::info!(namespace, "layer surface mapped");
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
            self.decorations.insert(id, width, height);
        } else {
            self.decorations.remove(id);
        }

        // The client has to learn its mode before it draws, or it decides for
        // itself and draws a frame we then draw over.
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
        tracing::debug!(server_side, "decoration mode agreed");
    }
}

impl FractionalScaleHandler for Solium {
    /// A client has asked what scale it is really drawn at.
    ///
    /// Answered from the output it is on rather than from a constant, so the
    /// answer stays right the day an output is not 1x. A surface not on any
    /// output yet is told the first output's scale, which is the one it is
    /// about to be on.
    fn new_fractional_scale(
        &mut self,
        surface: smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) {
        let scale = self
            .window_for(&surface)
            .and_then(|window| self.space.outputs_for_element(&window).first().cloned())
            .or_else(|| self.space.outputs().next().cloned())
            .map_or(1.0, |output| output.current_scale().fractional_scale());
        with_states(&surface, |states| {
            with_fractional_scale(states, |fractional| {
                fractional.set_preferred_scale(scale);
            });
        });
    }
}
smithay::delegate_fractional_scale!(Solium);
smithay::delegate_viewporter!(Solium);

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

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        // A client sets its own cursor when the pointer is over it — an I-beam
        // over text, a resize arrow on an edge. Dropping this on the floor
        // leaves every application with our arrow.
        self.pointer.status = image;
    }
    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&WlSurface>) {}
}

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

impl ClientDndGrabHandler for Solium {}
impl ServerDndGrabHandler for Solium {}

delegate_compositor!(Solium);
delegate_shm!(Solium);
delegate_xdg_shell!(Solium);
delegate_xdg_decoration!(Solium);
delegate_layer_shell!(Solium);
delegate_seat!(Solium);
delegate_output!(Solium);
delegate_data_device!(Solium);
smithay::delegate_primary_selection!(Solium);
smithay::delegate_xwayland_shell!(Solium);
