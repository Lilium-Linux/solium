//! A pane's captures, each kept in a pooled target between passes.
//!
//! Each records what it was drawn from ([`Inputs`]) and is drawn again only
//! when that differs (Ruling 9); it keeps one id for life and moves its commit
//! only when redrawn, so a capture of a still window is neither drawn nor
//! damaged. `tests::a_redrawn_capture_moves_its_commit_and_keeps_its_id`,
//! `state::tests::real_client::a_capture_whose_surface_tree_has_not_committed_is_not_drawn_again`.
//! A pane's warp carries its pane capture's id and a commit of its own, which
//! moves when that capture is redrawn and when the warp's mesh changes shape:
//! `tests::a_warp_whose_mesh_moves_inside_the_same_bounds_is_given_a_new_commit`.
//! Its popups' warp, drawn in front of it, does the same with the popups'
//! capture, apart from the pane's:
//! `tests::the_popups_warp_commits_apart_from_the_panes`.

use smithay::{
    backend::renderer::{
        element::{Element, Id},
        gles::GlesTexture,
        utils::CommitCounter,
    },
    utils::{Physical, Rectangle, Scale, Size, Transform},
};

use crate::pool::{Alloc, Pool, Target};

/// What a capture is of. Two kinds never share a texture: a pane and its
/// popups want different sizes.
/// `tests::a_capture_of_another_kind_at_the_same_size_is_stale`,
/// `tests::a_warped_pane_keeps_its_popups_capture`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// The whole pane, frame included: a warp's picture.
    Pane,
    /// A warped pane's popups, drawn in front of its warp:
    /// `render::tests::a_warped_panes_popups_are_in_front_of_it`.
    Over,
}

/// One element a capture was drawn from: what the damage tracker itself
/// compares (smithay `damage/mod.rs`), so this asks the tracker's question.
#[derive(Clone, Debug, PartialEq)]
struct Seen {
    id: Id,
    commit: CommitCounter,
    geometry: Rectangle<i32, Physical>,
    /// The source rectangle's four floats, as bits: a crop that moves by a
    /// fraction is a change.
    src: [u64; 4],
    alpha: u32,
    transform: Transform,
}

/// What a capture was drawn from: enough to know that drawing it again would
/// change no pixel. `tests::a_capture_of_another_kind_at_the_same_size_is_stale`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Inputs {
    kind: Kind,
    size: Size<i32, Physical>,
    scale: u64,
    seen: Vec<Seen>,
}

impl Inputs {
    pub(crate) fn of<E: Element>(
        kind: Kind,
        size: Size<i32, Physical>,
        scale: f64,
        elements: &[E],
    ) -> Self {
        let output = Scale::from(scale);
        let seen = elements
            .iter()
            .map(|element| {
                let src = element.src();
                Seen {
                    id: element.id().clone(),
                    commit: element.current_commit(),
                    geometry: element.geometry(output),
                    src: [
                        src.loc.x.to_bits(),
                        src.loc.y.to_bits(),
                        src.size.w.to_bits(),
                        src.size.h.to_bits(),
                    ],
                    alpha: element.alpha().to_bits(),
                    transform: element.transform(),
                }
            })
            .collect();
        Self {
            kind,
            size,
            scale: scale.to_bits(),
            seen,
        }
    }
}

/// One capture a pane keeps: a target, the id and commit the element drawn
/// from it carries, and what it was last drawn from.
#[derive(Debug)]
pub(crate) struct Capture<T = Target> {
    id: Id,
    commit: CommitCounter,
    target: Option<T>,
    drawn_from: Option<Inputs>,
}

impl<T> Default for Capture<T> {
    fn default() -> Self {
        Self {
            id: Id::new(),
            commit: CommitCounter::default(),
            target: None,
            drawn_from: None,
        }
    }
}

impl<T> Capture<T> {
    /// Whether drawing it now could change a pixel. `tests::a_capture_never_drawn_is_stale`.
    pub(crate) fn stale(&self, inputs: &Inputs) -> bool {
        self.target.is_none() || self.drawn_from.as_ref() != Some(inputs)
    }

    /// It was drawn into `target` from `inputs`.
    /// `tests::a_redrawn_capture_moves_its_commit_and_keeps_its_id`.
    pub(crate) fn drawn(&mut self, target: T, inputs: Inputs) {
        self.target = Some(target);
        self.drawn_from = Some(inputs);
        self.commit.increment();
    }

    pub(crate) fn id(&self) -> &Id {
        &self.id
    }

    pub(crate) fn commit(&self) -> CommitCounter {
        self.commit
    }

    pub(crate) fn target(&self) -> Option<&T> {
        self.target.as_ref()
    }
}

impl<T: Clone> Capture<Target<T>> {
    /// The target to draw this capture into at `size`: its own when it is that
    /// size, otherwise its own goes back to the pool and the pool's is taken.
    /// `tests::a_genie_costs_one_texture_and_not_one_a_frame`,
    /// `tests::a_resized_window_replaces_its_texture_rather_than_keeping_both`.
    pub(crate) fn target_for<A: Alloc<Tex = T>>(
        &mut self,
        pool: &mut Pool<T>,
        alloc: &mut A,
        size: Size<i32, Physical>,
    ) -> Option<Target<T>> {
        if let Some(held) = self.target.as_ref()
            && held.size() == size
        {
            return Some(held.clone());
        }
        if let Some(old) = self.target.take() {
            pool.give_back(old);
        }
        pool.target(alloc, size)
    }

    /// Give the target back. `tests::a_pane_that_stops_warping_gives_the_texture_back`.
    pub(crate) fn release(&mut self, pool: &mut Pool<T>) {
        if let Some(target) = self.target.take() {
            pool.give_back(target);
        }
        self.drawn_from = None;
    }
}

/// Every capture one pane keeps, by kind, and the commit each of its warps
/// carries. Owned by the pane, as `offscreen::Scratch` was.
/// `tests::a_warped_pane_keeps_its_popups_capture`,
/// `tests::a_warp_at_rest_keeps_its_commit`.
#[derive(Debug)]
pub(crate) struct Captures<T = GlesTexture> {
    pub(crate) pane: Capture<Target<T>>,
    /// A warped pane's popups, at the rectangle they cover, kept with the
    /// pane's while they are open: `tests::a_warped_pane_keeps_its_popups_capture`,
    /// `tests::a_warped_pane_whose_popups_close_gives_their_capture_back`.
    pub(crate) over: Capture<Target<T>>,
    /// Each warp's shape and commit: the pane's and its popups'. A warp's id
    /// is its capture's. `tests::the_popups_warp_commits_apart_from_the_panes`.
    warps: [(Option<crate::render::Shape>, CommitCounter); 2],
}

impl<T> Default for Captures<T> {
    fn default() -> Self {
        Self {
            pane: Capture::default(),
            over: Capture::default(),
            warps: [(None, CommitCounter::default()); 2],
        }
    }
}

impl<T> Captures<T> {
    /// The commit `kind`'s warp carries this pass, the pane's or its popups':
    /// moved when its capture was redrawn or its shape changed, and only then.
    /// `tests::a_warp_at_rest_keeps_its_commit`,
    /// `tests::a_warp_whose_mesh_moves_inside_the_same_bounds_is_given_a_new_commit`,
    /// `tests::the_popups_warp_commits_apart_from_the_panes`.
    pub(crate) fn warp_commit_for(
        &mut self,
        kind: Kind,
        shape: crate::render::Shape,
        redrawn: bool,
    ) -> CommitCounter {
        let (held, commit) = &mut self.warps[usize::from(kind == Kind::Over)];
        if redrawn || *held != Some(shape) {
            commit.increment();
            *held = Some(shape);
        }
        *commit
    }
}

impl<T: Clone> Captures<T> {
    pub(crate) fn get_mut(&mut self, kind: Kind) -> &mut Capture<Target<T>> {
        match kind {
            Kind::Pane => &mut self.pane,
            Kind::Over => &mut self.over,
        }
    }

    /// Give back every capture but those of `kinds`, the ones it captures this
    /// pass: a pane that captures nothing holds nothing, and a warped pane
    /// keeps its popups' capture only while it has popups open.
    /// `tests::a_pane_that_stops_warping_gives_the_texture_back`,
    /// `tests::a_warped_pane_keeps_its_popups_capture`,
    /// `tests::a_warped_pane_whose_popups_close_gives_their_capture_back`.
    pub(crate) fn keep_only(&mut self, kinds: &[Kind], pool: &mut Pool<T>) {
        for each in [Kind::Pane, Kind::Over] {
            if !kinds.contains(&each) {
                self.get_mut(each).release(pool);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use smithay::backend::renderer::element::solid::SolidColorRenderElement;
    use smithay::utils::{Physical, Size};

    use super::{Capture, Captures, Inputs, Kind};
    use crate::pool::{Alloc, Pool};

    fn nothing(kind: Kind) -> Inputs {
        Inputs::of::<SolidColorRenderElement>(kind, size(1150, 850), 1.0, &[])
    }

    /// The kind is in the key: a pane's capture and its popups' at the same
    /// size do not reuse each other's picture.
    #[test]
    fn a_capture_of_another_kind_at_the_same_size_is_stale() {
        let mut capture = Capture::<u32>::default();
        capture.drawn(1, nothing(Kind::Pane));
        assert!(!capture.stale(&nothing(Kind::Pane)));
        assert!(capture.stale(&nothing(Kind::Over)));
    }

    /// **A redrawn capture moves its commit and keeps its id**, which is what
    /// lets the damage tracker skip one that did not change.
    #[test]
    fn a_redrawn_capture_moves_its_commit_and_keeps_its_id() {
        let mut capture = Capture::<u32>::default();
        let (id, before) = (capture.id().clone(), capture.commit());
        capture.drawn(1, nothing(Kind::Pane));
        assert_eq!(capture.id(), &id);
        assert_ne!(capture.commit(), before);
    }

    /// A capture with nothing in it is stale, whatever it is asked.
    #[test]
    fn a_capture_never_drawn_is_stale() {
        assert!(Capture::<u32>::default().stale(&nothing(Kind::Pane)));
    }

    fn shape(turn: f32) -> crate::render::Shape {
        let frame = crate::present::Frame {
            matrix: crate::mat4::Mat4::rotate_z(turn),
            ..crate::present::Frame::real(smithay::utils::Rectangle::new(
                (100, 100).into(),
                (400, 400).into(),
            ))
        };
        crate::render::Shape::of(&frame, None, 1.0)
    }

    /// **A warp whose mesh moves inside the same bounds is given a new
    /// commit**: a square turned a quarter has the same box and another mesh.
    #[test]
    fn a_warp_whose_mesh_moves_inside_the_same_bounds_is_given_a_new_commit() {
        let mut captures = Captures::<u32>::default();
        let first = captures.warp_commit_for(Kind::Pane, shape(0.0), false);
        let turned =
            captures.warp_commit_for(Kind::Pane, shape(std::f32::consts::FRAC_PI_2), false);
        assert_ne!(first, turned);
    }

    /// A warp at rest keeps its commit, and one whose capture was redrawn does not.
    #[test]
    fn a_warp_at_rest_keeps_its_commit() {
        let mut captures = Captures::<u32>::default();
        let first = captures.warp_commit_for(Kind::Pane, shape(0.3), false);
        assert_eq!(
            captures.warp_commit_for(Kind::Pane, shape(0.3), false),
            first
        );
        assert_ne!(
            captures.warp_commit_for(Kind::Pane, shape(0.3), true),
            first
        );
    }

    /// The popups' warp has a commit of its own: a menu that commits does not
    /// make the window under it redraw, nor the other way round.
    #[test]
    fn the_popups_warp_commits_apart_from_the_panes() {
        let mut captures = Captures::<u32>::default();
        let pane = captures.warp_commit_for(Kind::Pane, shape(0.3), false);
        let _over = captures.warp_commit_for(Kind::Over, shape(0.3), true);
        assert_eq!(
            captures.warp_commit_for(Kind::Pane, shape(0.3), false),
            pane
        );
    }

    /// A GBM buffer freed when its last handle goes, as `offscreen`'s tests
    /// modelled it: a `GlesTexture` is an `Arc`, and the memory comes back
    /// when every handle is gone.
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
    struct Handle(
        #[expect(dead_code, reason = "held only for its drop, which counts the free")] Rc<Buffer>,
    );

    /// An allocator that counts what it makes and whose buffers count their frees.
    #[derive(Debug)]
    struct Counting {
        made: u32,
        freed: Rc<Cell<u32>>,
    }
    impl Alloc for Counting {
        type Tex = Handle;
        fn make(&mut self, _size: Size<i32, Physical>) -> Option<Handle> {
            self.made += 1;
            Some(Handle(Rc::new(Buffer(Rc::clone(&self.freed)))))
        }
        fn fbo(&mut self, _texture: &Handle) -> Option<u32> {
            Some(self.made)
        }
    }

    fn counting() -> Counting {
        Counting {
            made: 0,
            freed: Rc::new(Cell::new(0)),
        }
    }

    fn size(w: i32, h: i32) -> Size<i32, Physical> {
        (w, h).into()
    }

    /// **A genie costs one texture, not one a frame.**
    ///
    /// The defect this exists for. `render::prepare` captures each warped pane
    /// once a frame, and the capture once called `create_buffer` every time,
    /// unconditionally — forty lines above a comment saying that allocating a
    /// texture per frame is hundreds of megabytes a second, which was written
    /// about `offscreen::Screens` and never applied to the window path beside
    /// it. At the 3.9 MB of an ordinary 1150x850 window, a 60-frame genie was
    /// 234 MB of GBM churned for one animation of one window, and every mode
    /// that deforms windows deforms *every* window on screen.
    ///
    /// A warped window holds still long enough for its target to be reused
    /// because the size a capture asks for is the window's own
    /// (`offscreen::pixels`), never anything the warp does: a transform is
    /// applied to the texture afterwards, by `warp::mesh_part`, and
    /// `present.rs`'s first rule is that it never changes the geometry the
    /// texture is sized from. So sixty frames ask for one size, and get one
    /// target.
    ///
    /// Arithmetic, and it needs neither Qt nor a GPU — which is the only
    /// reason there is any coverage of this at all. `cargo test` runs in a
    /// container with no render node, so no test can hold a real
    /// `GlesTexture`.
    #[test]
    fn a_genie_costs_one_texture_and_not_one_a_frame() {
        let (mut captures, mut pool, mut alloc) = (
            Captures::<Handle>::default(),
            Pool::new(64 << 20),
            counting(),
        );
        for _ in 0..60 {
            let target = captures
                .get_mut(Kind::Pane)
                .target_for(&mut pool, &mut alloc, size(1150, 850))
                .expect("a target");
            captures
                .get_mut(Kind::Pane)
                .drawn(target, nothing(Kind::Pane));
        }
        assert_eq!(
            alloc.made, 1,
            "sixty frames of one window made {} textures",
            alloc.made
        );
    }

    /// A window that changes size gives its old target back and holds one.
    #[test]
    fn a_resized_window_replaces_its_texture_rather_than_keeping_both() {
        let (mut captures, mut pool, mut alloc) =
            (Captures::<Handle>::default(), Pool::new(0), counting());
        let first = captures
            .get_mut(Kind::Pane)
            .target_for(&mut pool, &mut alloc, size(1150, 850))
            .expect("a target");
        captures
            .get_mut(Kind::Pane)
            .drawn(first, nothing(Kind::Pane));
        let second = captures
            .get_mut(Kind::Pane)
            .target_for(&mut pool, &mut alloc, size(1200, 850))
            .expect("a target");
        captures
            .get_mut(Kind::Pane)
            .drawn(second, nothing(Kind::Pane));
        assert_eq!(
            alloc.freed.get(),
            1,
            "with no budget the old one is freed, not kept beside the new"
        );
    }

    /// A pane that stops warping gives its texture back.
    #[test]
    fn a_pane_that_stops_warping_gives_the_texture_back() {
        let (mut captures, mut pool, mut alloc) =
            (Captures::<Handle>::default(), Pool::new(0), counting());
        let target = captures
            .get_mut(Kind::Pane)
            .target_for(&mut pool, &mut alloc, size(1150, 850))
            .expect("a target");
        captures
            .get_mut(Kind::Pane)
            .drawn(target, nothing(Kind::Pane));
        captures.keep_only(&[], &mut pool);
        assert_eq!(alloc.freed.get(), 1);
        assert!(captures.get_mut(Kind::Pane).target().is_none());
    }

    /// A warped pane with popups open keeps their capture with its own, and
    /// gives it back with the rest when it stops warping.
    #[test]
    fn a_warped_pane_keeps_its_popups_capture() {
        let (mut captures, mut pool, mut alloc) =
            (Captures::<Handle>::default(), Pool::new(0), counting());
        for kind in [Kind::Pane, Kind::Over] {
            let target = captures
                .get_mut(kind)
                .target_for(&mut pool, &mut alloc, size(400, 300))
                .expect("a target");
            captures.get_mut(kind).drawn(target, nothing(kind));
        }
        captures.keep_only(&[Kind::Pane, Kind::Over], &mut pool);
        assert!(
            captures.get_mut(Kind::Over).target().is_some(),
            "a warped pane let its popups' capture go"
        );
        captures.keep_only(&[], &mut pool);
        assert!(captures.get_mut(Kind::Over).target().is_none());
        assert_eq!(alloc.freed.get(), 2);
    }

    /// **A warped pane whose popups close gives their capture back**: it holds
    /// only what it captured this pass, so a menu closed during a flight does
    /// not keep its texture for as long as the window goes on warping.
    #[test]
    fn a_warped_pane_whose_popups_close_gives_their_capture_back() {
        let (mut captures, mut pool, mut alloc) =
            (Captures::<Handle>::default(), Pool::new(0), counting());
        for kind in [Kind::Pane, Kind::Over] {
            let target = captures
                .get_mut(kind)
                .target_for(&mut pool, &mut alloc, size(400, 300))
                .expect("a target");
            captures.get_mut(kind).drawn(target, nothing(kind));
        }
        captures.keep_only(&[Kind::Pane], &mut pool);
        assert!(
            captures.get_mut(Kind::Over).target().is_none(),
            "the menu closed and the pane kept its capture"
        );
        assert!(captures.get_mut(Kind::Pane).target().is_some());
        assert_eq!(alloc.freed.get(), 1);
    }
}
