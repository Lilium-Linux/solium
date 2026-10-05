#![expect(
    unsafe_code,
    reason = "a framebuffer object per pooled texture, made and deleted in raw GL"
)]
#![expect(dead_code, reason = "nothing draws through the pool until Task 16")]

//! Render targets made once and reused, each with a framebuffer object made
//! once: what captures draw into now, and what P17's `View` and X1.3's effect
//! passes will take theirs from (spec §6.3 item 4).
//!
//! Smithay 0.7 makes a framebuffer object on every `bind` of a texture and
//! deletes it when the target drops (`gles/mod.rs:651-676`), so a capture a
//! frame was a framebuffer object made, checked and deleted a frame. Here a
//! target's is made with it (`tests::a_target_is_made_once_and_its_fbo_with_it`),
//! and drawing into it is a frame opened on a 1x1 carrier, bound once a pass,
//! with the target's framebuffer object bound inside: [`frame_for`]. Nothing
//! between `render` and `finish` rebinds the draw framebuffer in smithay 0.7
//! (`render_texture_from_to`, `draw_solid` and `clear` do not); wirecheck's
//! case 11f pins it, and \[16\] decision 1's revisit at Smithay 0.8 must check
//! it again.
//!
//! Smithay and std only, so `dev/wirecheck` includes this file.

use std::{cell::RefCell, rc::Rc};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Color32F, Frame as _, Offscreen as _, Renderer as _,
            element::RenderElement,
            gles::{GlesError, GlesFrame, GlesRenderer, GlesTarget, GlesTexture, ffi},
        },
    },
    utils::{Physical, Rectangle, Scale, Size, Transform},
};

/// What makes a target, so the pool's policy is tested with no GPU, as
/// `offscreen::Scratch`'s was. `tests::a_target_is_made_once_and_its_fbo_with_it`.
pub(crate) trait Alloc {
    type Tex: Clone;
    fn make(&mut self, size: Size<i32, Physical>) -> Option<Self::Tex>;
    fn fbo(&mut self, texture: &Self::Tex) -> Option<u32>;
}

/// A framebuffer object's name, put on the doomed list when the last target
/// holding it goes. `tests::a_dropped_target_has_its_fbo_deleted_at_the_next_sweep`.
#[derive(Debug)]
struct Fbo {
    name: u32,
    doomed: Rc<RefCell<Vec<u32>>>,
}

impl Drop for Fbo {
    fn drop(&mut self) {
        if let Ok(mut doomed) = self.doomed.try_borrow_mut() {
            doomed.push(self.name);
        }
    }
}

/// One pooled target: a texture of exactly `size`, and its framebuffer object.
/// `tests::a_target_of_another_size_is_made_anew`.
#[derive(Clone, Debug)]
pub(crate) struct Target<T = GlesTexture> {
    texture: T,
    fbo: Rc<Fbo>,
    size: Size<i32, Physical>,
}

impl<T> Target<T> {
    pub(crate) fn texture(&self) -> &T {
        &self.texture
    }
    pub(crate) fn size(&self) -> Size<i32, Physical> {
        self.size
    }
    pub(crate) fn fbo(&self) -> u32 {
        self.fbo.name
    }
}

/// The renderer's targets not in use.
/// `tests::a_target_is_made_once_and_its_fbo_with_it`.
#[derive(Debug)]
pub(crate) struct Pool<T = GlesTexture> {
    free: Vec<Target<T>>,
    doomed: Rc<RefCell<Vec<u32>>>,
    /// Bytes the free list may hold: twice the largest monitor's (Ruling 10).
    /// `tests::a_target_given_back_over_budget_is_dropped`.
    budget: usize,
    /// The 1x1 target a capture's frame is opened on: wirecheck's case 11f.
    carrier: Option<T>,
}

fn bytes(size: Size<i32, Physical>) -> usize {
    usize::try_from(size.w).unwrap_or(0) * usize::try_from(size.h).unwrap_or(0) * 4
}

impl<T: Clone> Pool<T> {
    pub(crate) fn new(budget: usize) -> Self {
        Self {
            free: Vec::new(),
            doomed: Rc::new(RefCell::new(Vec::new())),
            budget,
            carrier: None,
        }
    }

    pub(crate) fn set_budget(&mut self, budget: usize) {
        self.budget = budget;
    }

    /// A target of exactly `size`: from the free list, or made with its
    /// framebuffer object. Exact sizes, because a capture's UVs and the warp's
    /// `src` take the whole texture as the picture (`warp.rs:227-229`).
    /// `tests::a_target_of_another_size_is_made_anew`.
    pub(crate) fn target<A: Alloc<Tex = T>>(
        &mut self,
        alloc: &mut A,
        size: Size<i32, Physical>,
    ) -> Option<Target<T>> {
        if let Some(at) = self.free.iter().position(|held| held.size == size) {
            return Some(self.free.swap_remove(at));
        }
        let texture = alloc.make(size)?;
        let name = alloc.fbo(&texture)?;
        Some(Target {
            texture,
            fbo: Rc::new(Fbo {
                name,
                doomed: Rc::clone(&self.doomed),
            }),
            size,
        })
    }

    /// Hand a target back for reuse; over budget it is dropped instead.
    /// `tests::a_target_given_back_over_budget_is_dropped`.
    pub(crate) fn give_back(&mut self, target: Target<T>) {
        let held: usize = self.free.iter().map(|each| bytes(each.size)).sum();
        if held + bytes(target.size) <= self.budget {
            self.free.push(target);
        }
    }

    /// The framebuffer objects of targets dropped since the last call.
    /// `tests::a_dropped_target_has_its_fbo_deleted_at_the_next_sweep`.
    pub(crate) fn doomed(&mut self) -> Vec<u32> {
        self.doomed
            .try_borrow_mut()
            .map(|mut doomed| std::mem::take(&mut *doomed))
            .unwrap_or_default()
    }
}

impl Pool<GlesTexture> {
    /// The carrier: a 1x1 texture made once, bound once a pass. Wirecheck's
    /// case 11f.
    pub(crate) fn carrier(&mut self, renderer: &mut GlesRenderer) -> Option<GlesTexture> {
        if self.carrier.is_none() {
            self.carrier = renderer.create_buffer(Fourcc::Abgr8888, (1, 1).into()).ok();
        }
        self.carrier.clone()
    }

    /// Delete the framebuffer objects of targets dropped since the last
    /// sweep. Between frames only. Wirecheck's case 11f.
    pub(crate) fn sweep(&mut self, renderer: &mut GlesRenderer) {
        let doomed = self.doomed();
        if doomed.is_empty() {
            return;
        }
        // SAFETY: `with_context` makes the context current, and every name
        // was made against it.
        let _ = renderer.with_context(|gl| unsafe {
            gl.DeleteFramebuffers(i32::try_from(doomed.len()).unwrap_or(0), doomed.as_ptr());
        });
    }
}

/// [`Alloc`] on a real renderer. Wirecheck's case 11f.
#[derive(Debug)]
pub(crate) struct Gl<'a>(pub(crate) &'a mut GlesRenderer);

impl Alloc for Gl<'_> {
    type Tex = GlesTexture;

    fn make(&mut self, size: Size<i32, Physical>) -> Option<GlesTexture> {
        self.0
            .create_buffer(Fourcc::Abgr8888, (size.w, size.h).into())
            .ok()
    }

    fn fbo(&mut self, texture: &GlesTexture) -> Option<u32> {
        let id = texture.tex_id();
        // SAFETY: the context is current inside `with_context`; the texture is
        // this context's.
        self.0
            .with_context(|gl| unsafe {
                let mut name = 0;
                gl.GenFramebuffers(1, &raw mut name);
                gl.BindFramebuffer(ffi::FRAMEBUFFER, name);
                gl.FramebufferTexture2D(
                    ffi::FRAMEBUFFER,
                    ffi::COLOR_ATTACHMENT0,
                    ffi::TEXTURE_2D,
                    id,
                    0,
                );
                let complete =
                    gl.CheckFramebufferStatus(ffi::FRAMEBUFFER) == ffi::FRAMEBUFFER_COMPLETE;
                gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);
                if complete {
                    Some(name)
                } else {
                    gl.DeleteFramebuffers(1, &raw const name);
                    None
                }
            })
            .ok()
            .flatten()
    }
}

/// A frame of `target`'s size, opened on the bound carrier, drawing into the
/// target's own framebuffer object: wirecheck's case 11f.
pub(crate) fn frame_for<'frame, 'buffer, T>(
    renderer: &'frame mut GlesRenderer,
    carrier: &'frame mut GlesTarget<'buffer>,
    target: &Target<T>,
) -> Result<GlesFrame<'frame, 'buffer>, GlesError> {
    let mut frame = renderer.render(carrier, target.size, Transform::Normal)?;
    let fbo = target.fbo();
    // SAFETY: inside the frame's own context, which made the framebuffer.
    frame.with_context(|gl| unsafe { gl.BindFramebuffer(ffi::FRAMEBUFFER, fbo) })?;
    Ok(frame)
}

/// Clear a frame of `size` to transparent and draw `elements` over all of it;
/// the first failure is returned once every element has been tried.
/// Wirecheck's case 11f.
pub(crate) fn paint<E: RenderElement<GlesRenderer>>(
    frame: &mut GlesFrame<'_, '_>,
    size: Size<i32, Physical>,
    elements: &[E],
    scale: f64,
) -> Result<(), GlesError> {
    let whole = [Rectangle::from_size(size)];
    // Transparent, not black: a window's own corners are rounded, and anything
    // opaque here would draw a square behind them. Wirecheck's case 11f reads
    // a target painted with nothing back as transparent.
    frame.clear(Color32F::TRANSPARENT, &whole)?;
    let mut first = Ok(());
    for element in elements {
        let (source, destination) = (element.src(), element.geometry(Scale::from(scale)));
        if let Err(err) = element.draw(frame, source, destination, &whole, &[])
            && first.is_ok()
        {
            first = Err(err);
        }
    }
    first
}

/// The levels of a mip chain from `base` down to `fit`: halved, rounded up,
/// until both sides fit. P17 and X1.3 draw through them.
/// `tests::the_chain_halves_until_the_fit`.
pub(crate) fn chain_sizes(
    base: Size<i32, Physical>,
    fit: Size<i32, Physical>,
) -> Vec<Size<i32, Physical>> {
    let mut levels = Vec::new();
    let mut at = base;
    while at.w > fit.w || at.h > fit.h {
        at = (((at.w + 1) / 2).max(1), ((at.h + 1) / 2).max(1)).into();
        levels.push(at);
    }
    levels
}

#[cfg(test)]
mod tests {
    use smithay::utils::{Physical, Size};

    use super::{Alloc, Pool, chain_sizes};

    /// Textures and framebuffer objects a test can count.
    #[derive(Debug, Default)]
    struct Counted {
        textures: u32,
        fbos: u32,
    }

    impl Alloc for Counted {
        type Tex = u32;
        fn make(&mut self, _size: Size<i32, Physical>) -> Option<u32> {
            self.textures += 1;
            Some(self.textures)
        }
        fn fbo(&mut self, _texture: &u32) -> Option<u32> {
            self.fbos += 1;
            Some(100 + self.fbos)
        }
    }

    fn size(w: i32, h: i32) -> Size<i32, Physical> {
        (w, h).into()
    }

    /// **A target is made once, and its framebuffer object with it**: given
    /// back and asked for again at the same size, nothing new is made.
    #[test]
    fn a_target_is_made_once_and_its_fbo_with_it() {
        let (mut pool, mut alloc) = (Pool::<u32>::new(64 << 20), Counted::default());
        let first = pool.target(&mut alloc, size(1150, 850)).expect("a target");
        pool.give_back(first);
        let again = pool.target(&mut alloc, size(1150, 850)).expect("a target");
        assert_eq!((alloc.textures, alloc.fbos), (1, 1));
        assert_eq!(again.fbo(), 101);
    }

    /// Another size is another target.
    #[test]
    fn a_target_of_another_size_is_made_anew() {
        let (mut pool, mut alloc) = (Pool::<u32>::new(64 << 20), Counted::default());
        let first = pool.target(&mut alloc, size(1150, 850)).expect("a target");
        pool.give_back(first);
        let _other = pool.target(&mut alloc, size(1150, 851)).expect("a target");
        assert_eq!(alloc.textures, 2);
    }

    /// A target dropped rather than given back has its framebuffer object
    /// deleted at the next sweep, and one still held anywhere does not.
    #[test]
    fn a_dropped_target_has_its_fbo_deleted_at_the_next_sweep() {
        let (mut pool, mut alloc) = (Pool::<u32>::new(64 << 20), Counted::default());
        let target = pool.target(&mut alloc, size(64, 64)).expect("a target");
        let held = target.clone();
        drop(target);
        assert!(pool.doomed().is_empty(), "a clone still holds it");
        drop(held);
        assert_eq!(pool.doomed(), vec![101]);
        assert!(pool.doomed().is_empty(), "said once");
    }

    /// Over budget, a target given back is dropped, not kept: an overview of
    /// twenty windows closing does not leave twenty textures on the free list.
    #[test]
    fn a_target_given_back_over_budget_is_dropped() {
        let one = 100 * 100 * 4;
        let (mut pool, mut alloc) = (Pool::<u32>::new(one), Counted::default());
        let first = pool.target(&mut alloc, size(100, 100)).expect("a target");
        let second = pool.target(&mut alloc, size(100, 100)).expect("a target");
        pool.give_back(first);
        pool.give_back(second);
        assert_eq!(pool.doomed(), vec![102], "the second went over budget");
    }

    /// The mip chain's levels: halved, rounded up, until both sides fit.
    #[test]
    fn the_chain_halves_until_the_fit() {
        assert_eq!(
            chain_sizes(size(2560, 1440), size(320, 180)),
            vec![size(1280, 720), size(640, 360), size(320, 180)]
        );
        assert_eq!(
            chain_sizes(size(1151, 101), size(300, 30)),
            vec![size(576, 51), size(288, 26)]
        );
        assert!(chain_sizes(size(100, 100), size(200, 200)).is_empty());
    }
}
