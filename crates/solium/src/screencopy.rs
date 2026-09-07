//! `wlr-screencopy-unstable-v1`: handing a rendered frame to a client.
//!
//! This is screenshots, screen recording and screen sharing — all three, from
//! one protocol. `grim` takes a picture with it, `wf-recorder` records with it,
//! and `xdg-desktop-portal-wlr` shares a screen with it, which is what every
//! conferencing application asks the portal for.
//!
//! Implemented by hand because Smithay has no handler for it. The protocol is
//! small: a client asks for an output, is told what buffer to allocate, gives
//! us one, and is told when it has been filled.
//!
//! ## Why the newer protocol is not this one
//!
//! `ext-image-copy-capture-v1` replaces this and is where things are going,
//! but the `wayland-protocols` release this project builds against does not
//! carry it yet, and nothing installed anywhere speaks it. Choosing the
//! superseded protocol is deliberate: it is the one every tool in existence
//! already uses, and a preview that cannot take a screenshot is a preview
//! nobody can file a bug about. `ext-image-copy-capture-v1` is #47.
//!
//! ## When the copy happens
//!
//! Not when the client asks. Reading pixels needs a renderer, and the renderer
//! belongs to the backend's render loop — so a request is queued and drained
//! there, which is also the moment the frame it is copying actually exists.
//! `Solium::pending_captures` is that queue.

use std::sync::Mutex;

use smithay::{
    output::Output,
    reexports::{
        wayland_protocols_wlr::screencopy::v1::server::{
            zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
            zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
            protocol::{wl_buffer::WlBuffer, wl_shm},
        },
    },
    utils::{Buffer as BufferCoords, Rectangle},
};

use crate::state::Solium;

/// The format captures are handed over in.
///
/// One, deliberately. Every screencopy client in existence handles
/// `Xrgb8888`, it is what the compositor renders into, and offering a list
/// invites a client to pick the one path that was never exercised.
const FORMAT: wl_shm::Format = wl_shm::Format::Xrgb8888;

/// Bytes per pixel in [`FORMAT`].
const BYTES: i32 = 4;

/// The global, held for the session.
#[derive(Debug)]
pub(crate) struct ScreencopyState {
    #[expect(
        dead_code,
        reason = "registers zwlr_screencopy_manager_v1; dropping it would remove the global"
    )]
    global: smithay::reexports::wayland_server::backend::GlobalId,
}

impl ScreencopyState {
    pub(crate) fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ZwlrScreencopyManagerV1, ()> + 'static,
    {
        Self {
            global: display.create_global::<D, ZwlrScreencopyManagerV1, _>(3, ()),
        }
    }
}

/// What a client asked for, and what it is waiting on.
#[derive(Debug)]
pub(crate) struct Capture {
    /// The protocol object to answer.
    pub(crate) frame: ZwlrScreencopyFrameV1,
    /// Which monitor.
    pub(crate) output: Output,
    /// The region of that monitor, in its own logical coordinates.
    pub(crate) region: Rectangle<i32, smithay::utils::Logical>,
    /// The size of the buffer the client was told to allocate, in pixels.
    pub(crate) size: smithay::utils::Size<i32, BufferCoords>,
    /// Whether the pointer should be in the picture.
    pub(crate) cursor: bool,
    /// The buffer, once the client has handed one over.
    pub(crate) buffer: Option<WlBuffer>,
    /// Whether the client wants damage reported before `ready`.
    pub(crate) damage: bool,
}

/// A frame's state while it waits, kept in the protocol object's own data.
#[derive(Debug, Default)]
pub(crate) struct FrameData {
    inner: Mutex<Option<Pending>>,
}

/// What `capture_output` recorded, before a buffer arrived.
#[derive(Debug, Clone)]
struct Pending {
    output: Output,
    region: Rectangle<i32, smithay::utils::Logical>,
    size: smithay::utils::Size<i32, BufferCoords>,
    cursor: bool,
    /// Set once `copy` has been called, so a second one is an error rather
    /// than a second capture.
    used: bool,
}

impl GlobalDispatch<ZwlrScreencopyManagerV1, ()> for Solium {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrScreencopyManagerV1>,
        (): &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for Solium {
    fn request(
        state: &mut Self,
        _client: &Client,
        _manager: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        (): &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let (frame, overlay_cursor, wl_output, region) = match request {
            zwlr_screencopy_manager_v1::Request::CaptureOutput {
                frame,
                overlay_cursor,
                output,
            } => (frame, overlay_cursor, output, None),
            zwlr_screencopy_manager_v1::Request::CaptureOutputRegion {
                frame,
                overlay_cursor,
                output,
                x,
                y,
                width,
                height,
            } => (
                frame,
                overlay_cursor,
                output,
                Some(Rectangle::new((x, y).into(), (width, height).into())),
            ),
            zwlr_screencopy_manager_v1::Request::Destroy => return,
            _ => return,
        };

        let frame = data_init.init(frame, FrameData::default());
        let Some(output) = Output::from_resource(&wl_output) else {
            // The output is gone. `failed` and not a protocol error: a monitor
            // being unplugged between the request and its arrival is a race
            // the client cannot avoid and should not be killed for.
            frame.failed();
            return;
        };

        // The whole monitor unless a region was named, in its *own* logical
        // coordinates — the protocol's region is relative to the output, not
        // to the global space the compositor lays out in.
        let whole = Rectangle::from_size(state.output_logical_size(&output));
        let region = region.map_or(whole, |asked| {
            // Intersected rather than trusted. A region reaching off the
            // monitor would otherwise size a buffer for pixels that do not
            // exist, and the copy would read past the frame.
            asked.intersection(whole).unwrap_or_default()
        });
        if region.size.w <= 0 || region.size.h <= 0 {
            frame.failed();
            return;
        }

        // The buffer is in *device* pixels: a capture of a 2x monitor is worth
        // having at the resolution it actually has.
        let scale = output.current_scale().fractional_scale();
        #[expect(clippy::cast_possible_truncation, reason = "a region of one monitor")]
        let size: smithay::utils::Size<i32, BufferCoords> = (
            ((f64::from(region.size.w) * scale).round() as i32).max(1),
            ((f64::from(region.size.h) * scale).round() as i32).max(1),
        )
            .into();

        if let Some(data) = frame.data::<FrameData>()
            && let Ok(mut held) = data.inner.lock()
        {
            *held = Some(Pending {
                output,
                region,
                size,
                cursor: overlay_cursor != 0,
                used: false,
            });
        }

        #[expect(
            clippy::cast_sign_loss,
            reason = "a buffer size and stride are positive by construction"
        )]
        frame.buffer(
            FORMAT,
            size.w as u32,
            size.h as u32,
            (size.w * BYTES) as u32,
        );
        // Version 3 clients wait for this before allocating; older ones do not
        // get it and allocate on `buffer` alone, which is why it is sent last.
        if frame.version() >= 3 {
            frame.buffer_done();
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, FrameData> for Solium {
    fn request(
        state: &mut Self,
        _client: &Client,
        frame: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        data: &FrameData,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let (buffer, damage) = match request {
            zwlr_screencopy_frame_v1::Request::Copy { buffer } => (buffer, false),
            zwlr_screencopy_frame_v1::Request::CopyWithDamage { buffer } => (buffer, true),
            zwlr_screencopy_frame_v1::Request::Destroy => {
                state.forget_capture(frame);
                return;
            }
            _ => return,
        };

        let Ok(mut held) = data.inner.lock() else {
            frame.failed();
            return;
        };
        let Some(pending) = held.as_mut() else {
            frame.failed();
            return;
        };
        if pending.used {
            frame.post_error(
                zwlr_screencopy_frame_v1::Error::AlreadyUsed,
                "this frame has already been copied",
            );
            return;
        }
        pending.used = true;
        let pending = pending.clone();
        drop(held);

        // Queued, not copied. Reading pixels needs a renderer, and the
        // renderer belongs to the backend's loop -- which is also the only
        // place where the frame being copied actually exists.
        state.pending_captures.push(Capture {
            frame: frame.clone(),
            output: pending.output,
            region: pending.region,
            size: pending.size,
            cursor: pending.cursor,
            buffer: Some(buffer),
            damage,
        });
        state.redraw = true;
    }

    fn destroyed(
        state: &mut Self,
        _client: smithay::reexports::wayland_server::backend::ClientId,
        frame: &ZwlrScreencopyFrameV1,
        _data: &FrameData,
    ) {
        // A client that goes away mid-capture leaves a queued request whose
        // buffer is gone with it.
        state.forget_capture(frame);
    }
}

/// Fill every buffer a client has handed over, and answer for it.
///
/// Called from the backend's render loop, which is the only place a renderer
/// exists. One capture is one extra render pass over one monitor's region, so a
/// recorder asking every frame costs about one extra frame — which is what
/// recording is.
pub(crate) fn settle(
    state: &mut Solium,
    renderer: &mut smithay::backend::renderer::gles::GlesRenderer,
    prepared: &crate::render::Prepared,
) {
    if state.pending_captures.is_empty() {
        return;
    }
    // Taken, so a capture that fails cannot be retried forever and a scene
    // that damages during the copy queues for the *next* frame.
    let captures = std::mem::take(&mut state.pending_captures);
    for capture in captures {
        match fill(state, renderer, prepared, &capture) {
            Ok(()) => {
                // The buffer is top-down and in the format the client was
                // told, so no flags. `Y_INVERT` exists for compositors that
                // hand back a GL framebuffer as it comes; this one flips it,
                // because a client that ignores the flag then writes an
                // upside-down screenshot and blames its own code.
                capture
                    .frame
                    .flags(zwlr_screencopy_frame_v1::Flags::empty());
                if capture.damage {
                    // The whole region. A recorder uses this to skip encoding
                    // an unchanged frame, and claiming less than actually
                    // changed would drop real motion; claiming everything
                    // costs it an encode it might have skipped.
                    #[expect(
                        clippy::cast_sign_loss,
                        reason = "a region size is positive by construction"
                    )]
                    capture
                        .frame
                        .damage(0, 0, capture.size.w as u32, capture.size.h as u32);
                }
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                // The protocol splits the seconds into two halves, so both
                // of these truncate on purpose.
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "a 64-bit second sent as two 32-bit halves"
                )]
                let (high, low) = {
                    let seconds = now.as_secs();
                    ((seconds >> 32) as u32, seconds as u32)
                };
                capture.frame.ready(high, low, now.subsec_nanos());
            }
            Err(err) => {
                tracing::warn!(?err, "a screen capture failed");
                capture.frame.failed();
            }
        }
    }
}

/// Render the requested region and copy it into the client's buffer.
fn fill(
    state: &mut Solium,
    renderer: &mut smithay::backend::renderer::gles::GlesRenderer,
    prepared: &crate::render::Prepared,
    capture: &Capture,
) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use smithay::backend::{
        allocator::Fourcc,
        renderer::element::{Element as _, RenderElement},
        renderer::{Bind, Color32F, ExportMem, Frame as _, Offscreen, Renderer},
    };
    use smithay::utils::{Physical, Scale, Size, Transform};

    let buffer = capture
        .buffer
        .as_ref()
        .context("a capture with no buffer")?;

    // Where the region is in the global space, which is what `elements` draws
    // against. The protocol's coordinates are relative to the output.
    let origin = state
        .space
        .output_geometry(&capture.output)
        .context("the output is no longer mapped")?
        .loc;
    let screen = smithay::utils::Rectangle::new(origin + capture.region.loc, capture.region.size);
    let scale = capture.output.current_scale().fractional_scale();

    let size: Size<i32, Physical> = (capture.size.w, capture.size.h).into();
    let elements = crate::render::elements(
        state,
        renderer,
        prepared,
        crate::render::Picture {
            screen,
            scale,
            cursor: capture.cursor,
        },
    );

    // Its own texture, for the same reason the deform path uses one: this
    // binds a framebuffer, and doing that underneath the output's own bind
    // sends the whole frame into the texture instead of onto the screen.
    let buffer_size: smithay::utils::Size<i32, BufferCoords> = (size.w, size.h).into();
    let mut texture: smithay::backend::renderer::gles::GlesTexture = renderer
        .create_buffer(Fourcc::Abgr8888, buffer_size)
        .context("no buffer to capture into")?;

    let pixels = {
        let mut framebuffer = renderer
            .bind(&mut texture)
            .context("binding the capture buffer")?;
        {
            let mut frame = renderer
                .render(&mut framebuffer, size, Transform::Normal)
                .context("rendering the capture")?;
            frame
                .clear(
                    Color32F::new(0.05, 0.05, 0.06, 1.0),
                    &[smithay::utils::Rectangle::from_size(size)],
                )
                .context("clearing the capture")?;
            let whole = [smithay::utils::Rectangle::from_size(size)];
            // Reversed: the list is topmost-first, and drawing it in that order
            // onto a cleared buffer paints the top of the stack first.
            for element in elements.iter().rev() {
                let source = element.src();
                let destination = element.geometry(Scale::from(scale));
                if let Err(err) = element.draw(&mut frame, source, destination, &whole, &[]) {
                    tracing::warn!(?err, "an element did not render into a capture");
                }
            }
            frame
                .finish()
                .map_err(|_| anyhow::anyhow!("the capture did not finish"))?
                .wait()
                .map_err(|_| anyhow::anyhow!("waiting for the capture failed"))?;
        }
        let mapping = renderer
            .copy_framebuffer(
                &framebuffer,
                smithay::utils::Rectangle::from_size(buffer_size),
                Fourcc::Xrgb8888,
            )
            .map_err(|err| anyhow::anyhow!("copying the capture back: {err}"))?;
        renderer
            .map_texture(&mapping)
            .map_err(|err| anyhow::anyhow!("mapping the capture: {err}"))?
            .to_vec()
    };
    crate::warp::release_framebuffer(renderer);

    let stride = usize::try_from(size.w * BYTES).unwrap_or_default();
    let rows = usize::try_from(size.h).unwrap_or_default();
    anyhow::ensure!(
        pixels.len() >= stride * rows,
        "the capture is {} bytes, expected {}",
        pixels.len(),
        stride * rows
    );

    smithay::wayland::shm::with_buffer_contents_mut(buffer, |target, len, data| {
        let wanted = stride * rows;
        anyhow::ensure!(
            len >= wanted && data.stride as usize >= stride,
            "the client's buffer is {len} bytes with stride {}, needs {wanted} and {stride}",
            data.stride
        );
        // Row for row, *not* reversed.
        //
        // `capture.rs` reverses, and copying that was the first thing tried
        // here: the result was a desktop upside down, with a bar anchored to
        // the top of the screen sitting along the bottom of the image. The two
        // are not the same read. `capture.rs` reads the winit backend's own
        // framebuffer, which was rendered through the output's
        // `Flipped180` transform and is therefore stored flipped; reversing
        // undoes that. This reads an offscreen texture rendered with
        // `Transform::Normal`, which is already top-down, so reversing adds a
        // flip rather than removing one.
        //
        // A rotated monitor is captured un-rotated for the same reason: the
        // transform belongs to the display pipeline, not to this pass. That is
        // arguably what a screenshot should be — the picture, not the picture
        // as the panel happens to be mounted — and it is why the buffer is
        // sized from the logical geometry.
        for row in 0..rows {
            let from = row * stride;
            let into = row * usize::try_from(data.stride).unwrap_or_default();
            let Some(source) = pixels.get(from..from + stride) else {
                anyhow::bail!("the capture was short a row");
            };
            // SAFETY: `target` is valid for `len` bytes for the duration of
            // this callback, and `into + stride` is within it -- `len` was
            // checked against the client's own stride above. The client may
            // race us on its own buffer, which is its business and cannot be
            // unsound here: this only writes.
            #[expect(unsafe_code, reason = "shm is a raw mapping by nature")]
            unsafe {
                std::ptr::copy_nonoverlapping(source.as_ptr(), target.add(into), stride);
            }
        }
        Ok(())
    })
    .map_err(|err| anyhow::anyhow!("reaching the client's buffer: {err}"))?
    .context("filling the client's buffer")?;

    Ok(())
}
