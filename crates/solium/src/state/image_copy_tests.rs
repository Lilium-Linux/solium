// `ext-image-copy-capture-v1` from a client's side (#387): a screen share
// through `xdg-desktop-portal-wlr`, which asks for this protocol when it is
// offered. Its `wlr-screencopy` path in 0.8.3 and 0.8.4 hands over one frame
// and never asks for a second, which is the frozen share #387 is about.
//
// `ready` itself is sent by `screencopy::settle`, which needs a renderer and
// is not reached from here; `dev/screencast-check.sh` runs the portal against
// a nested Solium and counts the frames that arrive. What these play is
// everything up to it: the session, the frames asked for one after another,
// and what waits instead of being refused.

use super::*;
use crate::screencopy::Answer;

/// A share's end of the protocol, set up as the portal sets it up: a source
/// for the monitor, a session on it with the pointer painted in, and a buffer
/// of the size the session announced.
struct Stream {
    session: ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1,
    buffer: wl_buffer::WlBuffer,
    size: (i32, i32),
}

impl Stream {
    fn open(session: &mut Session) -> Self {
        let sources = session
            .app
            .client
            .image_sources
            .clone()
            .expect("ext_output_image_capture_source_manager_v1 is not advertised");
        let manager = session
            .app
            .client
            .image_copy
            .clone()
            .expect("ext_image_copy_capture_manager_v1 is not advertised");
        let output = session.app.client.output.clone().expect("wl_output bound");
        let source = sources.create_source(&output, &session.app.qh, ());
        let copy = manager.create_session(
            &source,
            ext_image_copy_capture_manager_v1::Options::PaintCursors,
            &session.app.qh,
            (),
        );
        session.app.pump(&mut session.display, &mut session.state);

        let monitor = session
            .state
            .space
            .outputs()
            .next()
            .cloned()
            .expect("a monitor");
        let mode = monitor.current_mode().expect("a mode").size;
        assert_eq!(
            monitor.current_scale().fractional_scale(),
            1.0,
            "the premise: at scale 1 the buffer is the mode"
        );
        let upright = monitor.current_transform().transform_size(mode);
        #[expect(clippy::cast_sign_loss, reason = "a mode is positive")]
        let (width, height) = (upright.w as u32, upright.h as u32);
        assert_eq!(
            session.app.client.copy_constraints,
            [
                CopyConstraint::Size(width, height),
                CopyConstraint::Shm(u32::from(wl_shm::Format::Xrgb8888)),
                CopyConstraint::Done,
            ],
            "the session did not announce the monitor's size and format, once"
        );

        let shm = session.app.client.shm.clone().expect("wl_shm bound");
        let length = upright.w * upright.h * 4;
        let fd = anon_file(length);
        let pool = shm.create_pool(fd.as_fd(), length, &session.app.qh, ());
        let buffer = pool.create_buffer(
            0,
            upright.w,
            upright.h,
            upright.w * 4,
            wl_shm::Format::Xrgb8888,
            &session.app.qh,
            (),
        );
        session.app.pump(&mut session.display, &mut session.state);
        Self {
            session: copy,
            buffer,
            size: (upright.w, upright.h),
        }
    }

    /// One frame asked for the way the portal asks: the buffer attached, all
    /// of it damaged, and `capture`.
    fn frame(
        &self,
        session: &mut Session,
        buffer: &wl_buffer::WlBuffer,
    ) -> ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1 {
        let frame = self.session.create_frame(&session.app.qh, ());
        frame.attach_buffer(buffer);
        frame.damage_buffer(0, 0, self.size.0, self.size.1);
        frame.capture();
        session.app.pump(&mut session.display, &mut session.state);
        frame
    }
}

/// Whether the one queued capture is an `ext-image-copy-capture-v1` one.
fn queued_ext(session: &Session) -> bool {
    matches!(
        session.state.pending_captures.as_slice(),
        [capture] if matches!(capture.frame, Answer::Ext { .. })
    )
}

/// **A screen share gets frame after frame** (#387). Each frame the portal
/// asks for is queued for the next render pass and asks for that pass, and
/// once it is destroyed, as the portal destroys each one after its answer,
/// the next one is taken rather than refused as a duplicate.
///
/// Before #387, fails at "not advertised": with only `wlr-screencopy` on
/// offer, the portal took the path that stops after one frame.
#[test]
fn a_screen_share_gets_frame_after_frame() {
    let mut session = Session::new();
    session.app.open(&mut session.display, &mut session.state);
    let stream = Stream::open(&mut session);
    for n in 0..3 {
        session.state.redraw = false;
        let frame = stream.frame(&mut session, &stream.buffer);
        assert!(
            queued_ext(&session),
            "frame {n} was not queued for the next render pass"
        );
        assert!(
            session.state.redraw,
            "frame {n} did not ask for a render pass, so it waits on something else drawing"
        );
        assert_eq!(
            session.app.client.copy_answers,
            [],
            "frame {n} was answered before anything filled it"
        );
        frame.destroy();
        session.app.pump(&mut session.display, &mut session.state);
        assert!(
            session.state.pending_captures.is_empty(),
            "frame {n} was destroyed and its capture is still queued"
        );
    }
    assert_eq!(session.app.client.copy_stopped, 0, "the session stopped");
}

/// **A share waits out the lock** rather than ending. A frame asked for
/// while the session is locked is neither filled, which would be a picture
/// of the lock screen, nor refused, which ends the portal's stream: it waits,
/// and is filled at the first pass after the unlock.
#[test]
fn a_screen_share_waits_out_the_lock() {
    let mut session = Session::new();
    session.app.open(&mut session.display, &mut session.state);
    let stream = Stream::open(&mut session);
    let lock = session.lock();
    let _frame = stream.frame(&mut session, &stream.buffer);
    assert!(queued_ext(&session), "a frame asked for while locked was not kept");
    assert!(
        crate::screencopy::waits(&session.state, &session.state.pending_captures[0]),
        "a frame asked for while locked would be filled from the lock screen"
    );
    assert_eq!(
        session.app.client.copy_answers,
        [],
        "a frame asked for while locked was answered"
    );
    session.assert_unlocks(lock);
    assert!(
        queued_ext(&session)
            && !crate::screencopy::waits(&session.state, &session.state.pending_captures[0]),
        "after the unlock, the waiting frame is still not filled"
    );
}

/// **A share waits for a monitor that is off**, where a screenshot is
/// refused (`an_off_monitor_cannot_be_captured`): the frame waiting when the
/// monitor goes off is kept, and is filled once it is back on.
#[test]
fn a_screen_share_waits_for_a_monitor_that_is_off() {
    let mut session = Session::new();
    session.app.open(&mut session.display, &mut session.state);
    let control = session.control(0);
    let stream = Stream::open(&mut session);
    let _frame = stream.frame(&mut session, &stream.buffer);
    assert!(queued_ext(&session), "the premise: a frame is waiting");

    session.power(&control, false);
    assert_eq!(
        session.app.client.copy_answers,
        [],
        "a share's frame was refused when its monitor went off"
    );
    assert!(
        queued_ext(&session)
            && crate::screencopy::waits(&session.state, &session.state.pending_captures[0]),
        "a share's frame for a monitor that is off is not waiting for it"
    );

    session.power(&control, true);
    assert!(
        queued_ext(&session)
            && !crate::screencopy::waits(&session.state, &session.state.pending_captures[0]),
        "the monitor is back on and the frame is still not filled"
    );
}

/// **A buffer that does not fit is refused at once**, with the reason that
/// asks the client to allocate again, rather than queued and refused a frame
/// later.
#[test]
fn a_buffer_that_does_not_fit_is_refused_at_once() {
    let mut session = Session::new();
    session.app.open(&mut session.display, &mut session.state);
    let stream = Stream::open(&mut session);
    let shm = session.app.client.shm.clone().expect("wl_shm bound");
    let fd = anon_file(16 * 16 * 4);
    let pool = shm.create_pool(fd.as_fd(), 16 * 16 * 4, &session.app.qh, ());
    let small = pool.create_buffer(
        0,
        16,
        16,
        16 * 4,
        wl_shm::Format::Xrgb8888,
        &session.app.qh,
        (),
    );
    let _frame = stream.frame(&mut session, &small);
    assert_eq!(
        session.app.client.copy_answers,
        [Err(u32::from(
            ext_image_copy_capture_frame_v1::FailureReason::BufferConstraints
        ))],
        "a 16x16 buffer for a whole monitor was not refused as the wrong buffer"
    );
    assert!(
        session.state.pending_captures.is_empty(),
        "and it was queued anyway"
    );
}
