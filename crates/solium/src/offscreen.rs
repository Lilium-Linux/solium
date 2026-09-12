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
//!
//! The pass is per frame and the texture is not. A window's size does not
//! change because it is being warped — `present.rs`'s first rule — so the
//! texture is made once, kept on the pane and drawn into again on every frame
//! of the animation. See [`Scratch`], which is the same argument `Screens`
//! makes forty lines further down and which was never applied here.

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
    utils::{Buffer as BufferCoords, Logical, Physical, Rectangle, Scale, Size, Transform},
};

use crate::{pane::PaneId, qml::paint::Kept, state::Solium};

/// How many textures one pane keeps for its own captures.
///
/// **One, because a pane is captured once a frame at one size.**
/// `render::prepare` walks the panes once per frame and calls [`capture`] at
/// most once for each, at that pane's own monitor's scale. There is never a
/// second size live to alternate with — which is exactly the case
/// `qml::paint`'s cap of two does exist for: one scene drawn on both sides of
/// a bezel, once per output, at two scales, every frame.
///
/// The arithmetic says the same from the other side. A capture of an ordinary
/// 1150x850 window is 1150 x 850 x 4 = 3.9 MB, and 2300 x 1700 x 4 = 15.6 MB
/// of it on a 2x monitor. One per *warped* pane is what every mode that
/// deforms windows holds at once, and they all deform every window on screen:
/// an overview of twenty windows is 78 MB, or 313 MB at 2x. A cap of two would
/// be 156 MB and 626 MB for a second entry nothing can ever ask for.
///
/// What it costs when the size does change — a client resizing mid-warp, a
/// window crossing onto a monitor at another scale — is today's behaviour and
/// nothing worse: one allocation, exactly as before this existed.
const KEPT: usize = 1;

/// The texture a pane's captures are drawn into, kept across frames.
///
/// **A buffer, not a picture.** [`capture`] clears and redraws the whole of it
/// on every frame, so what is reused is the allocation and never the image.
/// There is therefore no invalidation to get wrong: a window whose client is
/// painting, or whose title just changed, is as correct through this as it was
/// without it. That is the difference between this and `qml::paint`'s [`Kept`],
/// which keeps a *finished picture* and has to be told when Qt has a new one.
///
/// It is also why the key is the pixel size alone. `Drawn` carries the scale as
/// well, because the host is handed both and one buffer size holds two
/// different pictures at two scales; here it holds no picture at all, and a
/// buffer of the right number of pixels is the right buffer whatever last drew
/// into it. The scale reaches the key anyway, through the size it multiplies.
///
/// **Owned by the [`crate::pane::Pane`], as a field.** Not a
/// `HashMap<PaneId, _>` beside the panes: five such tables were deleted the
/// change before this one because they had to be reconciled by hand and two of
/// them could disagree, and 3.9 MB that has to be swept is a worse thing to
/// leave behind than a stale boolean.
///
/// Generic over what it keeps only so the policy can be tested without a GPU —
/// [`Kept`]'s own reason, and the only reason there is any coverage of this at
/// all. `cargo test` runs in a container with no render node (see
/// `dev/gate.sh`), so nothing in a test can hold a real `GlesTexture`.
#[derive(Debug)]
pub(crate) struct Scratch<T = GlesTexture> {
    kept: Kept<Size<i32, Physical>, T>,
}

impl<T> Default for Scratch<T> {
    fn default() -> Self {
        Self {
            kept: Kept::keeping(KEPT),
        }
    }
}

impl<T: Clone> Scratch<T> {
    /// The texture to draw a capture of `size` into, making one only when what
    /// is in hand is the wrong size.
    ///
    /// `make` is the allocation this whole change exists to stop doing every
    /// frame. It is a closure rather than the `&mut GlesRenderer` it wraps so
    /// that the two lines deciding whether to call it can be *counted* in a
    /// test, which is the only way this path is observable without a GPU.
    fn texture<E>(
        &mut self,
        size: Size<i32, Physical>,
        make: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        if let Some(held) = self.kept.get(size) {
            return Ok(held.clone());
        }
        let made = make()?;
        // Kept as well as handed out, not instead of: a `GlesTexture` is an
        // `Arc`, so this is a refcount rather than a second buffer, and the
        // caller's handle going away at the end of the frame is what makes
        // the cache the only thing still holding it.
        self.kept.push(size, made.clone());
        Ok(made)
    }

    /// Give back whatever is being kept.
    ///
    /// A pane that is not being warped this frame has no use for megabytes of
    /// texture, and there is no later frame on which handing it back gets
    /// cheaper — so a mode that warps every window on screen and is then left
    /// would otherwise leave one behind per window for the rest of the
    /// session. Called by `render::prepare` for every pane it does not
    /// capture, which is all of them on an ordinary desktop.
    ///
    /// Replacing the [`Kept`] rather than emptying it, because there is no
    /// `clear` and `current(_, true)` is the *invalidation* `qml::paint` needs
    /// rather than this. Either way the values are dropped, which is the half
    /// that frees anything: the `GlesTexture` is the last thing holding the
    /// `EGLImage`, which holds EGL's reference on the buffer — `Kept::push`
    /// sets that chain out in full for eviction and it is the same chain here.
    pub(crate) fn release(&mut self) {
        self.kept = Kept::keeping(KEPT);
    }
}

/// The pixel size a window's capture needs, at `scale`.
///
/// **Read from the window's own geometry and from nothing the warp is doing**,
/// which is the whole reason a cache keyed on it ever hits. `present.rs`'s
/// first rule is that a transform never changes real geometry: the matrix, the
/// deform and the animated `Frame::rect` are applied to this texture
/// afterwards, by `warp::mesh`, and not one of them is read here. So a window
/// bending through a genie is captured at the same size on every frame of it.
///
/// The sizes that do change it are all one-offs — the client actually resizing,
/// its frame's insets changing, the window crossing onto a monitor at another
/// scale — and each costs exactly the one allocation it used to cost every
/// frame.
fn pixels(outer: Size<i32, Logical>, scale: f64) -> Size<i32, Physical> {
    (
        ((f64::from(outer.w) * scale).ceil() as i32).max(1),
        ((f64::from(outer.h) * scale).ceil() as i32).max(1),
    )
        .into()
}

/// Draw `window` flat at its real size, frame and all, into a texture.
///
/// Returns the texture and the size it was drawn at, so a caller can map
/// texture coordinates back onto the window's own rectangle.
///
/// The texture belongs to `pane` and outlives the frame; see [`Scratch`]. The
/// pane is passed in rather than looked up with `Panes::id_of`, because the
/// one caller is iterating panes and already holds it — and because a capture
/// with nowhere to keep its texture would be back to allocating one a frame,
/// which is a case worth not having rather than one worth handling.
pub(crate) fn capture(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    pane: PaneId,
    window: &Window,
    scale: f64,
) -> Option<(GlesTexture, Size<i32, Physical>)> {
    let outer = state.outer_geometry(window)?;
    let size = pixels(outer.size, scale);

    // Built at the origin rather than at the window's position: the texture is
    // the window's own space, and where it ends up on screen is the warp's
    // business.
    let elements = crate::render::flat_window_elements(state, renderer, window, scale);
    if elements.is_empty() {
        tracing::warn!("a warped window had nothing to draw offscreen");
        return None;
    }

    // The pane's own texture, made once and then reused for as long as the
    // window stays this size. `state` and `renderer` are separate borrows --
    // the renderer is not reached through the state -- so the closure can hold
    // one while the pane holds the other.
    let mut texture = {
        let scratch = state.panes.get_mut(pane)?.scratch_mut();
        // The buffer is measured in buffer pixels, which for an offscreen
        // target are the physical pixels it was asked for. Converting through
        // logical space first — as this did — divides by the scale twice.
        let buffer_size: Size<i32, BufferCoords> = (size.w, size.h).into();
        match scratch.texture(size, || {
            renderer.create_buffer(Fourcc::Abgr8888, buffer_size)
        }) {
            Ok(texture) => texture,
            Err(err) => {
                tracing::warn!(
                    ?err,
                    ?buffer_size,
                    "no offscreen buffer for a warped window"
                );
                return None;
            }
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
                    // Damage is the whole texture, and stays so now that the
                    // texture is reused: the clear above threw away everything
                    // the last frame left in it, so there is nothing to
                    // preserve whether or not this buffer is new.
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
    ///
    /// The same reasoning, and for a long time the only place it was written
    /// down: [`Scratch`] is it applied to the per-window path, where there is
    /// one per warped window rather than one per monitor.
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
        screen: Rectangle<i32, Logical>,
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

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use smithay::utils::{Logical, Physical, Size};

    use super::{KEPT, Scratch, pixels};

    /// An ordinary window, the same one `qml::paint`'s tests measure and the
    /// one both caps are argued from: 1150 x 850 x 4 = 3.9 MB.
    fn window() -> Size<i32, Logical> {
        (1150, 850).into()
    }

    /// What a texture costs, at four bytes a pixel.
    fn bytes(size: Size<i32, Physical>) -> i64 {
        i64::from(size.w) * i64::from(size.h) * 4
    }

    /// A GBM buffer, which is freed when the last handle on it goes.
    ///
    /// Modelled rather than counted directly, because that is the shape of the
    /// thing: a `GlesTexture` is an `Arc<GlesTextureInternal>`, `capture` hands
    /// one out and [`Scratch`] keeps another, and the memory comes back when
    /// both are gone. A counter on the *handle* would say two buffers were
    /// freed where there was one.
    struct Buffer(Rc<Cell<u32>>);
    impl Drop for Buffer {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    impl std::fmt::Debug for Buffer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("Buffer")
        }
    }

    /// A handle on one. Cloning is a refcount, exactly as `GlesTexture`'s is.
    #[derive(Clone, Debug)]
    struct Handle(Rc<Buffer>);
    impl Handle {
        fn on(freed: &Rc<Cell<u32>>) -> Self {
            Self(Rc::new(Buffer(Rc::clone(freed))))
        }

        /// How many handles name this buffer.
        fn handles(&self) -> usize {
            Rc::strong_count(&self.0)
        }
    }

    /// **A genie costs one texture, not one a frame.**
    ///
    /// The defect this exists for. `render::prepare` calls `capture` once per
    /// warped pane per frame and `capture` called `create_buffer` every time,
    /// unconditionally — forty lines above a comment saying that allocating a
    /// texture per frame is hundreds of megabytes a second, which was written
    /// about `Screens` and never applied to the window path beside it. At the
    /// 3.9 MB of an ordinary window, a 60-frame genie was 234 MB of GBM
    /// churned for one animation of one window, and every mode that deforms
    /// windows deforms *every* window on screen.
    ///
    /// The genie here is real rather than decorative, because the question
    /// worth asking about a cache keyed on size is whether a warped window
    /// holds still long enough to hit it. It does, and the two assertions are
    /// the two halves of why: the window is measurably bending, and the size
    /// `capture` reads never moves. A transform is applied to this texture
    /// afterwards, by `warp::mesh`; `present.rs`'s first rule is that it never
    /// changes the geometry the texture is sized from.
    ///
    /// Arithmetic, and it needs neither Qt nor a GPU — which is the only
    /// reason there is any coverage of this at all. `cargo test` runs in a
    /// container with no render node, so no test can hold a real
    /// `GlesTexture`, and `dev/wirecheck` does not link this crate.
    #[test]
    fn a_genie_costs_one_texture_and_not_one_a_frame() {
        const FRAMES: u16 = 60;

        let outer = window();
        let rect =
            crate::present::for_effects(crate::present::logical((0.0, 0.0), (1150.0, 850.0)));
        let slot =
            crate::present::for_effects(crate::present::logical((40.0, 1000.0), (64.0, 32.0)));

        let mut scratch: Scratch<u32> = Scratch::default();
        let mut allocations = 0_u32;
        let mut bottom_edge = Vec::new();

        for step in 0..=FRAMES {
            let deform = solium_effects::Deform::Genie {
                progress: f32::from(step) / f32::from(FRAMES),
                spread: 0.5,
                axis: solium_effects::Axis::Down,
            };
            // Where the middle of the window's bottom edge is drawn this
            // frame. This is what a warp changes.
            bottom_edge.push(deform.place(rect, slot, 0.5, 1.0));

            // And this is what `capture` asks for, which is not that.
            let size = pixels(outer, 1.0);
            let _texture = scratch
                .texture(size, || {
                    allocations += 1;
                    Ok::<u32, ()>(allocations)
                })
                .expect("a counting allocator cannot fail");
        }

        assert_ne!(
            bottom_edge.first(),
            bottom_edge.last(),
            "the window never moved, so this proves nothing about a warp"
        );
        assert_eq!(
            allocations,
            1,
            "a texture per frame of the genie: {} allocations, {} MB",
            FRAMES,
            i64::from(FRAMES) * bytes(pixels(outer, 1.0)) / 1_000_000
        );
    }

    /// **The cap is one, and a resize replaces rather than joins.**
    ///
    /// One because a pane is captured once a frame at one size: `prepare`
    /// walks the panes once and calls `capture` at most once for each, at that
    /// pane's own monitor's scale. Nothing here alternates, which is the case
    /// `qml::paint`'s cap of two exists for — one scene drawn on both sides of
    /// a bezel, once per output, every frame.
    ///
    /// A second entry would be 3.9 MB per warped pane that nothing can ever
    /// ask for: 78 MB across the twenty windows an overview warps at once, and
    /// 313 MB of it on a 2x monitor.
    ///
    /// And the eviction has to *free*, or the cap is a number with nothing
    /// under it. The chain is the one `Kept::push` sets out: dropping the
    /// entry drops the `GlesTexture`, which is the last thing holding the
    /// `EGLImage`, which holds EGL's reference on the buffer.
    #[test]
    fn a_resized_window_replaces_its_texture_rather_than_keeping_both() {
        assert_eq!(KEPT, 1, "the arithmetic in this test is the cap's");

        let freed = Rc::new(Cell::new(0));
        let mut scratch: Scratch<Handle> = Scratch::default();

        // A window being dragged wider, a pixel at a time, while it is warped.
        for width in [1150, 1151, 1152] {
            let size = pixels((width, 850).into(), 1.0);
            let _texture = scratch
                .texture(size, || Ok::<Handle, ()>(Handle::on(&freed)))
                .expect("a counting allocator cannot fail");
        }
        assert_eq!(
            freed.get(),
            2,
            "the cache held every size it was ever asked for"
        );

        // The one it is still holding is the last, not the first.
        let mut more = 0_u32;
        let size = pixels((1152, 850).into(), 1.0);
        let texture = scratch
            .texture(size, || {
                more += 1;
                Ok::<Handle, ()>(Handle::on(&freed))
            })
            .expect("a counting allocator cannot fail");
        assert_eq!(more, 0, "the size it was last asked for was not kept");

        // Both handles, in the order the compositor lets them go: the caller's
        // at the end of the frame, the cache's when the pane does.
        drop(texture);
        assert_eq!(freed.get(), 2, "the cache stopped holding the texture");
        drop(scratch);
        assert_eq!(freed.get(), 3, "dropping the pane kept its texture alive");
    }

    /// **A pane that stops warping hands its texture back.**
    ///
    /// Without this the bound is "one texture per pane that has *ever* been
    /// warped", which on a desktop where overview has been opened once is
    /// every window on it — 78 MB held for the rest of the session with
    /// nothing on screen to show for it. `render::prepare` calls this for
    /// every pane it does not capture, which is all of them on a still screen.
    #[test]
    fn a_pane_that_stops_warping_gives_the_texture_back() {
        let freed = Rc::new(Cell::new(0));
        let mut scratch: Scratch<Handle> = Scratch::default();
        let size = pixels(window(), 1.0);

        let texture = scratch
            .texture(size, || Ok::<Handle, ()>(Handle::on(&freed)))
            .expect("a counting allocator cannot fail");
        assert_eq!(freed.get(), 0);
        // Two handles on one buffer while the capture is in flight: the
        // cache's and this caller's. That is the whole of why the cache keeps
        // a clone rather than the value, and why releasing it frees anything.
        assert_eq!(texture.handles(), 2, "the cache did not keep a handle");
        drop(texture);

        scratch.release();
        assert_eq!(
            freed.get(),
            1,
            "a pane that is not being warped kept its texture"
        );

        // And it is a release rather than a poisoning: the next warp allocates
        // again instead of getting nothing.
        let mut again = 0_u32;
        let _texture = scratch
            .texture(size, || {
                again += 1;
                Ok::<Handle, ()>(Handle::on(&freed))
            })
            .expect("a counting allocator cannot fail");
        assert_eq!(again, 1, "a released pane could not be warped again");
    }

    /// **The scale is not separately part of the key, and does not need to be.**
    ///
    /// `qml::paint`'s `Drawn` carries the pixels *and* the scale because one
    /// buffer size holds two different pictures at two scales — the host is
    /// handed both and lays the scene out from the pair. What [`Scratch`]
    /// keeps is not a picture: `capture` clears and redraws the whole texture
    /// on every frame, so a buffer with the right number of pixels is the
    /// right buffer whatever last drew into it.
    ///
    /// The scale still decides the key, through the size it multiplies, which
    /// is what makes a window crossing to a 2x monitor a miss rather than a
    /// window drawn at half its resolution.
    #[test]
    fn a_window_crossing_to_a_2x_monitor_asks_for_a_different_buffer() {
        let outer = window();
        assert_eq!(pixels(outer, 1.0), Size::from((1150, 850)));
        assert_eq!(pixels(outer, 2.0), Size::from((2300, 1700)));
        // A fractional scale rounds up, so the texture is never short of a row.
        assert_eq!(pixels(outer, 1.5), Size::from((1725, 1275)));

        // The numbers every cap above is argued from.
        assert_eq!(bytes(pixels(outer, 1.0)), 3_910_000);
        assert_eq!(bytes(pixels(outer, 2.0)), 15_640_000);

        let mut scratch: Scratch<u32> = Scratch::default();
        let mut allocations = 0_u32;
        for scale in [1.0, 1.0, 2.0, 2.0] {
            let _texture = scratch
                .texture(pixels(outer, scale), || {
                    allocations += 1;
                    Ok::<u32, ()>(allocations)
                })
                .expect("a counting allocator cannot fail");
        }
        assert_eq!(
            allocations, 2,
            "the two monitors did not each get a buffer of their own"
        );
    }

    /// A window of no size is still given a texture rather than a zero-sized
    /// allocation the driver would refuse.
    #[test]
    fn a_window_with_no_size_still_asks_for_a_pixel() {
        assert_eq!(pixels((0, 0).into(), 1.0), Size::from((1, 1)));
        assert_eq!(pixels((1, 1).into(), 0.1), Size::from((1, 1)));
    }
}
