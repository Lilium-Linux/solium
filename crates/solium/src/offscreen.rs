//! Rendering a window to a texture, so it can be deformed as one thing.
//!
//! A warp needs a single texture. Take the client's surface directly and two
//! things go wrong: the frame the compositor drew stays a flat rectangle
//! beside a tilted window, and a window with subsurfaces or a popup comes
//! apart along its own seams, each piece deformed about its own centre.
//!
//! So the window is drawn once, flat, at its real size, into a texture of its
//! own — frame included — and that texture is what gets bent. It costs a pass
//! and a texture per deformed window, which is why only deformed windows pay
//! it.

use std::time::Duration;

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, Color32F, Frame as _, Offscreen, Renderer,
            element::{Element as _, RenderElement},
            gles::{GlesRenderer, GlesTexture},
        },
    },
    desktop::Window,
    utils::{Buffer as BufferCoords, Physical, Rectangle, Scale, Size, Transform},
};

use crate::state::Solium;

/// Draw `window` flat at its real size, frame and all, into a texture.
///
/// Returns the texture and the size it was drawn at, so a caller can map
/// texture coordinates back onto the window's own rectangle.
pub(crate) fn capture(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    window: &Window,
    now: Duration,
    scale: f64,
) -> Option<(GlesTexture, Size<i32, Physical>)> {
    let outer = state.outer_geometry(window)?;
    let size: Size<i32, Physical> = (
        ((f64::from(outer.size.w) * scale).ceil() as i32).max(1),
        ((f64::from(outer.size.h) * scale).ceil() as i32).max(1),
    )
        .into();

    // Built at the origin rather than at the window's position: the texture is
    // the window's own space, and where it ends up on screen is the warp's
    // business.
    let elements = crate::render::flat_window_elements(state, renderer, window, now, scale);
    if elements.is_empty() {
        tracing::warn!("a warped window had nothing to draw offscreen");
        return None;
    }

    // The buffer is measured in buffer pixels, which for an offscreen target
    // are the physical pixels it was asked for. Converting through logical
    // space first — as this did — divides by the scale twice.
    let buffer_size: Size<i32, BufferCoords> = (size.w, size.h).into();
    let mut texture = match renderer.create_buffer(Fourcc::Abgr8888, buffer_size) {
        Ok(texture) => texture,
        Err(err) => {
            tracing::warn!(
                ?err,
                ?buffer_size,
                "no offscreen buffer for a warped window"
            );
            return None;
        }
    };

    {
        let mut framebuffer = match renderer.bind(&mut texture) {
            Ok(framebuffer) => framebuffer,
            Err(err) => {
                tracing::warn!(?err, "could not bind the offscreen buffer");
                return None;
            }
        };
        let mut frame = match renderer.render(&mut framebuffer, size, Transform::Normal) {
            Ok(frame) => frame,
            Err(err) => {
                tracing::warn!(?err, "could not render into the offscreen buffer");
                return None;
            }
        };

        // Transparent, not black: the window's own corners are rounded and
        // anything opaque here would draw a square behind them.
        frame
            .clear(Color32F::TRANSPARENT, &[Rectangle::from_size(size)])
            .ok()?;

        let whole = [Rectangle::from_size(size)];
        for element in &elements {
            let source = element.src();
            let destination = element.geometry(Scale::from(scale));
            // Damage is the whole texture: it was just created, so nothing in
            // it is worth preserving.
            if let Err(err) = element.draw(&mut frame, source, destination, &whole, &[]) {
                tracing::warn!(?err, "a window did not render offscreen");
            }
        }
        // Waited on, not dropped. The fence says when the GPU has actually
        // finished drawing into this texture; sampling it before then is a
        // race that shows up as a window full of garbage, intermittently,
        // which is the worst kind of rendering bug to be handed.
        match frame.finish() {
            Ok(sync) => {
                if let Err(err) = sync.wait() {
                    tracing::warn!(?err, "waiting for the offscreen draw failed");
                    return None;
                }
            }
            Err(err) => {
                tracing::warn!(?err, "the offscreen draw did not finish");
                return None;
            }
        }
    }

    Some((texture, size))
}
