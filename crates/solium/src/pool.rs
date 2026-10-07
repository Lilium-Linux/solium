#![expect(
    unsafe_code,
    reason = "a framebuffer object per pooled texture, made and deleted in raw GL"
)]

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
//! Smithay, std and the effects crate only, so `dev/wirecheck` includes this
//! file.

use std::{cell::RefCell, rc::Rc};

use solium_effects::stage::Plan;

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

/// What a target holds: the effects crate's one enum, so a plan's formats
/// and the pool's are the same type (\[fx0\] Ruling 10 left `rgba16f` to
/// X1.3). `tests::a_target_of_another_format_is_another_target`.
pub(crate) use solium_effects::stage::Format;

/// What makes a target, so the pool's policy is tested with no GPU, as
/// `offscreen::Scratch`'s was before the pool replaced it.
/// `tests::a_target_is_made_once_and_its_fbo_with_it`.
pub(crate) trait Alloc {
    type Tex: Clone;
    fn make(&mut self, size: Size<i32, Physical>, format: Format) -> Option<Self::Tex>;
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

/// One pooled target: a texture of exactly `size` and `format`, and its
/// framebuffer object. `tests::a_target_of_another_size_is_made_anew`,
/// `tests::a_target_of_another_format_is_another_target`.
#[derive(Clone, Debug)]
pub(crate) struct Target<T = GlesTexture> {
    texture: T,
    fbo: Rc<Fbo>,
    size: Size<i32, Physical>,
    format: Format,
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
    pub(crate) fn format(&self) -> Format {
        self.format
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
    /// Targets made, not taken from the free list, since the pool was.
    /// `tests::the_pool_counts_the_targets_it_made`.
    made: usize,
}

/// What a target costs the budget: four bytes a pixel, eight in `rgba16f`.
/// `tests::rgba16f_counts_eight_bytes_a_pixel_against_the_budget`.
fn bytes(size: Size<i32, Physical>, format: Format) -> usize {
    let pixel = match format {
        Format::Rgba8 => 4,
        Format::Rgba16f => 8,
    };
    usize::try_from(size.w).unwrap_or(0) * usize::try_from(size.h).unwrap_or(0) * pixel
}

impl<T: Clone> Pool<T> {
    pub(crate) fn new(budget: usize) -> Self {
        Self {
            free: Vec::new(),
            doomed: Rc::new(RefCell::new(Vec::new())),
            budget,
            carrier: None,
            made: 0,
        }
    }

    /// How many targets the pool has made, not counting those it handed out
    /// again from its free list: wirecheck's cases 12l and 12p see a second
    /// run, and a result drawn, make none.
    /// `tests::the_pool_counts_the_targets_it_made`.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "wirecheck's cases 12l and 12p count what a run makes"
        )
    )]
    pub(crate) fn made(&self) -> usize {
        self.made
    }

    pub(crate) fn set_budget(&mut self, budget: usize) {
        self.budget = budget;
    }

    /// A target of exactly `size` and `format`: from the free list, or made
    /// with its framebuffer object. Exact sizes, because a capture's UVs and
    /// the warp's `src` take the whole texture as the picture
    /// (`warp.rs:227-229`). `tests::a_target_of_another_size_is_made_anew`,
    /// `tests::a_target_of_another_format_is_another_target`.
    pub(crate) fn target<A: Alloc<Tex = T>>(
        &mut self,
        alloc: &mut A,
        size: Size<i32, Physical>,
        format: Format,
    ) -> Option<Target<T>> {
        if let Some(at) = self
            .free
            .iter()
            .position(|held| held.size == size && held.format == format)
        {
            return Some(self.free.swap_remove(at));
        }
        let texture = alloc.make(size, format)?;
        let name = alloc.fbo(&texture)?;
        self.made += 1;
        Some(Target {
            texture,
            fbo: Rc::new(Fbo {
                name,
                doomed: Rc::clone(&self.doomed),
            }),
            size,
            format,
        })
    }

    /// Hand a target back for reuse; over budget it is dropped instead.
    /// `tests::a_target_given_back_over_budget_is_dropped`,
    /// `tests::rgba16f_counts_eight_bytes_a_pixel_against_the_budget`.
    pub(crate) fn give_back(&mut self, target: Target<T>) {
        let held: usize = self
            .free
            .iter()
            .map(|each| bytes(each.size, each.format))
            .sum();
        if held + bytes(target.size, target.format) <= self.budget {
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

    fn make(&mut self, size: Size<i32, Physical>, format: Format) -> Option<GlesTexture> {
        let fourcc = match format {
            Format::Rgba8 => Fourcc::Abgr8888,
            Format::Rgba16f => Fourcc::Abgr16161616f,
        };
        self.0.create_buffer(fourcc, (size.w, size.h).into()).ok()
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

/// What this GPU can render into, found once with a 1×1 target of each
/// format beyond `rgba8`: wirecheck case 12c.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Formats {
    pub(crate) rgba16f: bool,
}

impl Formats {
    /// Whether a plan can run here: every format it draws into renders.
    /// `effect::host::tests::a_stage_asking_for_rgba16f_where_it_is_missing_takes_the_fallback`.
    pub(crate) fn supports(self, plan: &Plan) -> bool {
        self.rgba16f || !plan.formats().contains(&Format::Rgba16f)
    }
}

/// Probe the formats: a 1×1 target of each, whose framebuffer is complete
/// or not (`Gl::fbo` answers `None` for an incomplete one, and smithay
/// refuses `rgba16f` outright on a context without GLES 3). The target goes
/// back to `pool`'s doomed list; sweep it. Wirecheck case 12c, which draws
/// into one where the probe says yes.
pub(crate) fn formats(renderer: &mut GlesRenderer, pool: &mut Pool) -> Formats {
    let rgba16f = pool
        .target(&mut Gl(renderer), (1, 1).into(), Format::Rgba16f)
        .is_some();
    Formats { rgba16f }
}

/// [`formats`] through a pool of its own, swept at once: what `prepare` and
/// `--check` ask once of the GPU they hold, leaving no framebuffer behind.
/// Wirecheck case 12c probes the same way.
pub(crate) fn probe_formats(renderer: &mut GlesRenderer) -> Formats {
    let mut pool = Pool::new(0);
    let found = formats(renderer, &mut pool);
    pool.sweep(renderer);
    found
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

/// Back to front, for a list that is topmost-first: the same convention
/// every other walk of such a list already follows. Smithay's own damage
/// tracker reverses `render::elements`'s list internally
/// (`damage/mod.rs:379`), and `offscreen::Screens::draw` reverses it by hand
/// for the nested backend's software path. `paint` used to draw the list as
/// given, so a capture's last piece — a pane style's `behind` layer, in
/// `PANE_ORDER` — landed on top of its client instead of under it, during
/// every genie, warp and close fade (#227).
/// `tests::a_topmost_first_list_is_painted_back_to_front`.
fn draw_order<T>(elements: &[T]) -> impl Iterator<Item = &T> {
    elements.iter().rev()
}

/// Clear a frame of `size` to transparent and draw `elements` over all of it,
/// back to front; the first failure is returned once every element has been
/// tried. "First" means first in paint order -- the bottommost element,
/// since `draw_order` now visits `behind` before `above` -- not first in the
/// caller's topmost-first list; the only caller logs it in a
/// `tracing::warn!` (`offscreen::draw`), where it only reads differently
/// from the old order if more than one element fails in the same frame.
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
    for element in draw_order(elements) {
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
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "P17 and X1.3 draw through a chain")
)]
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

    use super::{Alloc, Format, Pool, chain_sizes, draw_order};

    /// **A capture draws its topmost-first list back to front**, exactly as
    /// the monitor's own flat path does. Pinned directly on `draw_order`
    /// because `paint` itself needs a real `GlesFrame`, which needs a GPU
    /// `cargo test` does not have (`render.rs:3505`); this is the part of the
    /// fix a GPU-free test can hold, and the wirecheck pattern in
    /// `dev/wirecheck` is where the pixels themselves are proven (#227).
    ///
    /// Four pieces, named as `PANE_ORDER` names them, so a reader sees which
    /// end is the client's: given topmost-first (`above`, `frame`, `client`,
    /// `behind`), painting in that order would leave `behind` drawn last and
    /// therefore on top of the client — the bug — so the list must come back
    /// reversed, `behind` first and `above` last.
    #[test]
    fn a_topmost_first_list_is_painted_back_to_front() {
        let topmost_first = ["above", "frame", "client", "behind"];
        let painted: Vec<&str> = draw_order(&topmost_first).copied().collect();
        assert_eq!(
            painted,
            ["behind", "client", "frame", "above"],
            "the first element painted must be the last one in the list, so \
             the list's own topmost member is painted last and ends up on top"
        );
    }

    /// Textures and framebuffer objects a test can count, and the format
    /// each texture was made in.
    #[derive(Debug, Default)]
    struct Counted {
        textures: u32,
        fbos: u32,
        made: Vec<Format>,
    }

    impl Alloc for Counted {
        type Tex = u32;
        fn make(&mut self, _size: Size<i32, Physical>, format: Format) -> Option<u32> {
            self.textures += 1;
            self.made.push(format);
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
        let first = pool
            .target(&mut alloc, size(1150, 850), Format::Rgba8)
            .expect("a target");
        pool.give_back(first);
        let again = pool
            .target(&mut alloc, size(1150, 850), Format::Rgba8)
            .expect("a target");
        assert_eq!((alloc.textures, alloc.fbos), (1, 1));
        assert_eq!(again.fbo(), 101);
    }

    /// **The pool counts the targets it made**: one taken from the free list
    /// is not counted, one made is. What wirecheck's cases 12l and 12p read
    /// to see a second run, and a result drawn, make nothing.
    #[test]
    fn the_pool_counts_the_targets_it_made() {
        let (mut pool, mut alloc) = (Pool::<u32>::new(64 << 20), Counted::default());
        assert_eq!(pool.made(), 0);
        let first = pool
            .target(&mut alloc, size(64, 64), Format::Rgba8)
            .expect("a target");
        assert_eq!(pool.made(), 1);
        pool.give_back(first);
        let _again = pool
            .target(&mut alloc, size(64, 64), Format::Rgba8)
            .expect("a target");
        assert_eq!(pool.made(), 1, "from the free list");
        let _other = pool
            .target(&mut alloc, size(64, 65), Format::Rgba8)
            .expect("a target");
        assert_eq!(pool.made(), 2);
        assert_eq!(pool.made(), usize::try_from(alloc.textures).expect("small"));
    }

    /// Another size is another target.
    #[test]
    fn a_target_of_another_size_is_made_anew() {
        let (mut pool, mut alloc) = (Pool::<u32>::new(64 << 20), Counted::default());
        let first = pool
            .target(&mut alloc, size(1150, 850), Format::Rgba8)
            .expect("a target");
        pool.give_back(first);
        let _other = pool
            .target(&mut alloc, size(1150, 851), Format::Rgba8)
            .expect("a target");
        assert_eq!(alloc.textures, 2);
    }

    /// A target dropped rather than given back has its framebuffer object
    /// deleted at the next sweep, and one still held anywhere does not.
    #[test]
    fn a_dropped_target_has_its_fbo_deleted_at_the_next_sweep() {
        let (mut pool, mut alloc) = (Pool::<u32>::new(64 << 20), Counted::default());
        let target = pool
            .target(&mut alloc, size(64, 64), Format::Rgba8)
            .expect("a target");
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
        let first = pool
            .target(&mut alloc, size(100, 100), Format::Rgba8)
            .expect("a target");
        let second = pool
            .target(&mut alloc, size(100, 100), Format::Rgba8)
            .expect("a target");
        pool.give_back(first);
        pool.give_back(second);
        assert_eq!(pool.doomed(), vec![102], "the second went over budget");
    }

    /// **A target of another format is another target**: an `rgba8` given
    /// back is not handed out for `rgba16f` at the same size, and the
    /// format reaches what makes the texture.
    #[test]
    fn a_target_of_another_format_is_another_target() {
        let (mut pool, mut alloc) = (Pool::<u32>::new(64 << 20), Counted::default());
        let eight = pool
            .target(&mut alloc, size(64, 64), Format::Rgba8)
            .expect("a target");
        pool.give_back(eight);
        let half = pool
            .target(&mut alloc, size(64, 64), Format::Rgba16f)
            .expect("a target");
        assert_eq!(alloc.textures, 2);
        assert_eq!(alloc.made, [Format::Rgba8, Format::Rgba16f]);
        assert_eq!(half.format(), Format::Rgba16f);
    }

    /// **`rgba16f` counts eight bytes a pixel against the budget**: the
    /// over-budget drop happens at half the pixels.
    #[test]
    fn rgba16f_counts_eight_bytes_a_pixel_against_the_budget() {
        let (mut pool, mut alloc) = (Pool::<u32>::new(100 * 100 * 8), Counted::default());
        let first = pool
            .target(&mut alloc, size(100, 100), Format::Rgba16f)
            .expect("a target");
        let second = pool
            .target(&mut alloc, size(100, 100), Format::Rgba16f)
            .expect("a target");
        pool.give_back(first);
        pool.give_back(second);
        assert_eq!(pool.doomed(), vec![102]);
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
