//! A pane's captures, each kept in a pooled target between passes.
//!
//! Each records what it was drawn from ([`Inputs`]) and is drawn again only
//! when that differs (Ruling 9); it keeps one id for life and moves its commit
//! only when redrawn, so a capture of a still window is neither drawn nor
//! damaged. `tests::a_redrawn_capture_moves_its_commit_and_keeps_its_id`,
//! `state::tests::real_client::a_capture_whose_surface_tree_has_not_committed_is_not_drawn_again`.

use smithay::{
    backend::renderer::{
        element::{Element, Id},
        gles::GlesTexture,
        utils::CommitCounter,
    },
    utils::{Physical, Rectangle, Scale, Size, Transform},
};

use crate::pool::{Alloc, Pool, Target};

/// What a capture is of. Two kinds never share a texture: a warped pane and a
/// rounded one want different sizes.
/// `tests::a_pane_switching_kinds_lets_the_other_capture_go`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// The whole pane, frame included: a warp's picture.
    Pane,
    /// The client alone: a style's client pass.
    Client,
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

/// Every capture one pane keeps, by kind. Owned by the pane, as
/// `offscreen::Scratch` was. `tests::a_pane_switching_kinds_lets_the_other_capture_go`.
#[derive(Debug)]
pub(crate) struct Captures<T = GlesTexture> {
    pub(crate) pane: Capture<Target<T>>,
    pub(crate) client: Capture<Target<T>>,
}

impl<T> Default for Captures<T> {
    fn default() -> Self {
        Self {
            pane: Capture::default(),
            client: Capture::default(),
        }
    }
}

impl<T: Clone> Captures<T> {
    pub(crate) fn get_mut(&mut self, kind: Kind) -> &mut Capture<Target<T>> {
        match kind {
            Kind::Pane => &mut self.pane,
            Kind::Client => &mut self.client,
        }
    }

    /// Give back every capture but `kind`'s (all of them for `None`): a pane
    /// that captures nothing this pass holds nothing.
    /// `tests::a_pane_that_stops_warping_gives_the_texture_back`,
    /// `tests::a_pane_switching_kinds_lets_the_other_capture_go`.
    pub(crate) fn keep_only(&mut self, kind: Option<Kind>, pool: &mut Pool<T>) {
        for each in [Kind::Pane, Kind::Client] {
            if Some(each) != kind {
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

    /// The kind is in the key: a pane switching between a warp and a client
    /// capture at the same size does not reuse the other's picture.
    #[test]
    fn a_capture_of_another_kind_at_the_same_size_is_stale() {
        let mut capture = Capture::<u32>::default();
        capture.drawn(1, nothing(Kind::Pane));
        assert!(!capture.stale(&nothing(Kind::Pane)));
        assert!(capture.stale(&nothing(Kind::Client)));
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
    /// applied to the texture afterwards, by `warp::mesh`, and `present.rs`'s
    /// first rule is that it never changes the geometry the texture is sized
    /// from. So sixty frames ask for one size, and get one target.
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
        captures.keep_only(None, &mut pool);
        assert_eq!(alloc.freed.get(), 1);
        assert!(captures.get_mut(Kind::Pane).target().is_none());
    }

    /// A pane that goes from a warp to its client pass lets the warp's capture
    /// go, and keeps the other: two kinds at two sizes never share a texture.
    #[test]
    fn a_pane_switching_kinds_lets_the_other_capture_go() {
        let (mut captures, mut pool, mut alloc) =
            (Captures::<Handle>::default(), Pool::new(0), counting());
        let warp = captures
            .get_mut(Kind::Pane)
            .target_for(&mut pool, &mut alloc, size(1150, 900))
            .expect("a target");
        captures
            .get_mut(Kind::Pane)
            .drawn(warp, nothing(Kind::Pane));
        let client = captures
            .get_mut(Kind::Client)
            .target_for(&mut pool, &mut alloc, size(1136, 820))
            .expect("a target");
        captures
            .get_mut(Kind::Client)
            .drawn(client, nothing(Kind::Client));
        captures.keep_only(Some(Kind::Client), &mut pool);
        assert!(captures.get_mut(Kind::Pane).target().is_none());
        assert!(
            captures.get_mut(Kind::Client).target().is_some(),
            "the capture asked for is kept"
        );
        assert_eq!(alloc.freed.get(), 1);
    }
}
