//! What a client leaves behind: the pictures its surfaces were last drawn from.
//!
//! A window its own application closes -- `exit`, `Ctrl+D`, an app's Quit, a
//! crash, `kill`, an X11 unmap -- has to fade out like one the compositor
//! closes (issue #126). The compositor's own closes can animate a live client,
//! because they animate first and ask afterwards. A client that goes on its own
//! is gone by the time anyone hears of it, so the fade has to be drawn from
//! something that outlives the client. This is that something.
//!
//! ## Where the picture comes from
//!
//! **What the renderer already imported, at the moment the client goes.** Every
//! surface smithay draws carries a `RendererSurfaceState` holding the texture
//! its current buffer was imported into, per renderer context. Nothing is
//! captured or copied while a window lives: a [`Picture`] is taken once, when
//! the client goes, and it is a handle clone per surface. Read in smithay
//! 0.7.0's source, and pinned where a test can reach it:
//!
//! * **An orderly exit** destroys `xdg_toplevel` before `xdg_surface` and
//!   `wl_surface`, in request order, so `toplevel_destroyed` runs with the
//!   whole surface tree and its state intact. smithay's own reset of that
//!   state is a destruction hook on the `wl_surface`
//!   (`backend/renderer/utils/wayland.rs`, `on_commit_buffer_handler`), which
//!   has not run yet. `an_orderly_exit_leaves_the_picture_the_renderer_imported`.
//! * **A crash, a `kill`, a disconnect** destroys every object the client had,
//!   in ascending object id (`wayland-backend`'s `queue_all_destructors`), and a
//!   `wl_surface` is created before the `xdg_surface` made from it, so the
//!   surface usually goes first. `CompositorHandler::destroyed` is called for it
//!   *before* the surface is unlinked from its tree and before its destruction
//!   hooks run (`wayland/compositor/handlers.rs`, `Dispatch::destroyed` for
//!   `WlSurface`: `state.destroyed(surface)` precedes
//!   `PrivateSurfaceData::cleanup`), so the picture is taken there.
//!   `a_client_that_disconnects_leaves_the_picture_the_renderer_imported`.
//! * **An X11 unmap** is taken in `unmapped_window`, or at the `wl_surface`'s
//!   destruction if Xwayland tears the surface down first; whichever comes
//!   first takes it. No test here has an X server; an `xmessage -timeout`
//!   closing itself in a nested session was seen to fade from its picture.
//!
//! **And the picture outlives the surface it was read from.** `GlesTexture` is
//! an `Arc` round the GL object (`backend/renderer/gles/texture.rs`), and the
//! texture is deleted only when the last clone is dropped: `Drop for
//! GlesTextureInternal` queues it on the renderer's cleanup channel, which the
//! renderer drains with its context current. smithay's reset drops *its* clone
//! and not ours. A dmabuf-backed texture holds its `EGLImage`s the same way, in
//! the same struct, and `EGL_EXT_image_dma_buf_import` lets the fds behind an
//! image be closed once it exists -- which is what a departed client's are. A
//! shared-memory texture is the compositor's own copy of the pixels. None of
//! this can be driven without a GPU, so it is read, not tested.
//!
//! **Every surface of the tree, not the root alone**, topmost first, the order
//! `render_elements_from_surface_tree` draws them in. A browser's page, a video
//! player's frame and a GL client's content are often subsurfaces, and a
//! picture of such a window's root alone is its chrome around a hole. The cost
//! per extra surface is one more handle.
//!
//! **What is not in it.** A surface whose current buffer the renderer never
//! imported -- committed after the last frame that drew it, or never drawn on
//! this GPU because it was off every screen -- has no texture to keep; it is
//! kept as a surface with no pixels, and drawn as nothing. A picture none of
//! whose surfaces has pixels is not drawn at all: the pane falls back to its
//! frame and a fill (see [`FILL`]), which is what every test without a
//! renderer gets (`a_window_that_closes_itself_fades_out_and_is_gone_on_time`
//! asserts it). A single-pixel buffer, which the renderer never imports, is
//! kept as the colour smithay's own surface element would draw -- read, not
//! tested. Popups are not in it: they are not in the tree walked.
//!
//! ## What the buffers are held for
//!
//! Each surface's `Buffer` is held with its picture, which is what delays the
//! `wl_buffer.release` a client is owed until the picture is dropped. A buffer
//! a client has been told it may reuse is one it may draw into again, and for a
//! dmabuf that is the memory the picture is sampled from. A client that has
//! gone is told nothing; one that closed a window and stayed is told when the
//! fade is over (`a_window_that_closed_itself_keeps_its_last_buffer_until_its_fade_is_over`).
//! A window that left no pixels keeps no picture, and so no buffer.
use smithay::{
    backend::renderer::{
        Color32F, ContextId, Frame,
        element::{Element, Id, Kind, RenderElement, UnderlyingStorage},
        gles::{GlesError, GlesFrame, GlesRenderer, GlesTexture},
        utils::{Buffer, CommitCounter, RendererSurfaceStateUserData, SurfaceView},
    },
    desktop::Window,
    utils::{Buffer as BufferCoords, Logical, Physical, Point, Rectangle, Scale, Size, Transform},
    wayland::{
        compositor::{TraversalAction, with_surface_tree_downward},
        seat::WaylandFocus as _,
    },
};

/// The fill a window is drawn with when nothing of its client survived.
///
/// `Theme.surface` as `qml/Solium/Theme.qml` ships it, which is the colour the
/// default `top` style's bar is painted in: the window reads as its frame
/// grown over the place its client was. **The shipped value, not the theme's**:
/// there is no channel from a style's QML to the compositor for a colour, and a
/// theme dropped into the user's directory is not consulted. Only a window
/// whose client left no picture at all is drawn with it.
pub(crate) const FILL: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// Which renderer's imports a picture is read from.
///
/// The context a texture was imported under is the key `RendererSurfaceState`
/// files it by, so the compositor keeps the one its renderer has. A second
/// kind exists only under test, for smithay's `DummyRenderer`: it imports
/// shared memory with no GPU, which is what lets a test see a picture taken
/// through the real handlers.
#[derive(Clone, Debug)]
pub(crate) enum Textures {
    Gles(ContextId<GlesTexture>),
    #[cfg(test)]
    Dummy(ContextId<smithay::backend::renderer::test::DummyTexture>),
}

/// What one surface is drawn from.
#[derive(Clone, Debug)]
enum Pixels {
    Texture(GlesTexture),
    /// Only under test. See [`Textures::Dummy`].
    #[cfg(test)]
    Dummy(smithay::backend::renderer::test::DummyTexture),
    /// A single-pixel buffer: a colour, which the renderer never imports.
    Colour(Color32F),
    /// A buffer the renderer never imported. Kept for its place in the tree and
    /// for the buffer, and drawn as nothing.
    Unimported,
}

/// One surface of a window, as the renderer last had it.
#[derive(Debug)]
struct Kept {
    /// The element's identity for the damage tracker, one per surface for the
    /// life of the picture, so that a fade redraws what changed rather than the
    /// whole of the window every frame.
    id: Id,
    /// Where the surface's top-left is, from the root surface's.
    offset: Point<i32, Logical>,
    view: SurfaceView,
    scale: i32,
    transform: Transform,
    /// The buffer's own size, in the surface's logical pixels.
    size: Size<i32, Logical>,
    pixels: Pixels,
    /// Held, not read. See the module docs.
    _buffer: Buffer,
}

/// A window's surfaces, as the renderer had them when its client went.
#[derive(Debug)]
pub(crate) struct Picture {
    /// Topmost first.
    surfaces: Vec<Kept>,
    /// The size the client last committed: what `render::place_client` fits
    /// the picture to, as it fits a live client's buffer.
    committed: Size<i32, Logical>,
    /// Where the window's own rectangle starts inside its buffer --
    /// `Window::geometry().loc`, a drop shadow's width for a client that draws
    /// its own decorations.
    inset: Point<i32, Logical>,
}

impl Picture {
    /// Take the picture of `window`'s surfaces, as imported under `textures`.
    ///
    /// Walks the tree the way `render_elements_from_surface_tree` does, so the
    /// surfaces come out in the order and at the offsets they are drawn at. A
    /// surface with no view or no buffer is not mapped, and neither it nor
    /// anything under it is kept -- the same rule the renderer applies.
    pub(crate) fn of(window: &Window, textures: Option<&Textures>) -> Self {
        #[cfg(test)]
        TAKEN.with(|taken| taken.set(taken.get() + 1));
        let geometry = window.geometry();
        let mut surfaces = Vec::new();
        if let Some(root) = window.wl_surface() {
            with_surface_tree_downward(
                &root,
                Point::<i32, Logical>::default(),
                |_, states, at| {
                    let view = states
                        .data_map
                        .get::<RendererSurfaceStateUserData>()
                        .and_then(|data| data.lock().ok().and_then(|data| data.view()));
                    match view {
                        Some(view) => TraversalAction::DoChildren(*at + view.offset),
                        None => TraversalAction::SkipChildren,
                    }
                },
                |_, states, at| {
                    let Some(data) = states.data_map.get::<RendererSurfaceStateUserData>() else {
                        return;
                    };
                    let Ok(data) = data.lock() else {
                        return;
                    };
                    let (Some(view), Some(buffer), Some(size)) =
                        (data.view(), data.buffer().cloned(), data.buffer_size())
                    else {
                        return;
                    };
                    let pixels = if let Ok(colour) =
                        smithay::wayland::single_pixel_buffer::get_single_pixel_buffer(&buffer)
                    {
                        Pixels::Colour(Color32F::from(colour.rgba32f()))
                    } else {
                        match textures {
                            Some(Textures::Gles(context)) => data
                                .texture(context.clone())
                                .cloned()
                                .map_or(Pixels::Unimported, Pixels::Texture),
                            #[cfg(test)]
                            Some(Textures::Dummy(context)) => data
                                .texture(context.clone())
                                .cloned()
                                .map_or(Pixels::Unimported, Pixels::Dummy),
                            None => Pixels::Unimported,
                        }
                    };
                    surfaces.push(Kept {
                        id: Id::new(),
                        offset: *at + view.offset,
                        view,
                        scale: data.buffer_scale(),
                        transform: data.buffer_transform(),
                        size,
                        pixels,
                        _buffer: buffer,
                    });
                },
                |_, _, _| true,
            );
        }
        Self {
            surfaces,
            committed: geometry.size,
            inset: geometry.loc,
        }
    }

    /// Whether any of it can be drawn.
    ///
    /// Under test a dummy texture counts, because it stands for exactly what a
    /// GLES texture would be on the hardware.
    pub(crate) fn drawable(&self) -> bool {
        self.surfaces
            .iter()
            .any(|kept| !matches!(kept.pixels, Pixels::Unimported))
    }

    /// The size the client last committed.
    pub(crate) const fn committed(&self) -> Size<i32, Logical> {
        self.committed
    }

    /// Where the window's rectangle starts inside its buffer.
    pub(crate) const fn inset(&self) -> Point<i32, Logical> {
        self.inset
    }

    /// Every drawable surface as an element, topmost first, with the root's
    /// top-left at `origin` and every one of them at `alpha`.
    ///
    /// At the surfaces' own size, like the live client's elements, so the
    /// caller puts them through the same `render::fitted` a live client's
    /// surfaces go through.
    pub(crate) fn elements(
        &self,
        origin: Point<i32, Physical>,
        scale: f64,
        alpha: f32,
    ) -> impl Iterator<Item = Surface> + '_ {
        let origin = origin.to_f64();
        self.surfaces.iter().filter_map(move |kept| {
            let pixels = match &kept.pixels {
                Pixels::Texture(texture) => Drawn::Texture(texture.clone()),
                Pixels::Colour(colour) => Drawn::Colour(*colour),
                #[cfg(test)]
                Pixels::Dummy(_) => return None,
                Pixels::Unimported => return None,
            };
            Some(Surface {
                id: kept.id.clone(),
                location: origin + kept.offset.to_f64().to_physical(scale),
                alpha,
                view: kept.view,
                scale: kept.scale,
                transform: kept.transform,
                size: kept.size,
                pixels,
            })
        })
    }

    /// The textures under test, as `(width, height)`, topmost first.
    #[cfg(test)]
    pub(crate) fn dummy_sizes(&self) -> Vec<(u32, u32)> {
        use smithay::backend::renderer::Texture as _;
        self.surfaces
            .iter()
            .filter_map(|kept| match &kept.pixels {
                Pixels::Dummy(texture) => Some((texture.width(), texture.height())),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
thread_local! {
    /// How many pictures this thread has taken. Per thread, because tests run
    /// on threads of their own and each counts only its own compositor's.
    static TAKEN: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many pictures this thread has taken, for
/// `nothing_is_kept_of_a_window_that_stays`.
#[cfg(test)]
pub(crate) fn taken() -> usize {
    TAKEN.with(std::cell::Cell::get)
}

/// What an element is drawn from. [`Pixels`] less the cases that draw nothing.
#[derive(Debug)]
enum Drawn {
    Texture(GlesTexture),
    Colour(Color32F),
}

/// One kept surface, placed for one frame.
///
/// `WaylandSurfaceRenderElement` cannot be built for a surface that has gone:
/// it reads the surface's state as it is built. This is the same element drawn
/// from what was kept of that state, with the same arithmetic for its size and
/// its source rectangle (`backend/renderer/element/surface.rs` in smithay
/// 0.7.0), so a picture is drawn exactly where its surfaces were.
#[derive(Debug)]
pub(crate) struct Surface {
    id: Id,
    location: Point<f64, Physical>,
    alpha: f32,
    view: SurfaceView,
    scale: i32,
    transform: Transform,
    size: Size<i32, Logical>,
    pixels: Drawn,
}

impl Surface {
    fn physical_size(&self, scale: Scale<f64>) -> Size<i32, Physical> {
        ((self.view.dst.to_f64().to_physical(scale).to_point() + self.location).to_i32_round()
            - self.location.to_i32_round())
        .to_size()
    }
}

impl Element for Surface {
    fn id(&self) -> &Id {
        &self.id
    }

    /// Never changes: nothing will ever commit to what a picture holds. What
    /// does change frame to frame -- where it is and how opaque -- the damage
    /// tracker reads from `geometry` and `alpha`.
    fn current_commit(&self) -> CommitCounter {
        CommitCounter::default()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        Rectangle::new(self.location.to_i32_round(), self.physical_size(scale))
    }

    fn src(&self) -> Rectangle<f64, BufferCoords> {
        self.view
            .src
            .to_buffer(f64::from(self.scale), self.transform, &self.size.to_f64())
    }

    fn transform(&self) -> Transform {
        self.transform
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for Surface {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        // Through the `Frame` trait, which is what smithay's own surface
        // element draws with; `GlesFrame` has inherent methods of the same
        // names that take a shader program as well.
        match &self.pixels {
            Drawn::Texture(texture) => Frame::render_texture_from_to(
                frame,
                texture,
                src,
                dst,
                damage,
                opaque_regions,
                self.transform,
                self.alpha,
            ),
            Drawn::Colour(colour) => Frame::draw_solid(frame, dst, damage, *colour * self.alpha),
        }
    }

    /// Never a scanout candidate: it is on its way out, translucent, and its
    /// buffer belongs to a client that has gone.
    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}
