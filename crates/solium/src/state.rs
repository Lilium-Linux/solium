//! Compositor state and the Wayland protocol handlers.
//!
//! Smithay hands each protocol a state object and a handler trait; this module
//! owns both. Layout and presentation deliberately do not live here — see
//! `docs/architecture.md`.

use std::time::Duration;

use smithay::output::Output;
use smithay::reexports::wayland_server::{Resource, backend::ObjectId};
use smithay::utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Serial};
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
    decoration::{Action, Decorations, TITLEBAR_HEIGHT},
    input::{grab::MoveGrab, profile::Profile, resize},
    layer,
    present::{self, Clock, Frame},
    script::{AnimationSpec, Command, Outcome, Rect, Scripts, Snapshot, WindowInfo},
};

/// A window's script-facing identity.
///
/// Stable for the window's lifetime and never reused, so a script that holds an
/// id across frames can only ever address the window it meant — an index into
/// the window list would silently come to mean a different window.
#[derive(Debug)]
struct WindowId(u64);

pub(crate) fn window_id(window: &Window) -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);

    window
        .user_data()
        .insert_if_missing(|| WindowId(NEXT.fetch_add(1, Ordering::Relaxed)));
    window.user_data().get::<WindowId>().map_or(0, |id| id.0)
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

    /// Something only a backend can carry out: switching VT, or stopping.
    ///
    /// The input layer must not do either itself. It runs inside the keyboard
    /// filter, holding the seat's lock, and it is shared with the nested
    /// backend where neither action means anything.
    pub(crate) request: Option<Request>,
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
            xdg_decoration_state: XdgDecorationState::new::<Self>(&display_handle),
            layer_shell_state: WlrLayerShellState::new::<Self>(&display_handle),
            seat_state,
            space: Space::default(),
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
            pending_drop: None,
            redraw: true,
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
        let real = self.real_geometry(window)?;
        if !self.is_decorated(window) {
            return Some(real);
        }
        Some(Rectangle::new(
            (real.loc.x, real.loc.y - TITLEBAR_HEIGHT).into(),
            (real.size.w, real.size.h + TITLEBAR_HEIGHT).into(),
        ))
    }

    /// Whether the compositor draws this window's frame.
    pub(crate) fn is_decorated(&self, window: &Window) -> bool {
        self.toplevel_id(window)
            .is_some_and(|id| self.decorations.contains(&id))
    }

    /// A window's toplevel surface id, the key frames are stored under.
    pub(crate) fn toplevel_id(&self, window: &Window) -> Option<ObjectId> {
        window.toplevel().map(|toplevel| toplevel.wl_surface().id())
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
        let windows = self
            .space
            .elements()
            .rev()
            .filter_map(|window| {
                let outer = self.outer_geometry(window)?;
                let drawn = present::frame(window, outer, now);
                Some(WindowInfo {
                    id: window_id(window),
                    rect: to_rect(outer),
                    drawn: Rect {
                        x: drawn.rect.loc.x,
                        y: drawn.rect.loc.y,
                        w: drawn.rect.size.w,
                        h: drawn.rect.size.h,
                    },
                    title: self.window_title(window),
                    focused: focused.as_ref() == Some(window),
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
                    animation,
                } => {
                    let Some(window) = self.window_by_id(id) else {
                        continue;
                    };
                    let Some(outer) = self.outer_geometry(&window) else {
                        continue;
                    };
                    let target = Frame {
                        rect: rect.map_or_else(
                            || outer.to_f64(),
                            |rect| present::logical((rect.x, rect.y), (rect.w, rect.h)),
                        ),
                        opacity: opacity.unwrap_or(1.0),
                    };
                    present::present(
                        &window,
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
                    let Some(window) = self.window_by_id(id) else {
                        continue;
                    };
                    let Some(outer) = self.outer_geometry(&window) else {
                        continue;
                    };
                    let start = Frame {
                        rect: present::logical((rect.x, rect.y), (rect.w, rect.h)),
                        opacity: opacity.unwrap_or(1.0),
                    };
                    present::from(
                        &window,
                        outer,
                        start,
                        now,
                        animation.duration,
                        animation.easing,
                    );
                }
                Command::Clear { id, animation } => {
                    let Some(window) = self.window_by_id(id) else {
                        continue;
                    };
                    let Some(outer) = self.outer_geometry(&window) else {
                        continue;
                    };
                    present::clear(&window, outer, now, animation.duration, animation.easing);
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
                    if let Some(toplevel) = self
                        .window_by_id(id)
                        .and_then(|window| window.toplevel().cloned())
                    {
                        toplevel.send_close();
                    }
                }
                Command::Spawn { program, args } => self.spawn(&program, &args),
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

        self.space.elements().rev().find_map(|window| {
            let outer = self.outer_geometry(window)?;
            // Against where the window is *drawn*: a window in a mode should be
            // resized by its thumbnail's edge or not at all, never by an edge
            // that is somewhere else on screen.
            let drawn = present::frame(window, outer, now).rect;
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
        let inset = self.frame_inset(window);
        let client = Rectangle::new(
            (outer.loc.x, outer.loc.y + inset).into(),
            (outer.size.w, (outer.size.h - inset).max(1)).into(),
        );

        if let Some(toplevel) = window.toplevel() {
            toplevel.with_pending_state(|state| state.size = Some(client.size));
            toplevel.send_pending_configure();
        }
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
        let Some(window) = self.window_by_id(id) else {
            return;
        };
        // Captured before anything moves: this is where the animation starts.
        let Some(was) = self.outer_geometry(&window) else {
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

        // The frame's share comes off the top; what is left is the client's.
        let inset = self.frame_inset(&window);
        let client = Rectangle::new(
            (outer.loc.x, outer.loc.y + inset).into(),
            (outer.size.w, (outer.size.h - inset).max(1)).into(),
        );

        if let Some(toplevel) = window.toplevel() {
            toplevel.with_pending_state(|state| state.size = Some(client.size));
            toplevel.send_pending_configure();
        }
        // `false`: laying out must not restack. A tiling arrangement that
        // reordered windows every time it ran would fight the user's focus.
        self.space.map_element(window.clone(), client.loc, false);

        present::from(
            &window,
            outer,
            present::Frame::real(was),
            now,
            animation.duration,
            animation.easing,
        );
    }

    /// Start a program as a client of this compositor.
    fn spawn(&self, program: &str, args: &[String]) {
        use std::process::{Command as Process, Stdio};

        let mut process = Process::new(program);
        process
            .args(args)
            // Without this the child inherits the *host* display and opens its
            // window next to the compositor rather than inside it.
            .env("WAYLAND_DISPLAY", &self.socket_name)
            // This compositor has no X server, so a child must not believe it
            // has one. Inheriting DISPLAY is worse than it sounds: a Qt or GTK
            // program prefers X11 when it is set, connects to the *host's*
            // XWayland, and opens its window on the host desktop. The spawn
            // logs success, nothing errors, and no window ever appears here.
            .env_remove("DISPLAY")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            // Errors are inherited, not discarded. A program that refuses to
            // start says why on stderr, and swallowing that leaves "the
            // binding does nothing" as the only symptom of a dozen different
            // causes.
            .stderr(Stdio::inherit());

        match process.spawn() {
            Ok(mut child) => {
                tracing::info!(program, socket = self.socket_name, "spawned");
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
            Err(err) => tracing::warn!(?err, program, "could not spawn"),
        }
    }

    /// The window a script means by an id.
    ///
    /// Ids that no longer exist are simply not found — a window closing while a
    /// mode holds its id is ordinary, not an error.
    fn window_by_id(&self, id: u64) -> Option<Window> {
        self.space
            .elements()
            .find(|window| window_id(window) == id)
            .cloned()
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
        self.space.elements().rev().find_map(|window| {
            let outer = self.outer_geometry(window)?;
            if !present::frame(window, outer, now).rect.contains(location) {
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

        for window in self.space.elements().rev() {
            let Some(outer) = self.outer_geometry(window) else {
                continue;
            };
            let frame = present::frame(window, outer, now);
            if !frame.rect.contains(location) {
                continue;
            }

            // Mapped through the *outer* rect, then offset into the client's
            // own space. A point in the titlebar lands above the client and
            // finds no surface, which is what should happen: the frame is the
            // compositor's, not the client's.
            let inset: Point<f64, Logical> = (0.0, f64::from(self.frame_inset(window))).into();
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
    ) -> Option<(Window, Point<f64, Logical>)> {
        let now = self.clock.now();

        self.space.elements().rev().find_map(|window| {
            if !self.is_decorated(window) {
                return None;
            }
            let outer = self.outer_geometry(window)?;
            let drawn = present::frame(window, outer, now);
            if !drawn.rect.contains(location) {
                return None;
            }

            let in_outer = present::to_window_space(drawn, outer, location) - outer.loc.to_f64();
            if in_outer.y >= f64::from(TITLEBAR_HEIGHT) {
                // Below the frame: the client's, not ours.
                return None;
            }
            Some((window.clone(), in_outer))
        })
    }

    /// Act on a frame button.
    pub(crate) fn frame_action(&mut self, window: &Window, action: Action) {
        match action {
            Action::Close => {
                if let Some(toplevel) = window.toplevel() {
                    // A request, not a kill: the client decides whether it can
                    // close, and the window goes away when it does.
                    toplevel.send_close();
                }
            }
            Action::ToggleMaximize => self.toggle_maximize(window),
        }
    }

    /// Fill the work area, or go back to where the window was.
    ///
    /// The frame's height comes out of the client's share, which is the same
    /// arithmetic as placement: a maximised window and its frame together fill
    /// the work area exactly.
    fn toggle_maximize(&mut self, window: &Window) {
        let Some(id) = self.toplevel_id(window) else {
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

        let inset = self.frame_inset(window);
        let restore = self
            .decorations
            .get_mut(&id)
            .map(|decoration| decoration.restore.take());

        let (location, size, maximized) = match restore {
            // Restoring: back to exactly where it was, because that rect was
            // stored rather than recomputed.
            Some(Some(previous)) => (previous.loc, previous.size, false),
            _ => (
                (work_area.loc.x, work_area.loc.y + inset).into(),
                (work_area.size.w, (work_area.size.h - inset).max(1)).into(),
                true,
            ),
        };

        if maximized && let Some(decoration) = self.decorations.get_mut(&id) {
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
    pub(crate) fn frame_inset(&self, window: &Window) -> i32 {
        if self.is_decorated(window) {
            TITLEBAR_HEIGHT
        } else {
            0
        }
    }

    /// Raise a window and give it the keyboard.
    pub(crate) fn focus_window(&mut self, window: &Window, serial: Serial) {
        let Some(location) = self.space.element_location(window) else {
            return;
        };
        // `true` restacks: a clicked window comes to the front.
        self.space.map_element(window.clone(), location, true);

        if let Some(keyboard) = self.seat.get_keyboard() {
            let surface = window
                .toplevel()
                .map(|toplevel| toplevel.wl_surface().clone());
            keyboard.set_focus(self, surface, serial);
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

        if !present::mark_shown(window) {
            return;
        }

        let location = self.initial_placement(window);
        self.space.map_element(window.clone(), location, true);

        // How a window appears is a script's decision — that is what makes the
        // dock-icon genie a script rather than a feature. The built-in is only
        // a fallback for when nothing has an opinion; a window popping into
        // existence with no animation at all is worse than a plain one.
        if !self.trigger_open(window)
            && let Some(outer) = self.outer_geometry(window)
        {
            present::open(window, outer, self.clock.now());
        }
    }

    /// Offer a newly shown window to whatever script wants to animate it in.
    /// Tell scripts a window has gone, so a layout can forget it.
    pub(crate) fn trigger_close(&mut self, window: &Window) {
        let id = window_id(window);
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
        let id = window_id(window);
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

    fn trigger_open(&mut self, window: &Window) -> bool {
        let id = window_id(window);
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
    fn initial_placement(&self, window: &Window) -> Point<i32, Logical> {
        let Some(output) = self.work_area() else {
            return (0, 0).into();
        };
        let size = window.geometry().size;

        const CASCADE: i32 = 44;
        const WRAP: usize = 6;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_possible_wrap,
            reason = "the index is taken modulo a small constant"
        )]
        let step = CASCADE * (self.space.elements().count() % WRAP) as i32;

        // The frame is above the client, so the client's own top edge starts
        // that far down: the pair has to fit in the work area, not just the
        // client.
        let inset = self.frame_inset(window);
        let outer_height = size.h + inset;

        let centred = |available: i32, window: i32| (available - window) / 2;
        let x = output.loc.x + centred(output.size.w, size.w).max(0) + step;
        let y = output.loc.y + centred(output.size.h, outer_height).max(0) + step + inset;

        // Kept on the output even if the cascade would walk a large window off
        // the bottom right.
        (
            x.min(output.loc.x + (output.size.w - size.w).max(0)),
            y.max(output.loc.y + inset)
                .min(output.loc.y + (output.size.h - outer_height).max(0) + inset),
        )
            .into()
    }

    /// The window owning a surface, if any.
    fn window_for(&self, surface: &WlSurface) -> Option<Window> {
        self.space
            .elements()
            .find(|window| window.toplevel().map(ToplevelSurface::wl_surface) == Some(surface))
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
        self.space.map_element(window, (0, 0), true);

        // Focus follows the newest window. #12 turns this into a policy.
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(
                self,
                Some(surface.wl_surface().clone()),
                SERIAL_COUNTER.next_serial(),
            );
        }
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        // Told before the window is forgotten, so a script can still ask which
        // one it was.
        // Bound before the call, so the borrow of `space` ends here rather
        // than lasting across it.
        let going = self
            .space
            .elements()
            .find(|window| window.toplevel().is_some_and(|top| *top == surface))
            .cloned();
        if let Some(window) = going {
            self.trigger_close(&window);
        }

        // The frame is dropped with the window it belongs to. Keyed by surface
        // id rather than kept on the window so that this is the only place it
        // has to happen.
        self.decorations.remove(&surface.wl_surface().id());
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
        let id = toplevel.wl_surface().id();

        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(if server_side {
                Mode::ServerSide
            } else {
                Mode::ClientSide
            });
        });

        if server_side {
            let width = self
                .window_for(toplevel.wl_surface())
                .and_then(|window| self.real_geometry(&window))
                .map_or(TITLEBAR_HEIGHT * 20, |real| real.size.w);
            self.decorations.insert(id, width);
        } else {
            self.decorations.remove(&id);
        }

        // The client has to learn its mode before it draws, or it decides for
        // itself and draws a frame we then draw over.
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
        tracing::debug!(server_side, "decoration mode agreed");
    }
}

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
