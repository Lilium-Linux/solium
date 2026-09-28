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
//! * **A window that unmaps itself before it goes** -- attaches no buffer and
//!   commits, and only then destroys `xdg_toplevel` -- leaves nothing, and
//!   vanishes as every window did before #126: smithay's reset drops the
//!   textures at that commit, and `Solium::depart` then finds a window with no
//!   buffer, which to it is one that was never on screen. Fading one would
//!   mean treating an unmap as a close, and an unmapped toplevel may map
//!   again. No client seen does it. Traced nested with `WAYLAND_DEBUG=1`,
//!   GTK 4.22 (a PyGObject window calling `close()`), Qt 6.11 (a PySide6
//!   window calling `close()`) and Firefox 156 (closed with `super+q`) each
//!   destroyed `xdg_toplevel` and `xdg_surface` first, and attached a null
//!   buffer, if at all, only after. `zenity --timeout` exits without tearing
//!   anything down, which is the disconnect above.
//!
//! **And the picture outlives the surface it was read from.** `GlesTexture` is
//! an `Arc` round the GL object (`backend/renderer/gles/texture.rs`), and the
//! texture is deleted only when the last clone is dropped: `Drop for
//! GlesTextureInternal` queues it on the renderer's cleanup channel, which the
//! renderer drains with its context current. smithay's reset drops *its* clone
//! and not ours. A dmabuf-backed texture holds its `EGLImage`s the same way, in
//! the same struct, and `EGL_EXT_image_dma_buf_import` lets the fds behind an
//! image be closed once it exists -- which is what a departed client's are. A
//! shared-memory texture is the compositor's own copy of the pixels, **but not
//! this picture's alone**: smithay keeps one per surface and uploads the
//! surface's next buffer of the same size into it, so a surface the client
//! keeps and gives a new buffer ends the fade ([`Picture::holds`]). None of
//! this can be driven without a GPU, so it is read, not tested.
//!
//! **Every surface of the tree, not the root alone**, topmost first, the order
//! `render_elements_from_surface_tree` draws them in. A browser's page, a video
//! player's frame and a GL client's content are often subsurfaces, and a
//! picture of such a window's root alone is its chrome around a hole. The cost
//! per extra surface is one more handle.
//!
//! **As the tree is when the window goes**, which is not always all of it. A
//! subsurface already unlinked is not in it: one a client takes down before
//! its window on an orderly exit
//! (`a_window_whose_subsurface_goes_first_fades_without_it`). On a disconnect
//! the window is taken at the first of its surfaces, or of their
//! `wl_subsurface` objects, to go, while all of it is there. Ids are recycled,
//! so that can be a subsurface's `wl_surface` older than the window's
//! (`a_client_that_disconnects_keeps_a_subsurface_older_than_its_window`), or
//! a `wl_subsurface` older still, whose destructor unlinks the subsurface and
//! which Solium is asked about first (`Solium::goes_with`,
//! `a_client_that_disconnects_keeps_a_subsurface_whose_wl_subsurface_is_older`).
//! Firefox 156, traced nested, is the first case: its page is a subsurface
//! whose `wl_surface` (#25) is older than the window's (#49) and whose
//! `wl_subsurface` (#55) is younger, and closed, it destroyed `xdg_toplevel`
//! before the subsurface. No client that takes a subsurface down first was
//! found.
//!
//! **What is not in it.** A surface whose current buffer the renderer never
//! imported -- committed after the last frame that drew it, or never drawn on
//! this GPU because it was off every screen -- has no texture to keep; it is
//! kept as a surface with no pixels, and drawn as [`FILL`] where it was. A
//! picture none of whose surfaces has pixels is not drawn at all: the pane
//! falls back to its frame and the fill over its client's rectangle, which is
//! what every test without a renderer gets
//! (`a_window_that_closes_itself_fades_out_and_is_gone_on_time` asserts it).
//!
//! **That is not rare**, and "never imported" undersells it: smithay clears a
//! surface's textures on every new buffer it is given
//! (`RendererSurfaceState::update_buffer`) and imports only when it draws, so
//! a client that commits a last frame and goes before the next frame is drawn
//! -- one that paints and exits inside a frame, one killed mid-animation --
//! leaves no texture for that surface, whatever it showed before.
//!
//! A single-pixel buffer, which the renderer never imports, is kept as the
//! colour smithay's own surface element would draw -- read, not tested.
//! Popups are not in it: they are not in the tree walked.
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
    reexports::wayland_server::{Resource as _, backend::ObjectId},
    utils::{Buffer as BufferCoords, Logical, Physical, Point, Rectangle, Scale, Size, Transform},
    wayland::{
        compositor::{TraversalAction, with_surface_tree_downward},
        seat::WaylandFocus as _,
    },
};

/// The fill a window is drawn with where its client left nothing to draw it
/// from, straight (not premultiplied) RGBA.
///
/// **A mid grey at half opacity, because nothing here knows what was there.**
/// The client's own pixels are what is missing, and there is no channel from a
/// style's QML to the compositor for the theme's colours either. This was
/// `Theme.surface` as shipped -- opaque white -- until #126's review, and on a
/// dark terminal or under a dark theme that is a white rectangle flashed where
/// the window stood, for the first frames of its fade. A grey sits between
/// whatever it replaces and its opposite, and at half opacity what is behind
/// the window shows through it as well; the fade takes it from there.
/// `a_window_that_left_no_picture_is_filled_with_a_translucent_grey`.
///
/// Drawn alone, over the client's rectangle, for a window that left nothing at
/// all ([`crate::pane::Remains::Lost`]), and in the place of each surface of a
/// picture that has no pixels ([`Picture::elements`]): a window whose page
/// survived and whose root did not, or the other way round, would otherwise be
/// its frame and one surface round a hole. The second is not hypothetical:
/// Firefox, closed nested with `WAYLAND_DEBUG=1`, committed its page
/// subsurface's last frame 6 ms before it destroyed its toplevel -- less than
/// a frame at 60 Hz, so a frame drawn in between is not to be counted on.
/// `a_surface_that_left_no_picture_is_filled_where_it_was`.
pub(crate) const FILL: [f32; 4] = [0.5, 0.5, 0.5, 0.5];

/// [`FILL`], premultiplied, which is what a solid draw is given.
fn fill() -> Color32F {
    let [r, g, b, a] = FILL;
    Color32F::new(r * a, g * a, b * a, a)
}

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
    /// for the buffer, and drawn as [`FILL`].
    Unimported,
}

/// One surface of a window, as the renderer last had it.
#[derive(Debug)]
struct Kept {
    /// The element's identity for the damage tracker, one per surface for the
    /// life of the picture, so that a fade redraws what changed rather than the
    /// whole of the window every frame.
    id: Id,
    /// Which surface it was, for [`Picture::holds`]. The id alone, which holds
    /// nothing of the surface: its serial tells a recycled protocol id apart.
    surface: ObjectId,
    /// Whether it is the window's own surface, whose fill is the window's
    /// rectangle rather than the whole buffer: a client that draws its own
    /// shadow draws it outside that rectangle, on the same surface.
    root: bool,
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
                |surface, states, at| {
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
                        surface: surface.id(),
                        root: *surface == *root,
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

    /// Whether it is drawn from this surface.
    ///
    /// For a client that keeps a surface after its window has gone and gives
    /// it a new buffer: one that hides a window and shows it again can map a
    /// new role on the same `wl_surface`. Not seen -- the GTK 4 and Qt 6
    /// windows traced for #126 destroyed their surface a few milliseconds
    /// after their toplevel -- and cheap to answer. smithay's GLES renderer
    /// keeps a shared-memory texture per surface (`import_shm_buffer`'s
    /// `CacheMap`, in the surface's own data, which
    /// `RendererSurfaceState::reset` leaves alone) and uploads the next buffer
    /// of the same size into that same texture, which is the one this picture
    /// holds a handle to: the fade would show the new window's pixels. So a new buffer on a surface this
    /// holds ends the fade there and then (`Solium::let_go_of_reused`). Read
    /// in smithay 0.7.0, where a dmabuf's texture is kept per buffer rather
    /// than per surface, so a new buffer does not touch it; the fade ends all
    /// the same, since which kind a texture is cannot be told from here.
    /// `a_window_that_went_ends_its_fade_when_its_surface_is_given_a_new_buffer`.
    pub(crate) fn holds(&self, surface: &ObjectId) -> bool {
        self.surfaces.iter().any(|kept| kept.surface == *surface)
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

    /// Every surface as an element, topmost first, with the root's top-left
    /// at `origin` and every one of them at `alpha`.
    ///
    /// At the surfaces' own size, like the live client's elements, so the
    /// caller puts them through the same `render::fitted` a live client's
    /// surfaces go through. **A surface with no pixels is drawn as [`FILL`]**,
    /// where it was and as big as it was -- the window's own rectangle for the
    /// root -- and at its place in the stack, so no surface that did survive is
    /// left standing in a hole. Only [`Self::drawable`] pictures are drawn
    /// from; one that is all fill is drawn as the client's rectangle instead.
    #[cfg_attr(
        not(test),
        expect(
            clippy::unnecessary_filter_map,
            reason = "every surface is kept outside a test; under test a dummy texture, \
                      which no GLES frame can draw, is dropped"
        )
    )]
    pub(crate) fn elements(
        &self,
        origin: Point<i32, Physical>,
        scale: f64,
        alpha: f32,
    ) -> impl Iterator<Item = Surface> + '_ {
        let origin = origin.to_f64();
        self.surfaces.iter().filter_map(move |kept| {
            let (pixels, offset, view) = match &kept.pixels {
                Pixels::Texture(texture) => {
                    (Drawn::Texture(texture.clone()), kept.offset, kept.view)
                }
                Pixels::Colour(colour) => (Drawn::Colour(*colour), kept.offset, kept.view),
                #[cfg(test)]
                Pixels::Dummy(_) => return None,
                Pixels::Unimported if kept.root => (
                    Drawn::Fill,
                    self.inset,
                    SurfaceView {
                        src: Rectangle::from_size(self.committed.to_f64()),
                        dst: self.committed,
                        offset: Point::default(),
                    },
                ),
                Pixels::Unimported => (Drawn::Fill, kept.offset, kept.view),
            };
            Some(Surface {
                id: kept.id.clone(),
                location: origin + offset.to_f64().to_physical(scale),
                alpha,
                view,
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

/// What an element is drawn from. [`Pixels`], with the pixels that did not
/// survive drawn as [`FILL`].
#[derive(Debug)]
enum Drawn {
    Texture(GlesTexture),
    Colour(Color32F),
    Fill,
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
    /// Whether this stands in for a surface that left no pixels. Under test,
    /// where it is the one kind of element a picture can be seen to draw.
    #[cfg(test)]
    pub(crate) const fn filled(&self) -> bool {
        matches!(self.pixels, Drawn::Fill)
    }

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
            Drawn::Fill => Frame::draw_solid(frame, dst, damage, fill() * self.alpha),
        }
    }

    /// Never a scanout candidate: it is on its way out, translucent, and its
    /// buffer belongs to a client that has gone.
    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}
