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
    let elements = crate::render::flat_window_elements(state, renderer, window, scale);
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

    // The pass is its own scope so the framebuffer is dropped -- and then
    // released, below -- on every path out of it, drawn or not.
    let drawn = {
        let mut framebuffer = match renderer.bind(&mut texture) {
            Ok(framebuffer) => framebuffer,
            Err(err) => {
                tracing::warn!(?err, "could not bind the offscreen buffer");
                crate::warp::release_framebuffer(renderer);
                return None;
            }
        };
        // The renderer stays borrowed for as long as the frame lives, so the
        // release cannot happen in here; the frame's own scope ends first.
        //
        // Nothing that touches a QML scene may run between here and the end of
        // this scope; see `qml::no_frame_in_flight`.
        let _frame = crate::qml::frame_in_flight();
        match renderer.render(&mut framebuffer, size, Transform::Normal) {
            Err(err) => {
                tracing::warn!(?err, "could not render into the offscreen buffer");
                false
            }
            Ok(mut frame) => {
                // Transparent, not black: the window's own corners are rounded and
                // anything opaque here would draw a square behind them.
                frame
                    .clear(Color32F::TRANSPARENT, &[Rectangle::from_size(size)])
                    .unwrap_or_else(|err| {
                        tracing::warn!(?err, "clearing the offscreen buffer failed")
                    });

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
                    Ok(sync) => match sync.wait() {
                        Ok(()) => true,
                        Err(err) => {
                            tracing::warn!(?err, "waiting for the offscreen draw failed");
                            false
                        }
                    },
                    Err(err) => {
                        tracing::warn!(?err, "the offscreen draw did not finish");
                        false
                    }
                }
            }
        }
    };

    crate::warp::release_framebuffer(renderer);
    drawn.then_some((texture, size))
}

/// One monitor's worth of picture, drawn into a texture of its own.
///
/// The nested backend's multi-monitor mode. On the hardware each output has its
/// own buffer and its own page flip; here there is one window, so each virtual
/// monitor is rendered into its own texture exactly as it would be into its own
/// buffer, and the window draws those side by side.
///
/// That is not a shortcut around the real thing — it is the real thing with the
/// scanout replaced. Every per-output path runs for each of them: its own
/// elements built at its own origin, its own layer map, its own work area. What
/// it cannot simulate is a second *pipeline*, which is `tty.rs`'s half of the
/// problem.
pub(crate) struct Screens {
    /// One texture per monitor, kept across frames. Allocating these per frame
    /// would be several hundred megabytes a second of texture churn on a
    /// development path, and would make the leak check in `dev/leak.sh` read
    /// like a leak.
    textures: Vec<(Size<i32, Physical>, GlesTexture)>,
}

impl std::fmt::Debug for Screens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Screens")
            .field("count", &self.textures.len())
            .finish()
    }
}

impl Screens {
    pub(crate) fn new() -> Self {
        Self {
            textures: Vec::new(),
        }
    }

    /// The texture for monitor `index`, at `size`, creating or replacing it if
    /// the size is not what it was.
    fn texture(
        &mut self,
        renderer: &mut GlesRenderer,
        index: usize,
        size: Size<i32, Physical>,
    ) -> Option<&mut GlesTexture> {
        while self.textures.len() <= index {
            let buffer: Size<i32, BufferCoords> = (size.w.max(1), size.h.max(1)).into();
            let texture = renderer.create_buffer(Fourcc::Abgr8888, buffer).ok()?;
            self.textures.push((size, texture));
        }
        if self
            .textures
            .get(index)
            .is_some_and(|(had, _)| *had != size)
        {
            let buffer: Size<i32, BufferCoords> = (size.w.max(1), size.h.max(1)).into();
            let texture = renderer.create_buffer(Fourcc::Abgr8888, buffer).ok()?;
            *self.textures.get_mut(index)? = (size, texture);
        }
        self.textures.get_mut(index).map(|(_, texture)| texture)
    }

    /// Draw one monitor and hand back its texture.
    pub(crate) fn draw(
        &mut self,
        state: &mut Solium,
        renderer: &mut GlesRenderer,
        prepared: &crate::render::Prepared,
        index: usize,
        screen: Rectangle<i32, smithay::utils::Logical>,
        scale: f64,
    ) -> Option<(GlesTexture, Size<i32, Physical>)> {
        let size: Size<i32, Physical> = (
            ((f64::from(screen.size.w) * scale).ceil() as i32).max(1),
            ((f64::from(screen.size.h) * scale).ceil() as i32).max(1),
        )
            .into();

        // Built before the texture is bound, because building can bind
        // framebuffers of its own -- the warp pass -- and a bind underneath a
        // bind redirects the whole picture. Same reason `Prepared` exists.
        let elements = crate::render::elements(
            state,
            renderer,
            prepared,
            crate::render::Picture::screen(screen, scale),
        );

        let mut texture = self.texture(renderer, index, size)?.clone();
        let drawn = {
            let mut framebuffer = match renderer.bind(&mut texture) {
                Ok(framebuffer) => framebuffer,
                Err(err) => {
                    tracing::warn!(?err, index, "could not bind a monitor's buffer");
                    crate::warp::release_framebuffer(renderer);
                    return None;
                }
            };
            let _frame = crate::qml::frame_in_flight();
            match renderer.render(&mut framebuffer, size, Transform::Normal) {
                Err(err) => {
                    tracing::warn!(?err, index, "could not render a monitor");
                    false
                }
                Ok(mut frame) => {
                    // The same background the backends clear to, so an empty
                    // monitor looks like an empty monitor rather than a hole.
                    frame
                        .clear(
                            Color32F::new(0.05, 0.05, 0.06, 1.0),
                            &[Rectangle::from_size(size)],
                        )
                        .unwrap_or_else(|err| tracing::warn!(?err, "clearing a monitor failed"));

                    let whole = [Rectangle::from_size(size)];
                    // Reversed: this list is topmost-first, and drawing it in
                    // that order onto a cleared buffer paints the top of the
                    // stack first and everything else over it.
                    for element in elements.iter().rev() {
                        let source = element.src();
                        let destination = element.geometry(Scale::from(scale));
                        if let Err(err) = element.draw(&mut frame, source, destination, &whole, &[])
                        {
                            tracing::warn!(?err, index, "an element did not render");
                        }
                    }

                    match frame.finish() {
                        Ok(sync) => match sync.wait() {
                            Ok(()) => true,
                            Err(err) => {
                                tracing::warn!(?err, "waiting for a monitor's draw failed");
                                false
                            }
                        },
                        Err(err) => {
                            tracing::warn!(?err, "a monitor's draw did not finish");
                            false
                        }
                    }
                }
            }
        };

        crate::warp::release_framebuffer(renderer);
        drawn.then_some((texture, size))
    }
}
