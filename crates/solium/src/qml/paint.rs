//! The compositor's half of a GPU scene: what Qt drew, as something to draw.
//!
//! Qt renders into a dmabuf we allocated; this is everything between that and a
//! render element. It is one module rather than a copy per scene owner because
//! it is not a shape — it is an *order*, and the order is not optional:
//!
//!   render → put our EGL context back → wait on Qt's fence → import the dmabuf
//!
//! Getting it wrong is silent in every direction. Skip the restore and the next
//! EGL call of ours runs against Qt's context; skip the fence and the
//! compositor samples a buffer that is still being written, which appears as
//! garbage on perhaps one frame in several hundred. Neither returns an error
//! from anything.
//!
//! The wallpaper, the panels, the window frames and the pointer all need it,
//! and three of them arrived after the first. Writing it out per caller is how
//! the bug this module's task exists to fix happened in the first place: three
//! sites each deciding for themselves, and two of them deciding wrong.

use std::os::fd::OwnedFd;

use anyhow::{Result, anyhow};
use smithay::{
    backend::{
        egl::fence::EGLFence,
        renderer::{
            ImportDma as _, Renderer as _,
            element::{Id, Kind, texture::TextureRenderElement},
            gles::{GlesRenderer, GlesTexture},
            sync::SyncPoint,
            utils::DamageBag,
        },
    },
    utils::{Buffer as BufferCoords, Logical, Rectangle, Size, Transform},
};

use super::Scene;

/// Where a scene's picture goes on screen.
///
/// One argument rather than four: they are only meaningful together, and a
/// fourth positional `f64` on a call that already takes a size and a scale is
/// the kind of thing that gets passed in the wrong order once and stays wrong.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Placement {
    /// The top-left corner, in **physical** pixels.
    pub(crate) position: (f64, f64),
    /// The **logical** size it is drawn at, which the output scale then takes
    /// back up to real pixels.
    pub(crate) size: Size<i32, Logical>,
    /// The window's opacity, not the scene's. See `Decoration::frame`.
    pub(crate) alpha: f32,
    /// `Kind::Cursor` is what lets the DRM backend put an element on the
    /// hardware cursor plane; everything else is `Kind::Unspecified`.
    pub(crate) kind: Kind,
}

/// One scene's buffer, as the compositor sees it.
///
/// Owns the imported texture and the damage that goes with it, and nothing
/// else: the buffer itself belongs to the [`Scene`], which is the arrangement
/// that makes the whole path safe — the thing being read cannot outlive the
/// thing drawing into it.
#[derive(Debug)]
pub(crate) struct Gpu {
    /// The dmabuf imported as a texture, kept so an idle frame costs no import.
    texture: Option<GlesTexture>,
    /// Stable for the life of the scene, so the damage tracker sees one element
    /// moving and changing rather than a new one every frame.
    id: Id,
    /// What of the imported texture has changed, in its own pixels.
    ///
    /// The GPU path's answer to a question the memory path never has to ask.
    /// Qt draws into the same buffer every frame, so nothing about the texture
    /// says whether it holds anything new; without this the element is either
    /// permanently undamaged — a scene frozen on its first frame — or
    /// permanently new, which repaints its whole area on every frame anything
    /// else draws.
    damage: DamageBag<i32, BufferCoords>,
    /// The pixel size of the buffer the scene is **actually** on.
    ///
    /// Not the size the caller last asked for. After a rebind that failed the
    /// two differ, and this is the one the texture is: see [`Gpu::element`].
    size: (i32, i32),
    /// Whether the last rebind failed and has already been reported.
    ///
    /// A rebind that fails once fails every frame after — the size it is
    /// retried at does not change — so without this a driver that will not
    /// allocate writes one warning per scene per frame for the rest of the
    /// session.
    stale: bool,
}

impl Gpu {
    /// A scene whose buffer is `size` pixels.
    ///
    /// The size is the caller's to state because it is the caller that built
    /// the scene, and the two are not always the same: a shell surface and a
    /// window frame are both built at a placeholder size and rebound to their
    /// real one on the first frame — passing `(0, 0)` is how they say so.
    pub(crate) fn new(size: (i32, i32)) -> Self {
        Self {
            texture: None,
            id: Id::new(),
            damage: DamageBag::default(),
            size,
            stale: false,
        }
    }

    /// Render the scene at `size` device pixels and hand back an element.
    ///
    /// Three things happen in this order and none of them are optional. Qt
    /// renders; the compositor's EGL context goes back on this thread; Qt's
    /// fence is waited for. The second is why the Qt half is one call rather
    /// than inline — see [`restore`] — and the third is why the fence is
    /// imported rather than dropped.
    ///
    /// Subject to [`super::no_frame_in_flight`], through every call it makes.
    pub(crate) fn element(
        &mut self,
        scene: &mut Scene,
        renderer: &mut GlesRenderer,
        size: (i32, i32),
        scale: f64,
        placement: Placement,
    ) -> Option<TextureRenderElement<GlesTexture>> {
        let rendered = self.render(scene, size, scale);
        // Unconditional, and underneath every way out of the call above,
        // including the paths that failed. Rendering leaves Qt's context on the
        // thread; building a scene, freeing one and rebinding one leave none.
        // Neither is a state the next line can run in — `EGLFence::import` is
        // an `eglCreateSync` of our own, not smithay's, so nothing will make
        // our context current for it.
        if let Err(err) = restore(renderer) {
            tracing::error!(?err, "the compositor's EGL context could not be restored");
            return None;
        }

        match rendered {
            // `None` is "Qt had nothing new to draw", so the texture already in
            // hand is this frame's picture and no import and no wait are owed.
            Ok(None) => {}
            Ok(Some(fence)) => {
                // A fence inside the `Some` is the driver's; without one the
                // host has already waited on the CPU with `glFinish` and the
                // frame is complete, which is a correct answer and not a
                // missing fence.
                if let Some(fence) = fence
                    && let Err(err) = wait_for(renderer, fence)
                {
                    // Not drawing this frame is the cheaper wrong answer:
                    // sampling anyway is the race described above.
                    tracing::warn!(?err, "could not wait for Qt's fence; skipping a frame");
                    return None;
                }
                // Re-imported every frame rather than once: `import_dmabuf` is
                // cached on the buffer and re-binds the EGLImage to the same
                // texture name, which is what makes what Qt just wrote visible
                // to our context.
                let Some(buffer) = scene.buffer() else {
                    tracing::warn!("a GPU scene has no buffer to sample");
                    return None;
                };
                match renderer.import_dmabuf(buffer, None) {
                    Ok(texture) => {
                        // Whole-buffer damage, because Qt does not say what it
                        // repainted and the buffer is the same one every frame.
                        self.damage.add([Rectangle::from_size(self.size.into())]);
                        self.texture = Some(texture);
                    }
                    Err(err) => {
                        tracing::warn!(?err, "could not import a scene's buffer");
                        return None;
                    }
                }
            }
            // The picture in hand is the last one Qt drew, and it is still the
            // truest thing available: see `render` for why that is drawn rather
            // than dropped.
            Err(err) => {
                if !self.stale {
                    self.stale = true;
                    tracing::warn!(
                        ?err,
                        asked = ?size,
                        holding = ?self.size,
                        "a GPU scene did not render; drawing the last frame it managed. Said \
                         once per scene until it renders again"
                    );
                }
            }
        }

        // The whole buffer in its **own** pixels, which after a failed rebind
        // is not `size`: `self.size` is what the texture in hand actually is,
        // and stating anything else here samples outside it. The element maps
        // that onto `placement.size` logical pixels, so a frozen scene is
        // stretched into the geometry it should have had rather than cropped
        // to a corner of it.
        let texture = self.texture.as_ref()?;
        let source = Rectangle::from_size((f64::from(self.size.0), f64::from(self.size.1)).into());
        Some(TextureRenderElement::from_texture_with_damage(
            self.id.clone(),
            renderer.context_id(),
            placement.position,
            texture.clone(),
            1,
            Transform::Normal,
            Some(placement.alpha),
            Some(source),
            Some(placement.size),
            None,
            self.damage.snapshot(),
            placement.kind,
        ))
    }

    /// Everything that hands this thread's GL context to Qt, in one place.
    ///
    /// Gathered into one call so [`Gpu::element`] can put the context back
    /// underneath it on every path out, the failures included. **Nothing in
    /// here may touch the renderer.**
    fn render(
        &mut self,
        scene: &mut Scene,
        size: (i32, i32),
        scale: f64,
    ) -> Result<Option<Option<OwnedFd>>> {
        if self.size != size {
            // A dmabuf cannot be resized, so changing size means a new buffer —
            // and *only* a new buffer. Rebuilding the scene around one builds a
            // new QML object tree, which restarts every animation, transition
            // and stored property in it; a pane's scene is sized from an
            // *animating* rectangle, so that happened once per frame for the
            // length of every window animation and no animation inside a
            // resizing scene ever advanced. That was a correctness bug wearing
            // a performance bug's clothes.
            //
            // **A failed rebind freezes the scene; it does not hide it.** The
            // host's contract is that the scene stays whole and stays on the
            // buffer it already has — the import runs before the release — so
            // the texture in hand is still a real picture, just the wrong size.
            // `self.size` is therefore left naming the buffer that texture
            // really is, and `element` draws it stretched into the new
            // geometry.
            //
            // The alternative was in place until this task and it was chosen by
            // omission rather than on purpose: propagate, draw nothing, and the
            // surface *disappears* — not for a frame, but for good, since
            // nothing about the retry changes and it fails identically every
            // frame after. A missing window frame leaves a client sitting below
            // a 32-pixel strip of desktop, because the insets it reserved are
            // read from the scene once and do not go away with the picture; a
            // missing pointer is indistinguishable from input being dead, which
            // is the exact report that made `cursor.rs` exist. Stretched chrome
            // is visibly wrong and gets reported; absent chrome reads as a
            // crash and gets rebooted.
            //
            // `self.size` is not advanced on failure, so the next frame asks
            // for the same thing again: an allocation that failed because the
            // GPU was momentarily full heals itself without anything having to
            // notice.
            scene.rebind_sized(size.0, size.1, scale)?;
            self.size = size;
            // The compositor's side of the dmabuf is a separate import of a
            // genuinely different buffer, so the texture in hand names the old
            // one and nothing in the new one is the old one's.
            self.texture = None;
            self.damage.reset();
        } else {
            // Only the ratio can have moved; the pixel size is the buffer's and
            // `solium_qml_scene_resize` refuses to change it.
            scene.resize(size.0, size.1, scale);
        }
        let rendered = scene.render_gpu();
        if rendered.is_ok() {
            self.stale = false;
        }
        rendered
    }
}

/// Put the compositor's EGL context back after Qt has had the thread.
///
/// `render_gpu` leaves Qt's context on the thread and building, freeing or
/// rebinding a scene leaves none, so after any of them the thread is not in a
/// state this file can make an EGL call in.
///
/// Only *this* file, and that distinction is the whole reason this exists as a
/// deliberate call rather than something the renderer handles. Every entry
/// point on `GlesRenderer` itself re-binds its context, so between frames an
/// empty thread costs it one `eglMakeCurrent` and nothing else — a live
/// `GlesFrame` is the exception, and `qml::no_frame_in_flight` is where that is
/// spelled out and enforced. What the renderer cannot cover either way is a
/// call that is not smithay's, and `EGLFence::import` is exactly that: an
/// `eglCreateSync` against our display, needing a current context nobody else
/// is going to make for it.
///
/// `EGLContext::make_current` is the API. There is no `bind_context`.
///
/// This has a matching half on the other side, and neither works alone. An
/// `eglMakeCurrent` is invisible to Qt — it keeps its own thread-local record of
/// which context is current — so once this has run, Qt believes it still has the
/// thread and skips the `makeCurrent` its next call needs. On a render that
/// draws the frame into our context and leaves the buffer empty; on a *teardown*
/// it deletes Qt's GL object names out of our context, which are our objects.
/// See `clear_stale_current_context` in `qml/host.cpp`, which is what makes the
/// second frame draw and the first free safe.
#[expect(unsafe_code, reason = "restoring our EGL context after Qt")]
pub(crate) fn restore(renderer: &GlesRenderer) -> Result<()> {
    // SAFETY: called on the thread that owns this context, with no other
    // context of ours in use on it. What makes it unsafe is that the context
    // could have been destroyed; Qt has its own and does not touch this one.
    unsafe { renderer.egl_context().make_current() }
        .map_err(|err| anyhow!("making the compositor's EGL context current again: {err}"))
}

/// Wait for Qt's frame to land before sampling the buffer it landed in.
///
/// `Renderer::wait` takes a `SyncPoint` and not a raw fd, so the fence is
/// imported first — `EGLFence::import` is the only constructor that takes a
/// native fence fd. Smithay then inserts it into our context if it can and
/// blocks the thread on it if it cannot, so a driver with no server-side wait
/// costs a stall rather than correctness.
fn wait_for(renderer: &mut GlesRenderer, fence: OwnedFd) -> Result<()> {
    let imported = {
        let display = renderer.egl_context().display();
        EGLFence::import(display, fence).map_err(|err| anyhow!("importing Qt's fence: {err}"))?
    };
    renderer
        .wait(&SyncPoint::from(imported))
        .map_err(|err| anyhow!("waiting on Qt's fence: {err}"))
}
