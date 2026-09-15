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
///
/// `Clone` is a refcount: a `GlesTexture` is an `Arc<GlesTextureInternal>`, so
/// two of these naming one picture are two handles and one buffer. [`Gpu`]
/// holds one in its cache and one as `shown`, and normally they are the same
/// picture — see [`Gpu::shown`].
#[derive(Clone, Debug)]
pub(crate) struct Sampled {
    /// The scene's dmabuf, imported into the compositor's context.
    pub(crate) texture: GlesTexture,
    /// The pixel size of the buffer that texture names — **not** the size the
    /// caller asked for, when a rebind has failed and the scene is frozen.
    pub(crate) size: (i32, i32),
}

/// What makes one drawn picture of a scene different from another.
///
/// The pixel size **and** the ratio it was laid out at, because the host is
/// given both and the picture depends on both: `solium_qml_scene_rebind` sets
/// the window's geometry to `pixels / scale` and the render target's device
/// pixel ratio to `scale`, so a 2300-pixel buffer at scale 2 holds a
/// 1150-logical-wide layout and the same buffer at scale 1 holds a
/// 2300-logical-wide one. Same buffer, different picture.
///
/// Keyed on the pair rather than on the pixels alone because the pixels alone
/// do not identify it. `pixels = round(logical * scale)`, so for *one* window
/// the size does determine the scale — but a window that resizes can land on a
/// pixel size another scale reached a moment ago, and serving that from the
/// cache would draw a titlebar at half or twice its height. It costs eight
/// bytes a key to not have to reason about how often that happens.
///
/// Exact `f64` equality is the right comparison here and not a tolerance: the
/// scale is a monitor's configured number, handed down unchanged frame after
/// frame, so two frames of one output compare identical. A scale that somehow
/// differed by an ulp would *miss* and pay a rebind, which is the safe
/// direction to be wrong in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Drawn {
    /// The size of the buffer, in device pixels.
    pixels: (i32, i32),
    /// Device pixels per logical one, which is what the scene was laid out at.
    scale: f64,
}

/// How many pictures of one scene are kept at once.
///
/// **Two, because two is what a bezel gives you.** The case this exists for is
/// a window whose slot straddles two monitors at different scales, which is
/// drawn on both every frame — one entry per output, and there is no third
/// output for a window to straddle onto without a third monitor.
///
/// Deliberately not the pointer's four, and the difference is three orders of
/// magnitude. A cursor buffer is 24x24x4 = 2.3 KB at 1x, so `cursor.rs` can
/// afford a generous guard against a script animating an output's scale. A
/// decoration on an ordinary 1150x850 window is 3.9 MB and there is one
/// decoration per window: at four entries, ten windows is 156 MB of GBM that
/// mostly never gets looked at again. Two is 78 MB in the same
/// worst case, and the realistic number — windows do not usually straddle — is
/// one entry each.
///
/// What it costs when it is too small is *today's behaviour and nothing worse*:
/// a window straddling three monitors at three distinct scales thrashes a
/// two-entry cache and rebinds every frame, exactly as it did before this
/// existed. Raising this constant is the knob, and the price of raising it is
/// the arithmetic above.
const KEPT: usize = 2;

/// What a scene has already been drawn at, newest last.
///
/// Keyed by what was asked for, because the compositor draws one scene once per
/// output per frame and each output brings its own scale. `render.rs:477` keeps
/// a window whose *slot* straddles a bezel on **both** screens, deliberately
/// and with a comment arguing for it, so a desktop with a 1x and a 2x monitor
/// asks for 1150x850 and then 2300x1700, alternating, on every frame for as
/// long as the window sits there.
///
/// Without this, every one of those calls took [`Gpu::render`]'s rebind branch:
/// a GBM allocation, a dmabuf export, an `eglCreateImageKHR`, a Qt
/// render-target swap, a full Qt render and an `import_dmabuf` — twice a frame,
/// for ever — and `damage.reset()` with each one, so both outputs repainted the
/// window's whole area every frame as well. None of it needed any user action
/// beyond owning two monitors and dragging a window between them.
///
/// Generic over what is kept only so the policy can be tested without Qt: what
/// costs anything here is how often this misses, and that is arithmetic rather
/// than graphics. Generic over the **key** because the two callers do not agree
/// on what a size is — a pointer is square and keyed on one edge, a window
/// frame is not and is keyed on a [`Drawn`]. It was written for the pointer
/// first and lived in `cursor.rs`; it is here because this is the second
/// caller and a second copy of it was the thing not to write.
///
/// The cap is the caller's for the same reason: see [`KEPT`] here and
/// `cursor.rs`'s, which differ by three orders of magnitude of buffer.
#[derive(Debug)]
pub(crate) struct Kept<K, T> {
    held: Vec<(K, T)>,
    /// At least one, whatever the caller says. A cap of zero would mean
    /// `push` evicting from an empty list, and a cache that cannot hold
    /// anything is a slower way of having no cache.
    cap: usize,
}

impl<K: Copy + PartialEq, T> Kept<K, T> {
    /// A cache holding at most `cap` pictures.
    pub(crate) fn keeping(cap: usize) -> Self {
        Self {
            held: Vec::new(),
            cap: cap.max(1),
        }
    }

    /// Whether a current picture of `key` is in hand.
    ///
    /// **The whole of the rebind decision, and the only place either caller
    /// makes it.** `fresh` is whether Qt has something new to draw for this
    /// scene; when it does, everything kept is the *previous* picture and every
    /// size of it goes at once. One call rather than a `clear` the caller has
    /// to remember before a `has`, because forgetting it is not a miss — it is
    /// the monitor nobody is looking at keeping a stale picture for ever, and
    /// there is no frame on which that corrects itself.
    ///
    /// Invalidating on the dirty flag is not a guess about what Qt would have
    /// done. `solium_qml_scene_render_gpu` returns `SOLIUM_QML_UNCHANGED` under
    /// exactly `if (!scene->dirty)` (`host.cpp:1476`), so a `false` here means
    /// the render this skips would have drawn nothing.
    pub(crate) fn current(&mut self, key: K, fresh: bool) -> bool {
        if fresh {
            self.held.clear();
        }
        self.held.iter().any(|(held, _)| *held == key)
    }

    /// The picture kept for `key`, if there is one.
    pub(crate) fn get(&self, key: K) -> Option<&T> {
        self.held
            .iter()
            .find(|(held, _)| *held == key)
            .map(|(_, value)| value)
    }

    /// The same, to lend to something that needs it by `&mut` —
    /// `MemoryRenderBufferRenderElement::from_buffer` does.
    pub(crate) fn get_mut(&mut self, key: K) -> Option<&mut T> {
        self.held
            .iter_mut()
            .find(|(held, _)| *held == key)
            .map(|(_, value)| value)
    }

    /// Keep `value` under `key`, oldest out first.
    ///
    /// **Eviction drops the value here, and that is the half that decides
    /// whether the cap means anything.** What [`Gpu`] keeps is a [`Sampled`],
    /// whose `GlesTexture` is the last thing holding the `EGLImage` that holds
    /// EGL's own reference on the GBM buffer object — see [`Gpu::render`], where
    /// that argument is set out in full. So dropping the entry is what actually
    /// returns the 3.9 MB, and a cache that retired evicted entries anywhere
    /// instead of dropping them would keep every buffer it ever made while
    /// looking like it had a bound. `cursor.rs` keeps `MemoryRenderBuffer`s and
    /// the same is true of them, three orders of magnitude smaller.
    pub(crate) fn push(&mut self, key: K, value: T) {
        self.held.retain(|(held, _)| *held != key);
        // `cap` is at least one, so the list is never empty when this runs.
        while self.held.len() >= self.cap {
            self.held.remove(0);
        }
        self.held.push((key, value));
    }

    /// How many pictures are being kept.
    ///
    /// Nothing in the compositor asks; the cap is only observable from a test.
    #[cfg(test)]
    pub(crate) fn count(&self) -> usize {
        self.held.len()
    }
}

/// A complaint that is made once and then held until the thing works again.
///
/// Every failure this module can reach fires from inside [`Gpu::refresh`],
/// which `render.rs` reaches **once per scene per output per frame** — and none
/// of them heals by itself. A driver that will not allocate, a context that will
/// not come back and a buffer that will not import all fail identically on the
/// next frame and the one after, so an unlatched line here is not a log line,
/// it is the log, at four figures a second.
///
/// The cache in front of it does not change that arithmetic, and is not an
/// excuse to drop a latch. Every one of these failures leaves nothing in
/// [`Gpu::kept`], so a scene that is failing misses on every output on every
/// frame and reaches here exactly as often as it did before.
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

/// One scene's buffers, as the compositor sees them.
///
/// Owns the imported textures and the damage that goes with them, and nothing
/// else: the buffer Qt is *drawing into* belongs to the [`Scene`], which is the
/// arrangement that makes the whole path safe — the thing being written cannot
/// be freed while something is reading it.
///
/// Plural since the cache, and that needs saying rather than being inferred
/// from the field list. A texture here can name a buffer the scene has already
/// rebound away from, which is safe and is *better* than the alternative:
/// nothing is drawing into it any more, so the picture cannot change underneath
/// a reader. [`Gpu::render`] is where that is argued from smithay's ownership
/// rules rather than asserted.
#[derive(Debug)]
pub(crate) struct Gpu {
    /// The last picture that imported, whatever size it was.
    ///
    /// One field and not two, because the pair is the invariant: a texture
    /// whose size is stated from somewhere else is how a stale texture comes to
    /// be sampled outside itself. It is replaced only when a *new* import has
    /// succeeded, which is what stops one failed import from hiding a scene —
    /// see [`Gpu::take`].
    ///
    /// **This is the freeze anchor and that is now its whole job.** A rebind
    /// that fails leaves the scene on the buffer it already had, and this is
    /// what [`Gpu::element`] then draws stretched into the geometry the scene
    /// should have had; on a scene's very first frame it is `None` and nothing
    /// is drawn, which is the narrower half of that guarantee and is stated in
    /// the plan. Everything that is *not* a failure is served from `kept`.
    ///
    /// Normally this is a second handle on a picture `kept` is also holding, so
    /// it costs a refcount and no memory. It outlives eviction deliberately —
    /// being able to draw the last good frame is the whole point of it — so the
    /// true bound on one of these is [`KEPT`] buffers plus at most one, and
    /// reaching the "plus one" takes a third straddled output evicting it.
    shown: Option<Sampled>,
    /// One imported picture per [`Drawn`] this scene has been asked for.
    ///
    /// The fix for the multi-output rebind churn; [`Kept`] is where the shape is
    /// argued and [`KEPT`] is where the two is.
    kept: Kept<Drawn, Sampled>,
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
            kept: Kept::keeping(KEPT),
            id: Id::new(),
            damage: DamageBag::default(),
            bound: size,
            said: Complaints::default(),
        }
    }

    /// The scene's picture at `size` device pixels, drawing it if it is not
    /// already in hand.
    ///
    /// **Called once per scene per output per frame**, which is the fact the
    /// whole of this is shaped by. An output asking for a size another output
    /// already paid for this frame costs a comparison: see [`Kept`] for the
    /// desktop that made that necessary, and [`Gpu::refresh`] for everything a
    /// miss owes.
    ///
    /// A hit calls neither Qt nor the renderer, so — unlike a miss — it leaves
    /// the thread exactly as it found it and has nothing to restore. That is
    /// the same trade `cursor.rs` already makes one layer up, where a hit on
    /// its own cache does not reach this function at all.
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
        let wanted = Drawn {
            pixels: size,
            scale,
        };
        // `needs_render` is read before anything is drawn, which is the only
        // moment it means "Qt has something we have not got": `render_gpu`
        // clears it. `Decoration::frame` has already set this frame's
        // properties by the time it reaches here, so a retitled or refocused
        // window is dirty on the first output that asks and every kept size of
        // it is dropped together.
        //
        // And `needs_render` alone, deliberately, without the
        // `animation_in_flight` that `render::Drawn` now asks alongside it.
        // They are two different questions that happen to share a flag. That
        // one decides whether there will be another *frame*, and has to keep
        // saying yes through the quiet ticks of an animation that is still
        // running. This one decides whether the picture in hand is stale, and a
        // tick that changed no pixel has staled nothing -- invalidating on it
        // would re-import and re-upload an identical texture on every quiet
        // tick of every animation, which is the cost this cache exists to
        // avoid.
        if !self.kept.current(wanted, scene.needs_render()) {
            self.refresh(scene, renderer, size, scale)?;
        }
        // One expression, at the end, and in that order. After a miss that
        // worked the first arm holds this frame's picture; after one that
        // failed it holds nothing and `shown` is the last frame the scene
        // managed, drawn stretched — or `None` on a scene that has never drawn,
        // which is the case that draws nothing at all.
        self.kept.get(wanted).or(self.shown.as_ref())
    }

    /// Everything a miss owes: render, restore, wait on Qt, import.
    ///
    /// Three things happen in this order and none of them are optional. Qt
    /// renders; the compositor's EGL context goes back on this thread; Qt's
    /// fence is waited for. The second is why the Qt half is one call rather
    /// than inline — see [`restore`] — and the third is why the fence is
    /// imported rather than dropped: sampling a buffer that is still being
    /// written is a race that surfaces as garbage on maybe one frame in several
    /// hundred, which is the hardest possible thing to attribute.
    ///
    /// Split out of [`Gpu::sample`] so that what to hand back is decided in one
    /// place, at the end, rather than returned from four points inside a
    /// borrow. `None` here means **draw nothing this frame** and is not the
    /// same answer as a render that failed: a failed render still has the last
    /// picture to show, whereas a context that would not come back and a fence
    /// that would not wait leave nothing this function is willing to sample.
    fn refresh(
        &mut self,
        scene: &mut Scene,
        renderer: &mut GlesRenderer,
        size: (i32, i32),
        scale: f64,
    ) -> Option<()> {
        // Qt's share of the frame on the GPU path, and the driver's underneath
        // it: a rebind if the size moved, a render, a fence wait and an
        // `import_dmabuf`. The whole of what a cache miss owes, which is
        // precisely what is worth being able to see -- see `pacing::Phase::Qml`
        // and `Kept`, whose entire reason to exist is how expensive this is.
        let _qml = crate::pacing::span(crate::pacing::Phase::Qml);
        crate::pacing::scene_rendered();
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
                self.take(scene, renderer, scale);
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
                    self.take(scene, renderer, scale);
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

        Some(())
    }

    /// Import the scene's buffer, replacing what is shown only if that worked.
    ///
    /// Re-imported rather than kept: `import_dmabuf` is cached on the buffer and
    /// re-binds the EGLImage to the same texture name, which is what makes what
    /// Qt just wrote visible to our context.
    ///
    /// `scale` is only for the cache key, and it is paired with `self.bound`
    /// rather than with what the caller asked for so that the key always
    /// describes the picture actually in hand. The two are the same here —
    /// [`Gpu::render`] returns `Ok` only with `bound` equal to the size it was
    /// asked for — and if that ever stopped being true this would *miss* on the
    /// next lookup and pay a rebind, rather than hand back a picture laid out at
    /// something else.
    fn take(&mut self, scene: &Scene, renderer: &mut GlesRenderer, scale: f64) {
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
                let sampled = Sampled {
                    texture,
                    size: self.bound,
                };
                // Both, and the clone is a refcount on one texture. `kept` is
                // what the next output to ask for this size reads; `shown` is
                // the anchor a failed rebind falls back to, and it has to
                // survive this entry being evicted.
                self.kept.push(
                    Drawn {
                        pixels: self.bound,
                        scale,
                    },
                    sampled.clone(),
                );
                self.shown = Some(sampled);
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
    /// Gathered into one call so [`Gpu::refresh`] can put the context back
    /// underneath it on every path out, the failures included — `refresh` is the
    /// caller that restores, and the only one. [`Gpu::element`] was split off
    /// above it later and goes through `sample` like everything else, and
    /// `sample` reaches this only on a cache miss: a hit hands Qt nothing and so
    /// has nothing to take back.
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
            // Counted before the call rather than after it, so a rebind that
            // *fails* is still counted: it costs the allocation either way, and
            // a failure repeats on every frame afterwards -- which is exactly
            // the shape of thing the count is there to make visible.
            crate::pacing::scene_rebound();
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
    use super::{Drawn, KEPT, Kept, Said};

    /// An ordinary 1150x850 window, as the two monitors under it see it.
    ///
    /// The numbers from the plan's Task 8 Step 5, which is the desktop this
    /// cache exists for: one window, one `Gpu`, two outputs at scale 1 and
    /// scale 2, and `Decoration::frame` computing `round(logical * scale)` for
    /// each of them. They are not the same size and they are not meant to be —
    /// the whole point of the GPU path is that a 2x monitor gets twice the
    /// pixels rather than a stretched copy of the 1x one's.
    const ON_THE_1X: Drawn = Drawn {
        pixels: (1150, 850),
        scale: 1.0,
    };
    const ON_THE_2X: Drawn = Drawn {
        pixels: (2300, 1700),
        scale: 2.0,
    };

    /// **A window straddling a bezel costs two renders in total, not two a
    /// frame.**
    ///
    /// The regression this cache was added for. `render.rs:477` decides by the
    /// window's *slot*, so a window dragged between two monitors and left there
    /// is drawn on both — deliberately, with a comment arguing for it — and
    /// `Gpu::sample` therefore runs once per output per frame with that
    /// output's scale. Before the cache, `Gpu::render`'s `self.bound != size`
    /// was the only thing between those calls and a rebind, and with two sizes
    /// alternating it was true on **every single one**: a fresh GBM allocation,
    /// a dmabuf export, an `eglCreateImageKHR`, a Qt render-target swap, a full
    /// Qt render and an `import_dmabuf`, twice a frame, for as long as the
    /// window sat there — plus a `damage.reset()` each time, so both outputs
    /// repainted the window's whole area every frame too.
    ///
    /// Both rules are here rather than only the new one, because the number
    /// this test exists to move is meaningless without the number it moved
    /// *from*, and a commit message is not somewhere a future reader looks.
    /// The first half is what `paint.rs` did before this commit, written out;
    /// the second half calls the production decision itself.
    ///
    /// Arithmetic, and it needs neither Qt nor a GPU — which is the only reason
    /// there is any coverage of this at all. `dev/wirecheck` does not link the
    /// compositor crate, so `paint.rs` is invisible to it and this defect
    /// survived seven task reviews.
    #[test]
    fn a_window_across_a_bezel_is_drawn_twice_and_then_not_again() {
        // The rule this replaced: the scene's buffer is one size, so asking for
        // the other one always means a new buffer.
        let mut bound = (0, 0);
        let mut rebinds = 0;
        for _ in 0..100 {
            for wanted in [ON_THE_1X, ON_THE_2X] {
                if bound != wanted.pixels {
                    rebinds += 1;
                    bound = wanted.pixels;
                }
            }
        }
        assert_eq!(
            rebinds, 200,
            "the churn this cache exists to stop: two rebinds a frame, for ever"
        );

        // And the rule now, which is `Kept::current` and is what `Gpu::sample`
        // asks. `false` throughout because the scene has nothing new to draw:
        // an idle decorated window is the case, and a decoration that *is*
        // animating genuinely owes both outputs a fresh picture.
        let mut kept: Kept<Drawn, u32> = Kept::keeping(KEPT);
        let mut renders = 0;
        for frame in 0..100 {
            for wanted in [ON_THE_1X, ON_THE_2X] {
                if !kept.current(wanted, false) {
                    renders += 1;
                    kept.push(wanted, frame);
                }
            }
        }
        assert_eq!(
            renders, 2,
            "a straddling window was re-rendered after the first frame"
        );
        assert!(kept.get(ON_THE_1X).is_some() && kept.get(ON_THE_2X).is_some());
    }

    /// And when Qt does have something new, **every** size of it goes.
    ///
    /// The other direction, and the one whose failure is invisible rather than
    /// slow: a decoration that keeps the size belonging to the monitor nobody
    /// is looking at shows a stale title, or a stale focus ring, on that
    /// monitor for ever. There is no later frame on which that corrects itself,
    /// because the dirty flag is spent by the first output to render.
    #[test]
    fn new_content_drops_the_other_monitor_s_size_too() {
        let mut kept: Kept<Drawn, u32> = Kept::keeping(KEPT);
        kept.push(ON_THE_1X, 1);
        kept.push(ON_THE_2X, 1);

        // The first output of the frame reports the scene fresh and misses.
        assert!(!kept.current(ON_THE_1X, true));
        kept.push(ON_THE_1X, 2);
        // The second one is told nothing is fresh, because rendering for the
        // first cleared Qt's flag — and it must still miss, or it draws the
        // frame before last.
        assert!(
            !kept.current(ON_THE_2X, false),
            "the 2x monitor kept a picture from before the change"
        );
    }

    /// A picture is identified by its scale as well as its pixels.
    ///
    /// The host is handed both — `rebind` sets the window's geometry to
    /// `pixels / scale` and the target's device pixel ratio to `scale` — so one
    /// buffer size holds two different pictures at two different scales. A
    /// window that resizes can land on a pixel size another scale reached a
    /// moment earlier, and serving that from the cache would draw a titlebar at
    /// half or twice its proper height.
    #[test]
    fn one_pixel_size_at_two_scales_is_two_pictures() {
        let mut kept: Kept<Drawn, u32> = Kept::keeping(KEPT);
        let at_one = Drawn {
            pixels: (2300, 1700),
            scale: 1.0,
        };
        kept.push(ON_THE_2X, 1);
        assert!(
            !kept.current(at_one, false),
            "a scale-2 picture was served to a scale-1 output"
        );
    }

    /// The cap, which is two because a bezel has two sides.
    ///
    /// Read for exactly what it is: a guard, not an eviction policy anything on
    /// a working desktop reaches. What it costs when it is too small — three
    /// monitors, three distinct scales, one window straddling all of them — is
    /// the behaviour this cache replaced, and not anything worse.
    #[test]
    fn the_cap_is_two_and_evicts_the_oldest() {
        let mut kept: Kept<Drawn, u32> = Kept::keeping(KEPT);
        let sizes: Vec<Drawn> = (1..=(KEPT + 2))
            .map(|step| Drawn {
                #[expect(clippy::cast_possible_truncation, reason = "three small integers")]
                pixels: (100 * step as i32, 100),
                scale: 1.0,
            })
            .collect();
        for size in &sizes {
            kept.push(*size, 0);
        }
        assert_eq!(kept.count(), KEPT);
        assert!(!kept.current(sizes[0], false), "the oldest size was kept");
        assert!(
            kept.current(sizes[KEPT + 1], false),
            "the newest size was dropped"
        );
    }

    /// **Eviction drops what it evicts**, rather than moving it somewhere.
    ///
    /// The half that decides whether the cap means anything. What is kept here
    /// is a `Sampled`, and its `GlesTexture` is the last thing holding the
    /// `EGLImage` that holds EGL's own reference on the GBM buffer object —
    /// `Gpu::render` sets that argument out in full. So the 3.9 MB comes back
    /// when, and only when, the entry is dropped; a cache that retired entries
    /// into a list, or handed them to a caller that kept them, would have a cap
    /// and no bound.
    ///
    /// This asserts the container and not the GL, because the GL needs a
    /// device. A drop counter is what is checkable here, and it is the thing
    /// that would actually be got wrong.
    #[test]
    fn eviction_drops_the_buffer_rather_than_parking_it() {
        use std::{cell::Cell, rc::Rc};

        struct Counted(Rc<Cell<u32>>);
        impl Drop for Counted {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        impl std::fmt::Debug for Counted {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("Counted")
            }
        }

        let dropped = Rc::new(Cell::new(0));
        let mut kept: Kept<i32, Counted> = Kept::keeping(2);
        for size in 1..=5 {
            kept.push(size, Counted(Rc::clone(&dropped)));
        }
        assert_eq!(dropped.get(), 3, "three evictions freed nothing");

        // And re-pushing a size it already holds frees the old one rather than
        // keeping both under one key.
        kept.push(5, Counted(Rc::clone(&dropped)));
        assert_eq!(dropped.get(), 4, "the replaced picture was kept alive");

        drop(kept);
        assert_eq!(dropped.get(), 6, "dropping the cache kept its buffers");
    }

    /// A cap of zero would mean evicting from an empty list, and there is no
    /// sensible thing for a cache that cannot hold anything to do.
    #[test]
    fn a_cap_of_zero_still_holds_one() {
        let mut kept: Kept<i32, u32> = Kept::keeping(0);
        kept.push(24, 1);
        assert!(kept.current(24, false));
    }

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
