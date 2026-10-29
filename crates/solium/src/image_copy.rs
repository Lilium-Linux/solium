//! `ext-image-copy-capture-v1`: handing a monitor's picture to a client, the
//! newer way.
//!
//! The successor to `wlr-screencopy` (`screencopy.rs`), offered beside it.
//! A client names what it wants as an `ext_image_capture_source_v1` -- here a
//! monitor, through `ext_output_image_capture_source_manager_v1` -- opens a
//! session on it, is told once what buffer to allocate, and then asks for
//! frames into that buffer one after another until it is done.
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
//! Two parts of the protocol are not offered. A window as a source
//! (`ext_foreign_toplevel_image_capture_source_manager_v1`) needs
//! `ext-foreign-toplevel-list-v1` first, and is #47. A pointer cursor session
//! is accepted, and the capture session it hands out says `stopped` at once:
//! the cursor is in the picture when a client asks for `paint_cursors`, and a
//! separate stream of it is something nothing here draws.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use smithay::{
    output::Output,
    reexports::{
        wayland_protocols::ext::{
            image_capture_source::v1::server::{
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

/// The two globals, held for the session.
#[derive(Debug)]
pub(crate) struct ImageCopyState {
    #[expect(
        dead_code,
        reason = "registers ext_output_image_capture_source_manager_v1; dropping it would remove the global"
    )]
    sources: GlobalId,
    #[expect(
        dead_code,
        reason = "registers ext_image_copy_capture_manager_v1; dropping it would remove the global"
    )]
    manager: GlobalId,
}

impl ImageCopyState {
    pub(crate) fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ExtOutputImageCaptureSourceManagerV1, ()>
            + GlobalDispatch<ExtImageCopyCaptureManagerV1, ()>
            + 'static,
    {
        Self {
            sources: display.create_global::<D, ExtOutputImageCaptureSourceManagerV1, _>(1, ()),
            manager: display.create_global::<D, ExtImageCopyCaptureManagerV1, _>(1, ()),
        }
    }
}

/// What an `ext_image_capture_source_v1` names: a monitor, or nothing when
/// the `wl_output` it was made from had already gone.
#[derive(Debug)]
pub(crate) struct Source {
    output: Option<Output>,
}

/// A capture session, kept in its protocol object's own data.
#[derive(Debug)]
pub(crate) struct Session {
    /// The monitor. `None` for a session that never had one, which is
    /// stopped from the start.
    output: Option<Output>,
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

/// One announcement of buffer constraints, and the monitor it was made for.
#[derive(Debug, Clone, Copy)]
struct Announced {
    output_state: OutputState,
    region: Rectangle<i32, Logical>,
    size: Size<i32, BufferCoords>,
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

/// Tell a session what buffer to allocate, as the monitor is now.
///
/// A session whose monitor has gone, or has no mode, is stopped instead: the
/// protocol's word for a source that is no longer there.
fn announce(state: &Solium, session: &ExtImageCopyCaptureSessionV1, data: &Session) {
    let Ok(mut held) = data.inner.lock() else {
        return;
    };
    if held.stopped {
        return;
    }
    let announced = data
        .output
        .as_ref()
        .filter(|output| state.space.output_geometry(output).is_some())
        .and_then(|output| {
            let output_state = OutputState::of(output)?;
            // The whole monitor, in its own logical coordinates, and the
            // buffer in device pixels -- the same sums `wlr-screencopy`'s
            // `capture_output` makes.
            let region = Rectangle::from_size(state.output_logical_size(output));
            let size = crate::screencopy::capture_rect(
                region,
                output_state.scale,
                output_state.mode,
                output_state.transform,
            )
            .size;
            (size.w > 0 && size.h > 0).then_some(Announced {
                output_state,
                region,
                size,
            })
        });
    let Some(announced) = announced else {
        held.announced = None;
        held.stopped = true;
        session.stopped();
        return;
    };
    held.announced = Some(announced);
    drop(held);
    #[expect(
        clippy::cast_sign_loss,
        reason = "a buffer size is positive by construction"
    )]
    session.buffer_size(announced.size.w as u32, announced.size.h as u32);
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
    // client can turn the picture upright.
    frame.transform(capture.output_state.transform.into());
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
    let mapped = state.space.output_geometry(&capture.output).is_some();
    match OutputState::of(&capture.output).filter(|_| mapped) {
        None => {
            stop(session);
            frame.failed(ext_image_copy_capture_frame_v1::FailureReason::Stopped);
        }
        Some(now) if now != capture.output_state => {
            if let Some(data) = session.data::<Session>() {
                announce(state, session, data);
            }
            frame.failed(ext_image_copy_capture_frame_v1::FailureReason::BufferConstraints);
        }
        Some(_) => frame.failed(ext_image_copy_capture_frame_v1::FailureReason::Unknown),
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
                    output: Output::from_resource(&output),
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
        // `destroy` only, and sessions made from it carry on: they hold the
        // monitor themselves.
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
                let output = source
                    .data::<Source>()
                    .and_then(|source| source.output.clone());
                let options = match options {
                    WEnum::Value(options) => Some(options),
                    WEnum::Unknown(_) => None,
                };
                let session = data_init.init(
                    session,
                    Session {
                        output,
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
                    output: None,
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
        .and_then(|held| (!held.stopped).then_some(held.announced).flatten());
    let (Some(announced), Some(output)) = (announced, data.output.clone()) else {
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
        output,
        region: announced.region,
        size: announced.size,
        output_state: announced.output_state,
        cursor: data.cursor,
        buffer: Some(buffer),
        damage: true,
    });
    state.redraw = true;
}
