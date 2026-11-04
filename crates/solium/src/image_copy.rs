//! `ext-image-copy-capture-v1`: handing a monitor's picture to a client, the
//! newer way.
//!
//! The successor to `wlr-screencopy` (`screencopy.rs`), offered beside it.
//! A client names what it wants as an `ext_image_capture_source_v1` -- a
//! monitor, through `ext_output_image_capture_source_manager_v1`, or one
//! window, through `ext_foreign_toplevel_image_capture_source_manager_v1`
//! -- opens a session on it, is told what buffer to allocate, and then asks
//! for frames into that buffer one after another until it is done.
//!
//! ## Why it is offered
//!
//! Screen sharing. `xdg-desktop-portal-wlr` uses this protocol when the
//! compositor offers it and `wlr-screencopy` otherwise, and its
//! `wlr-screencopy` path in 0.8.3 and 0.8.4 delivers one frame and then never
//! asks for another: it drives its PipeWire stream itself and only restarts
//! the stream's cycle from this protocol's path (#387). `grim` 1.5 prefers it
//! as well.
//!
//! ## What is shared, and what is not
//!
//! Everything below the protocol. A capture asked for here becomes the same
//! [`Capture`] a `wlr-screencopy` frame does, is queued on the same
//! `Solium::pending_captures`, and is filled by the same `screencopy::settle`
//! from the same render pass. This file only asks and answers.
//!
//! ## One window
//!
//! A window is named by its handle on `ext-foreign-toplevel-list-v1`
//! (`toplevels.rs`), and its picture is the window alone, drawn at its
//! monitor's scale whatever covers it or wherever it is (#47):
//! `toplevels.capture` in `config.lua` says whether that is the client as it
//! drew itself (`"client"`, the default) or the pane as it is on screen
//! (`"pane"`), and `false` stops offering windows at all. A session is told
//! the window's size again whenever it changes, every pass
//! ([`Solium::follow_window_captures`]), and stops when the window goes or
//! the setting takes windows away.
//!
//! A pointer cursor session is accepted, and the capture session it hands
//! out says `stopped` at once: the cursor is in the picture when a client
//! asks for `paint_cursors`, and a separate stream of it is something
//! nothing here draws.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use smithay::{
    output::Output,
    reexports::{
        wayland_protocols::ext::{
            image_capture_source::v1::server::{
                ext_foreign_toplevel_image_capture_source_manager_v1::{
                    self, ExtForeignToplevelImageCaptureSourceManagerV1,
                },
                ext_image_capture_source_v1::{self, ExtImageCaptureSourceV1},
                ext_output_image_capture_source_manager_v1::{
                    self, ExtOutputImageCaptureSourceManagerV1,
                },
            },
            image_copy_capture::v1::server::{
                ext_image_copy_capture_cursor_session_v1::{
                    self, ExtImageCopyCaptureCursorSessionV1,
                },
                ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1},
                ext_image_copy_capture_manager_v1::{self, ExtImageCopyCaptureManagerV1},
                ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
            },
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
            backend::{ClientId, GlobalId},
            protocol::wl_buffer::WlBuffer,
        },
    },
    utils::{Buffer as BufferCoords, Logical, Rectangle, Size},
};

use crate::screencopy::{Answer, BYTES, Capture, FORMAT, OutputState};
use crate::state::Solium;
use crate::toplevels::WindowCapture;

/// Whether windows are offered as capture sources: read by the window source
/// manager's `can_view`.
#[derive(Debug, Clone)]
pub(crate) struct WindowsOffered(Arc<AtomicBool>);

/// The globals, held for the session, and the sessions on a window, which
/// are told when it changes.
#[derive(Debug)]
pub(crate) struct ImageCopyState {
    #[expect(
        dead_code,
        reason = "registers ext_output_image_capture_source_manager_v1; dropping it would remove the global"
    )]
    sources: GlobalId,
    #[expect(
        dead_code,
        reason = "registers ext_foreign_toplevel_image_capture_source_manager_v1; dropping it would remove the global"
    )]
    window_sources: GlobalId,
    #[expect(
        dead_code,
        reason = "registers ext_image_copy_capture_manager_v1; dropping it would remove the global"
    )]
    manager: GlobalId,
    windows_offered: WindowsOffered,
    window_sessions: Vec<ExtImageCopyCaptureSessionV1>,
}

impl ImageCopyState {
    pub(crate) fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ExtOutputImageCaptureSourceManagerV1, ()>
            + GlobalDispatch<ExtForeignToplevelImageCaptureSourceManagerV1, WindowsOffered>
            + GlobalDispatch<ExtImageCopyCaptureManagerV1, ()>
            + 'static,
    {
        let windows_offered = WindowsOffered(Arc::new(AtomicBool::new(true)));
        Self {
            sources: display.create_global::<D, ExtOutputImageCaptureSourceManagerV1, _>(1, ()),
            window_sources: display
                .create_global::<D, ExtForeignToplevelImageCaptureSourceManagerV1, _>(
                    1,
                    windows_offered.clone(),
                ),
            manager: display.create_global::<D, ExtImageCopyCaptureManagerV1, _>(1, ()),
            windows_offered,
            window_sessions: Vec::new(),
        }
    }

    /// Offer windows as capture sources, or hide the manager from clients
    /// that have not bound it. Sessions already open on a window stop at the
    /// next pass ([`Solium::follow_window_captures`]).
    pub(crate) fn offer_windows(&self, on: bool) {
        self.windows_offered.0.store(on, Ordering::Relaxed);
    }
}

/// What an `ext_image_capture_source_v1` or a session names.
#[derive(Debug, Clone)]
enum Subject {
    /// A monitor.
    Output(Output),
    /// A window, by the id scripts hold it by.
    Window(u64),
    /// Nothing: a `wl_output` or a window handle already gone when the
    /// source was made, or a cursor session's. Its sessions stop at once.
    Nothing,
}

/// What an `ext_image_capture_source_v1` names.
#[derive(Debug)]
pub(crate) struct Source {
    subject: Subject,
}

/// A capture session, kept in its protocol object's own data.
#[derive(Debug)]
pub(crate) struct Session {
    /// What it captures.
    subject: Subject,
    /// Whether the pointer is drawn into its frames (`paint_cursors`).
    cursor: bool,
    inner: Mutex<SessionState>,
}

#[derive(Debug, Default)]
struct SessionState {
    /// What the client was last told to allocate. `None` before the first
    /// announcement and after the session stopped.
    announced: Option<Announced>,
    /// Whether one of its frames exists. The protocol allows one at a time.
    frame: bool,
    /// Whether `stopped` has been sent. Nothing is captured after it.
    stopped: bool,
}

/// One announcement of buffer constraints, and what it was made for: the
/// monitor, as it was, the region of it or the window's own rectangle, the
/// buffer, and for a window, which one and what of it is drawn.
#[derive(Debug, Clone)]
pub(crate) struct Announced {
    output: Output,
    pub(crate) output_state: OutputState,
    region: Rectangle<i32, Logical>,
    pub(crate) size: Size<i32, BufferCoords>,
    pub(crate) window: Option<(u64, WindowCapture)>,
}

impl Announced {
    /// Whether a buffer allocated for `self` is still the one to allocate
    /// for `now`. A window's monitor may change under it as it moves; only
    /// its size and what is drawn of it matter.
    fn holds(&self, now: &Self) -> bool {
        self.size == now.size
            && self.window == now.window
            && (self.window.is_some() || self.output_state == now.output_state)
    }
}

/// What a session on `subject` would be told to allocate now, or `None` for
/// a subject there is no picture of any more.
fn extent(state: &Solium, subject: &Subject) -> Option<Announced> {
    match subject {
        Subject::Output(output) => output_extent(state, output),
        Subject::Window(id) => window_extent(state, *id),
        Subject::Nothing => None,
    }
}

/// A whole monitor, in its own logical coordinates, and the buffer in device
/// pixels -- the same sums `wlr-screencopy`'s `capture_output` makes. `None`
/// for a monitor gone or with no mode.
fn output_extent(state: &Solium, output: &Output) -> Option<Announced> {
    state.space.output_geometry(output)?;
    let output_state = OutputState::of(output)?;
    let region = Rectangle::from_size(state.output_logical_size(output));
    let size = crate::screencopy::capture_rect(
        region,
        output_state.scale,
        output_state.mode,
        output_state.transform,
    )
    .size;
    (size.w > 0 && size.h > 0).then(|| Announced {
        output: output.clone(),
        output_state,
        region,
        size,
        window: None,
    })
}

/// Window `id` as `toplevels.capture` draws it: its client's window geometry
/// or its pane's outer rectangle, at the scale of the monitor it is on (the
/// first monitor for one on none). `None` for a window gone, or while
/// windows are not offered.
/// `state::tests::real_client::reflow_on_close::window_capture`.
pub(crate) fn window_extent(state: &Solium, id: u64) -> Option<Announced> {
    let settings = state.toplevels.settings;
    if !settings.list {
        return None;
    }
    let look = settings.capture;
    let pane = state.panes.by_script_id(id)?;
    let window = pane.client()?;
    let logical = match look {
        WindowCapture::Off => return None,
        WindowCapture::Client => window.geometry().size,
        WindowCapture::Pane => crate::render::flat(state, window)?.outer,
    };
    let output = state
        .pane_outer_of(pane.id())
        .and_then(|outer| state.output_of(outer))
        .or_else(|| state.space.outputs().next().cloned())?;
    let output_state = OutputState::of(&output)?;
    let physical = logical
        .to_f64()
        .to_physical(output_state.scale)
        .to_i32_round();
    (physical.w > 0 && physical.h > 0).then(|| Announced {
        output,
        output_state,
        region: Rectangle::from_size(logical),
        size: (physical.w, physical.h).into(),
        window: Some((id, look)),
    })
}

/// A frame, kept in its protocol object's own data.
#[derive(Debug)]
pub(crate) struct Frame {
    session: ExtImageCopyCaptureSessionV1,
    inner: Mutex<FrameState>,
}

#[derive(Debug, Default)]
struct FrameState {
    buffer: Option<WlBuffer>,
    /// Set by `capture`, after which the frame takes no more requests but
    /// `destroy`.
    captured: bool,
}

/// A pointer cursor session: whether its one capture session has been taken.
#[derive(Debug, Default)]
pub(crate) struct CursorSession {
    taken: AtomicBool,
}

/// Tell a session what buffer to allocate, as its monitor or window is now.
///
/// A session whose monitor has gone, or has no mode, or whose window has
/// gone, is stopped instead: the protocol's word for a source that is no
/// longer there.
fn announce(state: &Solium, session: &ExtImageCopyCaptureSessionV1, data: &Session) {
    let Ok(mut held) = data.inner.lock() else {
        return;
    };
    if held.stopped {
        return;
    }
    let Some(announced) = extent(state, &data.subject) else {
        held.announced = None;
        held.stopped = true;
        session.stopped();
        return;
    };
    let size = announced.size;
    held.announced = Some(announced);
    drop(held);
    #[expect(
        clippy::cast_sign_loss,
        reason = "a buffer size is positive by construction"
    )]
    session.buffer_size(size.w as u32, size.h as u32);
    session.shm_format(FORMAT);
    session.done();
}

/// A session that will capture nothing more.
fn stop(session: &ExtImageCopyCaptureSessionV1) {
    if let Some(data) = session.data::<Session>()
        && let Ok(mut held) = data.inner.lock()
    {
        if held.stopped {
            return;
        }
        held.stopped = true;
        held.announced = None;
    }
    session.stopped();
}

/// Answer a filled capture.
pub(crate) fn ready(frame: &ExtImageCopyCaptureFrameV1, capture: &Capture) {
    // The rows are stored in the monitor's buffer orientation, as the
    // `wlr-screencopy` path stores them; this says which that is, so the
    // client can turn the picture upright. A window's is drawn upright.
    let transform = if capture.window.is_some() {
        smithay::utils::Transform::Normal
    } else {
        capture.output_state.transform
    };
    frame.transform(transform.into());
    // The whole frame, every time. The protocol only requires it of the
    // first; claiming more than changed costs an encoder a frame it could
    // have skipped, and claiming less would lose real motion.
    frame.damage(0, 0, capture.size.w, capture.size.h);
    // Monotonic, as the protocol asks: the stream's clock, not the wall's.
    let now = crate::pacing::monotonic_now();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a 64-bit second sent as two 32-bit halves"
    )]
    let (high, low) = {
        let seconds = now.as_secs();
        ((seconds >> 32) as u32, seconds as u32)
    };
    frame.presentation_time(high, low, now.subsec_nanos());
    frame.ready();
}

/// Tell a client its capture was not made, and why.
///
/// The reason matters to the client. `buffer_constraints` asks it to
/// allocate again, so a monitor that changed mode is announced again first;
/// `stopped` ends its session; `unknown` says it may simply ask again.
pub(crate) fn failed(state: &Solium, capture: &Capture) {
    let Answer::Ext { frame, session } = &capture.frame else {
        return;
    };
    let Some(data) = session.data::<Session>() else {
        frame.failed(ext_image_copy_capture_frame_v1::FailureReason::Unknown);
        return;
    };
    let asked = Announced {
        output: capture.output.clone(),
        output_state: capture.output_state,
        region: capture.region,
        size: capture.size,
        window: capture.window,
    };
    match extent(state, &data.subject) {
        None => {
            stop(session);
            frame.failed(ext_image_copy_capture_frame_v1::FailureReason::Stopped);
        }
        Some(now) if !asked.holds(&now) => {
            announce(state, session, data);
            frame.failed(ext_image_copy_capture_frame_v1::FailureReason::BufferConstraints);
        }
        Some(_) => frame.failed(ext_image_copy_capture_frame_v1::FailureReason::Unknown),
    }
}

impl Solium {
    /// Tell each session on a window what changed since it was last told:
    /// a new size, a new `toplevels.capture`, announced again; a window gone,
    /// or windows no longer offered, stopped. Every pass, beside the window
    /// lists, and as cheap with no session on a window: nothing at all.
    /// `state::tests::real_client::reflow_on_close::window_capture`.
    pub(crate) fn follow_window_captures(&mut self) {
        self.image_copy_state
            .window_sessions
            .retain(|session| session.is_alive());
        let sessions = self.image_copy_state.window_sessions.clone();
        for session in &sessions {
            let Some(data) = session.data::<Session>() else {
                continue;
            };
            let held = data
                .inner
                .lock()
                .ok()
                .and_then(|held| (!held.stopped).then(|| held.announced.clone()));
            let Some(announced) = held else {
                continue;
            };
            match extent(self, &data.subject) {
                None => stop(session),
                Some(now) => {
                    if !announced.is_some_and(|announced| announced.holds(&now)) {
                        announce(self, session, data);
                    }
                }
            }
        }
        self.image_copy_state.window_sessions.retain(|session| {
            session
                .data::<Session>()
                .and_then(|data| data.inner.lock().ok().map(|held| !held.stopped))
                .unwrap_or(false)
        });
    }
}

/// Whether `buffer` is one the announced capture can be copied into: shared
/// memory in [`FORMAT`], the announced size, and rows long enough.
fn fits(buffer: &WlBuffer, size: Size<i32, BufferCoords>) -> bool {
    smithay::wayland::shm::with_buffer_contents(buffer, |_, _, data| {
        data.format == FORMAT
            && data.width == size.w
            && data.height == size.h
            && data.stride >= size.w * BYTES
    })
    .unwrap_or(false)
}

impl GlobalDispatch<ExtOutputImageCaptureSourceManagerV1, ()> for Solium {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ExtOutputImageCaptureSourceManagerV1>,
        (): &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ExtOutputImageCaptureSourceManagerV1, ()> for Solium {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _manager: &ExtOutputImageCaptureSourceManagerV1,
        request: ext_output_image_capture_source_manager_v1::Request,
        (): &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let ext_output_image_capture_source_manager_v1::Request::CreateSource {
            source,
            output,
        } = request
        {
            // A monitor unplugged between the request and its arrival makes
            // a source of nothing, and every session on it stops at once.
            data_init.init(
                source,
                Source {
                    subject: Output::from_resource(&output)
                        .map_or(Subject::Nothing, Subject::Output),
                },
            );
        }
    }
}

impl GlobalDispatch<ExtForeignToplevelImageCaptureSourceManagerV1, WindowsOffered> for Solium {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ExtForeignToplevelImageCaptureSourceManagerV1>,
        _offered: &WindowsOffered,
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(_client: Client, offered: &WindowsOffered) -> bool {
        offered.0.load(Ordering::Relaxed)
    }
}

impl Dispatch<ExtForeignToplevelImageCaptureSourceManagerV1, ()> for Solium {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _manager: &ExtForeignToplevelImageCaptureSourceManagerV1,
        request: ext_foreign_toplevel_image_capture_source_manager_v1::Request,
        (): &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let ext_foreign_toplevel_image_capture_source_manager_v1::Request::CreateSource {
            source,
            toplevel_handle,
        } = request
        {
            // A window that went between its handle's last event and this
            // makes a source of nothing, as a monitor unplugged does.
            data_init.init(
                source,
                Source {
                    subject: crate::toplevels::listed_id(&toplevel_handle)
                        .map_or(Subject::Nothing, Subject::Window),
                },
            );
        }
    }
}

impl Dispatch<ExtImageCaptureSourceV1, Source> for Solium {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _source: &ExtImageCaptureSourceV1,
        _request: ext_image_capture_source_v1::Request,
        _data: &Source,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        // `destroy` only, and sessions made from it carry on: they hold what
        // they capture themselves.
    }
}

impl GlobalDispatch<ExtImageCopyCaptureManagerV1, ()> for Solium {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ExtImageCopyCaptureManagerV1>,
        (): &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ExtImageCopyCaptureManagerV1, ()> for Solium {
    fn request(
        state: &mut Self,
        _client: &Client,
        manager: &ExtImageCopyCaptureManagerV1,
        request: ext_image_copy_capture_manager_v1::Request,
        (): &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            ext_image_copy_capture_manager_v1::Request::CreateSession {
                session,
                source,
                options,
            } => {
                let subject = source
                    .data::<Source>()
                    .map_or(Subject::Nothing, |source| source.subject.clone());
                let on_window = matches!(subject, Subject::Window(_));
                let options = match options {
                    WEnum::Value(options) => Some(options),
                    WEnum::Unknown(_) => None,
                };
                let session = data_init.init(
                    session,
                    Session {
                        subject,
                        cursor: options.is_some_and(|options| {
                            options
                                .contains(ext_image_copy_capture_manager_v1::Options::PaintCursors)
                        }),
                        inner: Mutex::default(),
                    },
                );
                if options.is_none() {
                    manager.post_error(
                        ext_image_copy_capture_manager_v1::Error::InvalidOption,
                        "unknown capture options",
                    );
                    return;
                }
                // Announced whether or not the session is locked or the
                // monitor is off: a size is not a picture, and its frames
                // wait for one (`screencopy::waits`).
                if let Some(data) = session.data::<Session>() {
                    announce(state, &session, data);
                }
                if on_window {
                    state.image_copy_state.window_sessions.push(session);
                }
            }
            ext_image_copy_capture_manager_v1::Request::CreatePointerCursorSession {
                session,
                ..
            } => {
                data_init.init(session, CursorSession::default());
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureCursorSessionV1, CursorSession> for Solium {
    fn request(
        _state: &mut Self,
        _client: &Client,
        cursor: &ExtImageCopyCaptureCursorSessionV1,
        request: ext_image_copy_capture_cursor_session_v1::Request,
        data: &CursorSession,
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let ext_image_copy_capture_cursor_session_v1::Request::GetCaptureSession { session } =
            request
        {
            // No monitor, so stopped as soon as it exists. See the module's
            // documentation for why there is no cursor stream.
            let session = data_init.init(
                session,
                Session {
                    subject: Subject::Nothing,
                    cursor: false,
                    inner: Mutex::default(),
                },
            );
            if data.taken.swap(true, Ordering::Relaxed) {
                cursor.post_error(
                    ext_image_copy_capture_cursor_session_v1::Error::DuplicateSession,
                    "this cursor session's capture session was already taken",
                );
                return;
            }
            stop(&session);
        }
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, Session> for Solium {
    fn request(
        _state: &mut Self,
        _client: &Client,
        session: &ExtImageCopyCaptureSessionV1,
        request: ext_image_copy_capture_session_v1::Request,
        data: &Session,
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let ext_image_copy_capture_session_v1::Request::CreateFrame { frame } = request {
            data_init.init(
                frame,
                Frame {
                    session: session.clone(),
                    inner: Mutex::default(),
                },
            );
            let duplicate = data
                .inner
                .lock()
                .map_or(true, |mut held| std::mem::replace(&mut held.frame, true));
            if duplicate {
                session.post_error(
                    ext_image_copy_capture_session_v1::Error::DuplicateFrame,
                    "a frame was created before the previous one was destroyed",
                );
            }
        }
        // `destroy` stops nothing the client still holds: its frame, if
        // any, is answered or forgotten as it would have been.
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, Frame> for Solium {
    fn request(
        state: &mut Self,
        _client: &Client,
        frame: &ExtImageCopyCaptureFrameV1,
        request: ext_image_copy_capture_frame_v1::Request,
        data: &Frame,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let Ok(mut held) = data.inner.lock() else {
            frame.failed(ext_image_copy_capture_frame_v1::FailureReason::Unknown);
            return;
        };
        match request {
            ext_image_copy_capture_frame_v1::Request::AttachBuffer { buffer } => {
                if held.captured {
                    frame.post_error(
                        ext_image_copy_capture_frame_v1::Error::AlreadyCaptured,
                        "a buffer was attached after capture",
                    );
                    return;
                }
                held.buffer = Some(buffer);
            }
            ext_image_copy_capture_frame_v1::Request::DamageBuffer {
                x,
                y,
                width,
                height,
            } => {
                if held.captured {
                    frame.post_error(
                        ext_image_copy_capture_frame_v1::Error::AlreadyCaptured,
                        "damage was sent after capture",
                    );
                } else if x < 0 || y < 0 || width <= 0 || height <= 0 {
                    frame.post_error(
                        ext_image_copy_capture_frame_v1::Error::InvalidBufferDamage,
                        "buffer damage must be inside the buffer and not empty",
                    );
                }
                // Otherwise nothing to keep: every capture fills the whole
                // buffer, which covers whatever the client damaged.
            }
            ext_image_copy_capture_frame_v1::Request::Capture => {
                if held.captured {
                    frame.post_error(
                        ext_image_copy_capture_frame_v1::Error::AlreadyCaptured,
                        "capture was sent twice",
                    );
                    return;
                }
                let Some(buffer) = held.buffer.clone() else {
                    frame.post_error(
                        ext_image_copy_capture_frame_v1::Error::NoBuffer,
                        "capture was sent without a buffer",
                    );
                    return;
                };
                held.captured = true;
                drop(held);
                capture(state, frame, &data.session, buffer);
            }
            _ => {}
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        frame: &ExtImageCopyCaptureFrameV1,
        data: &Frame,
    ) {
        // Destroyed after its answer, as a stream does before asking for the
        // next one; or before it, by a client that changed its mind or went
        // away. Either way the session may have another.
        state.forget_capture(&frame.id());
        if let Some(session) = data.session.data::<Session>()
            && let Ok(mut held) = session.inner.lock()
        {
            held.frame = false;
        }
    }
}

/// Queue a frame's capture, or say why it cannot be made.
fn capture(
    state: &mut Solium,
    frame: &ExtImageCopyCaptureFrameV1,
    session: &ExtImageCopyCaptureSessionV1,
    buffer: WlBuffer,
) {
    let Some(data) = session.data::<Session>() else {
        frame.failed(ext_image_copy_capture_frame_v1::FailureReason::Unknown);
        return;
    };
    let announced = data
        .inner
        .lock()
        .ok()
        .and_then(|held| (!held.stopped).then(|| held.announced.clone()).flatten());
    let Some(announced) = announced else {
        frame.failed(ext_image_copy_capture_frame_v1::FailureReason::Stopped);
        return;
    };
    // Checked now rather than when it is filled: a client told at once that
    // its buffer is the wrong one allocates again, and one told a frame
    // later has lost that frame. A dma-buf is never offered, so it is
    // always the wrong one.
    if !fits(&buffer, announced.size) {
        frame.failed(ext_image_copy_capture_frame_v1::FailureReason::BufferConstraints);
        return;
    }
    // Queued whatever the lock or the monitor's power: `screencopy::settle`
    // holds it until there is a picture it may be given.
    state.pending_captures.push(Capture {
        frame: Answer::Ext {
            frame: frame.clone(),
            session: session.clone(),
        },
        output: announced.output,
        region: announced.region,
        size: announced.size,
        output_state: announced.output_state,
        cursor: data.cursor,
        buffer: Some(buffer),
        damage: true,
        window: announced.window,
    });
    state.redraw = true;
}
