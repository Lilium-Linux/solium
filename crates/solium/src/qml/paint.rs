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

/// A picture Qt has drawn, ready to be put on screen.
///
/// The size travels with the texture because after a rebind that failed the two
/// no longer agree with what the caller asked for, and the *texture's* size is
/// the one every use of it has to state. See [`Gpu::sample`].
#[derive(Debug)]
pub(crate) struct Sampled {
    /// The scene's dmabuf, imported into the compositor's context.
    pub(crate) texture: GlesTexture,
    /// The pixel size of the buffer that texture names — **not** the size the
    /// caller asked for, when a rebind has failed and the scene is frozen.
    pub(crate) size: (i32, i32),
}

/// A complaint that is made once and then held until the thing works again.
///
/// Every failure this module can reach fires from inside [`Gpu::sample`], which
/// `render.rs` reaches **once per scene per output per frame** — and none of
/// them heals by itself. A driver that will not allocate, a context that will
/// not come back and a buffer that will not import all fail identically on the
/// next frame and the one after, so an unlatched line here is not a log line,
/// it is the log, at four figures a second.
///
/// Which is worse than noise. journald's defaults are `RateLimitBurst=10000`
/// per `RateLimitIntervalSec=30s`, so a flood of ours starts dropping messages
/// within seconds — and what it drops is whatever else was being said at the
/// time, which on this path is the Qt diagnostics the failure is actually
/// diagnosed from.
///
/// The reset is the other half and points the other way: a latch that never
/// clears turns a *transient* failure into permanent silence, which is the
/// failure mode of a throttle rather than of a flood.
///
/// A type rather than a bare `bool` because there are several of them and
/// `cursor.rs`'s `read_back` is a free function, so the flag has to travel.
#[derive(Debug, Default)]
pub(crate) struct Said(bool);

impl Said {
    /// Say it, unless it has already been said since the last success.
    pub(crate) fn once(&mut self, say: impl FnOnce()) {
        if !self.0 {
            self.0 = true;
            say();
        }
    }

    /// It worked, so the next failure is news again.
    pub(crate) fn worked(&mut self) {
        self.0 = false;
    }
}

/// What a scene has already complained about, one latch per failure.
///
/// One per site rather than one for the whole of [`Gpu::sample`], because these
/// fail for unrelated reasons and clear on unrelated successes: under a shared
/// flag the second thing to go wrong is the one nobody ever hears about. The
/// same split `cursor.rs` makes between its `drawing` and its `uploading`, and
/// for the same reason.
#[derive(Debug, Default)]
struct Complaints {
    /// The compositor's EGL context would not come back.
    ///
    /// The `error!` of the five and the one that most needs holding: every
    /// other line in the journal from that point on is downstream of it, so
    /// this is the flood that buries its own cause.
    restoring: Said,
    /// Qt's fence would not import, or would not be waited on.
    ///
    /// Cleared only by a wait that succeeded, and deliberately not by a frame
    /// that needed none: a driver handing a fence out every other frame would
    /// otherwise re-open the flood at half the rate.
    waiting: Said,
    /// The scene had no buffer under it to sample.
    missing: Said,
    /// `import_dmabuf` refused the buffer it does have.
    importing: Said,
    /// The scene did not render and is being drawn frozen.
    ///
    /// A rebind that fails once fails every frame after — the size it is
    /// retried at does not change — which is the case that put the first latch
    /// here.
    rendering: Said,
}

/// One scene's buffer, as the compositor sees it.
///
/// Owns the imported texture and the damage that goes with it, and nothing
/// else: the buffer itself belongs to the [`Scene`], which is the arrangement
/// that makes the whole path safe — the thing being read cannot outlive the
/// thing drawing into it.
#[derive(Debug)]
pub(crate) struct Gpu {
    /// The imported texture and the size of the buffer it names, kept so an
    /// idle frame costs no import.
    ///
    /// One field and not two, because the pair is the invariant: a texture
    /// whose size is stated from somewhere else is how a stale texture comes to
    /// be sampled outside itself. It is replaced only when a *new* import has
    /// succeeded, which is what stops one failed import from hiding a scene —
    /// see [`Gpu::take`].
    shown: Option<Sampled>,
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
    /// The pixel size of the buffer the **scene** is on.
    ///
    /// Moves only when a rebind succeeds, so after one that failed it still
    /// names the buffer Qt is drawing into — which is what makes the freeze
    /// safe. It is compared against `shown`'s size rather than assumed equal to
    /// it: between a successful rebind and a successful import they differ.
    bound: (i32, i32),
    /// What has already been said about this scene, and is therefore not worth
    /// saying again until it works. See [`Complaints`].
    said: Complaints,
}

impl Gpu {
    /// A scene whose buffer is `size` pixels.
    ///
    /// The size is the caller's to state because it is the caller that built
    /// the scene, and the two are not always the same: a shell surface and a
    /// window frame are both built at 1x1 and rebound to their real size on the
    /// first frame — passing `(0, 0)` is how they say so.
    pub(crate) fn new(size: (i32, i32)) -> Self {
        Self {
            shown: None,
            id: Id::new(),
            damage: DamageBag::default(),
            bound: size,
            said: Complaints::default(),
        }
    }

    /// Render the scene at `size` device pixels and hand back what Qt drew.
    ///
    /// Three things happen in this order and none of them are optional. Qt
    /// renders; the compositor's EGL context goes back on this thread; Qt's
    /// fence is waited for. The second is why the Qt half is one call rather
    /// than inline — see [`restore`] — and the third is why the fence is
    /// imported rather than dropped: sampling a buffer that is still being
    /// written is a race that surfaces as garbage on maybe one frame in several
    /// hundred, which is the hardest possible thing to attribute.
    ///
    /// The `Sampled` carries its own size, and it can be smaller than `size`:
    /// that is a frozen scene, and [`Gpu::render`] is where the freeze is
    /// argued.
    ///
    /// Subject to [`super::no_frame_in_flight`], through every call it makes.
    pub(crate) fn sample(
        &mut self,
        scene: &mut Scene,
        renderer: &mut GlesRenderer,
        size: (i32, i32),
        scale: f64,
    ) -> Option<&Sampled> {
        let rendered = self.render(scene, size, scale);
        // Unconditional, and underneath every way out of the call above,
        // including the paths that failed. Rendering leaves Qt's context on the
        // thread; building a scene, freeing one and rebinding one leave none.
        // Neither is a state the next line can run in — `EGLFence::import` is
        // an `eglCreateSync` of our own, not smithay's, so nothing will make
        // our context current for it.
        match restore(renderer) {
            Ok(()) => self.said.restoring.worked(),
            Err(err) => {
                // Latched, and this is the one of the five that most needs it.
                // A context that will not come back does not come back next
                // frame either, so this is an `error!` per scene per output per
                // frame for the rest of the session — and journald drops the
                // *rest* of the journal to keep up with it, starting with the
                // Qt diagnostics that say what actually went wrong. See
                // [`Complaints`].
                self.said.restoring.once(|| {
                    tracing::error!(
                        ?err,
                        "the compositor's EGL context could not be restored. Said once per scene \
                         until it is"
                    );
                });
                return None;
            }
        }

        match rendered {
            Ok(Some(fence)) => {
                // A fence inside the `Some` is the driver's; without one the
                // host has already waited on the CPU with `glFinish` and the
                // frame is complete, which is a correct answer and not a
                // missing fence.
                if let Some(fence) = fence {
                    match wait_for(renderer, fence) {
                        Ok(()) => self.said.waiting.worked(),
                        Err(err) => {
                            // Not drawing this frame is the cheaper wrong
                            // answer: sampling anyway is the race described
                            // above.
                            self.said.waiting.once(|| {
                                tracing::warn!(
                                    ?err,
                                    "could not wait for Qt's fence; skipping a frame. Said once \
                                     per scene until one is waited for"
                                );
                            });
                            return None;
                        }
                    }
                }
                self.take(scene, renderer);
            }
            // Qt had nothing new to draw, so the texture in hand is normally
            // this frame's picture already and no import and no wait are owed.
            //
            // Normally, and not always. An import that failed left `shown`
            // naming a *different* buffer from the one the scene is now on, and
            // `render_gpu` has already spent the dirty flag on the frame it
            // rendered into it. Nothing will set that flag again — a pointer
            // has no animation and nothing writes its properties — so without
            // this the scene is never imported again and never drawn again: one
            // transient `import_dmabuf` failure and the picture is gone for the
            // session, which is the exact outcome the freeze above exists to
            // rule out.
            //
            // Safe precisely because Qt said "up to date": the buffer holds a
            // finished frame, so importing it now needs no fence and asks
            // nothing of Qt.
            //
            // That last sentence is load-bearing and its proof is in the C++,
            // so it is written down here rather than left to be re-derived. The
            // obvious way this could be wrong is a buffer Qt has never drawn
            // into: a rebind onto a fresh dmabuf, followed by an `UNCHANGED`
            // render, would send an *uninitialised* allocation through `take`
            // and put GBM's leftovers on screen. It cannot happen, because
            // `solium_qml_scene_rebind` sets `scene->dirty = true`
            // (`host.cpp:1315`) before its `release_the_thread`, and
            // `render_gpu` returns `SOLIUM_QML_UNCHANGED` only under
            // `if (!scene->dirty)` (`host.cpp:1476`). So the first render after
            // any rebind always draws, and this arm is only ever reached on a
            // buffer with a finished frame already in it. Anything that moves
            // that assignment — or makes a rebind leave the flag alone — breaks
            // this arm silently, in the buffer's contents rather than in a
            // return value.
            Ok(None) => {
                if self
                    .shown
                    .as_ref()
                    .is_none_or(|held| held.size != self.bound)
                {
                    self.take(scene, renderer);
                }
            }
            // The picture in hand is the last one Qt drew, and it is still the
            // truest thing available: see `render` for why that is drawn rather
            // than dropped. No recovery import here, unlike the arm above — a
            // render that *failed* says nothing about what is in the buffer,
            // and a rebind that succeeded before it means the buffer may be one
            // Qt has never drawn into at all.
            Err(err) => {
                let (asked, holding) = (size, self.bound);
                self.said.rendering.once(|| {
                    tracing::warn!(
                        ?err,
                        ?asked,
                        ?holding,
                        "a GPU scene did not render; drawing the last frame it managed. Said \
                         once per scene until it renders again"
                    );
                });
            }
        }

        self.shown.as_ref()
    }

    /// Import the scene's buffer, replacing what is shown only if that worked.
    ///
    /// Re-imported rather than kept: `import_dmabuf` is cached on the buffer and
    /// re-binds the EGLImage to the same texture name, which is what makes what
    /// Qt just wrote visible to our context.
    fn take(&mut self, scene: &Scene, renderer: &mut GlesRenderer) {
        let Some(buffer) = scene.buffer() else {
            self.said.missing.once(|| {
                tracing::warn!(
                    "a GPU scene has no buffer to sample. Said once per scene until one appears"
                );
            });
            return;
        };
        self.said.missing.worked();
        match renderer.import_dmabuf(buffer, None) {
            Ok(texture) => {
                self.said.importing.worked();
                // Whole-buffer damage, because Qt does not say what it
                // repainted and the buffer is the same one every frame.
                self.damage.add([Rectangle::from_size(self.bound.into())]);
                self.shown = Some(Sampled {
                    texture,
                    size: self.bound,
                });
            }
            // Deliberately leaves `shown` alone. The previous texture is a real
            // picture — see `Gpu::render` for why it stays valid after the
            // buffer behind it has gone — so keeping it is a stale frame, and
            // clearing it is a scene that never draws again. The `Ok(None)` arm
            // in `sample` is what gets out of it.
            //
            // Latched: an import refused once is refused every frame after, and
            // this runs once per scene per output per frame.
            Err(err) => self.said.importing.once(|| {
                tracing::warn!(
                    ?err,
                    "could not import a scene's buffer. Said once per scene until one imports"
                );
            }),
        }
    }

    /// Render the scene at `size` device pixels and hand back an element.
    ///
    /// [`Gpu::sample`] with a `TextureRenderElement` around it, which is what
    /// every caller but the pointer wants. The pointer needs the pixels rather
    /// than the texture — see `cursor.rs`.
    pub(crate) fn element(
        &mut self,
        scene: &mut Scene,
        renderer: &mut GlesRenderer,
        size: (i32, i32),
        scale: f64,
        placement: Placement,
    ) -> Option<TextureRenderElement<GlesTexture>> {
        let (texture, held) = {
            let shown = self.sample(scene, renderer, size, scale)?;
            (shown.texture.clone(), shown.size)
        };
        // The whole buffer in its **own** pixels, which after a failed rebind
        // is not `size`: `held` is what the texture in hand actually is, and
        // stating anything else here samples outside it. The element maps that
        // onto `placement.size` logical pixels, so a frozen scene is stretched
        // into the geometry it should have had rather than cropped to a corner
        // of it.
        let source = Rectangle::from_size((f64::from(held.0), f64::from(held.1)).into());
        Some(TextureRenderElement::from_texture_with_damage(
            self.id.clone(),
            renderer.context_id(),
            placement.position,
            texture,
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
    /// Gathered into one call so [`Gpu::sample`] can put the context back
    /// underneath it on every path out, the failures included — `sample` is the
    /// caller that restores, and the only one. [`Gpu::element`] was split off
    /// above it later and goes through `sample` like everything else.
    /// **Nothing in here may touch the renderer.**
    fn render(
        &mut self,
        scene: &mut Scene,
        size: (i32, i32),
        scale: f64,
    ) -> Result<Option<Option<OwnedFd>>> {
        if self.bound != size {
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
            // `self.bound` is therefore left naming the buffer Qt is really
            // drawing into, `shown` keeps the size of the one it was imported
            // from, and `element` draws that stretched into the new geometry.
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
            // `self.bound` is not advanced on failure, so the next frame asks
            // for the same thing again: an allocation that failed because the
            // GPU was momentarily full heals itself without anything having to
            // notice.
            scene.rebind_sized(size.0, size.1, scale)?;
            self.bound = size;
            // The texture in hand is *not* cleared here, and that is the whole
            // of F4: it is a stale picture rather than a dangling one, and
            // `shown` carries its size, so nothing samples outside it. Clearing
            // it would mean an import that then failed left the scene with
            // nothing to draw and no dirty flag left to earn a retry with.
            //
            // The scene does **not** still own the buffer that texture names.
            // This line is on the success path, and `Scene::rebind`'s
            // `self.target = Some(target)` has just dropped the old `Target`
            // and with it the old `Dmabuf`. What keeps it safe is the other
            // end: a `GlesTexture` is an `Arc<GlesTextureInternal>` owning both
            // the GL texture name and the `EGLImage` it was bound from, and
            // smithay destroys neither until the last clone drops — we hold
            // one. The `EGLImage` in turn holds EGL's own reference on the
            // underlying buffer object, taken inside `eglCreateImageKHR` and
            // independent of the fds the `Dmabuf` closed, so the pixels stay
            // allocated and stay ours to read. smithay's only remaining link to
            // the dropped buffer is `dmabuf_cache: HashMap<WeakDmabuf,
            // GlesTexture>` (`gles/mod.rs:303`), keyed weakly and swept in
            // `cleanup()` — so the drop evicts a cache entry and frees nothing
            // we are still holding.
            //
            // Which makes it *better* than the arrangement the old comment
            // described: nobody is drawing into that buffer any more, so the
            // frozen picture cannot change underneath us either.
            //
            // Nothing in the new buffer is the old buffer's, so no damage
            // recorded against it means anything.
            self.damage.reset();
        } else {
            // Only the ratio can have moved; the pixel size is the buffer's and
            // `solium_qml_scene_resize` refuses to change it.
            scene.resize(size.0, size.1, scale);
        }
        let rendered = scene.render_gpu();
        if rendered.is_ok() {
            self.said.rendering.worked();
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

#[cfg(test)]
mod tests {
    use super::Said;

    /// A failure that does not heal is reached once per scene per output per
    /// frame for the rest of the session -- a frozen GPU scene is exactly that
    /// -- so the thing worth asserting is that a hundred frames of it cost one
    /// line and not a hundred. Arithmetic, and it does not need Qt or a GPU.
    ///
    /// The reset half matters just as much and in the other direction: a
    /// latch that never clears turns a *transient* failure into permanent
    /// silence, which is the failure mode of a throttle rather than of a flood.
    #[test]
    fn a_complaint_is_said_once_and_is_news_again_after_a_success() {
        let mut said = Said::default();
        let mut lines = 0;
        for _ in 0..100 {
            said.once(|| lines += 1);
        }
        assert_eq!(lines, 1, "a scene logged once per frame");

        said.worked();
        for _ in 0..100 {
            said.once(|| lines += 1);
        }
        assert_eq!(lines, 2, "a failure after a success was swallowed");
    }
}
