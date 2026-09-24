//! Dwindle, as Hyprland actually does it: a tree, not a formula.
//!
//! The arrangement people mean by "Hyprland tiling" cannot be written as a
//! function of how many windows there are. Read `addTarget` in Hyprland's
//! `DwindleAlgorithm.cpp` and the reason is immediate: a new window splits **a
//! specific existing window** — the one under the mouse, or the focused one —
//! and the split runs across whichever axis *that window's own box* is longer
//! on. Two windows opened in a different order, or with the cursor somewhere
//! else, give different layouts. There is no count to compute from.
//!
//! So this is a binary tree. Leaves are windows; every branch remembers which
//! way it was cut and where. Inserting replaces a leaf with a branch holding
//! the old window and the new one. Removing replaces a branch with whichever
//! child is left, which is what makes closing a window hand its space back to
//! its neighbour rather than reshuffling the screen.
//!
//! Per-branch ratios are the other half of it: resizing a window moves one
//! seam, and everything on the far side of the tree stays exactly where it is.
//!
//! [`Tiling::insert`] splits the tile it is pointed at however small that tile
//! already is, so tiles go on halving for as long as windows keep opening.
//! [`Settings::minimum`] is the floor (#134). `insert` still ignores it, because
//! it is what a layout falls back to when nothing else will do; the two
//! `insert_*` methods beside it refuse a split that would go under it, so a
//! layout can try them first, and the seam movers will not move a seam to take
//! a tile under it either.

use crate::{Rect, Settings};

/// How far under the minimum a tile may come out and still count as at it.
///
/// A split's halves are `f64` arithmetic on a box that has already been
/// through a division or two, so a tile laid out at exactly the minimum can
/// land a rounding error short of it. A millionth of a pixel is far below
/// anything drawn and far above that error.
const SLACK: f64 = 1e-6;

/// Which way a branch was cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Children sit side by side.
    Vertical,
    /// Children sit one above the other.
    Horizontal,
}

impl Axis {
    /// The cut a box would get from [`Tiling::insert`]: across whichever side
    /// is longer, so the split follows the shape of the space being divided.
    fn longer(rect: Rect) -> Self {
        if rect.w >= rect.h {
            Self::Vertical
        } else {
            Self::Horizontal
        }
    }

    /// The other one.
    const fn across(self) -> Self {
        match self {
            Self::Vertical => Self::Horizontal,
            Self::Horizontal => Self::Vertical,
        }
    }
}

/// Which side of a window was grabbed.
///
/// Not an axis, and the difference is the whole of #120. A window that is the
/// right-hand child of a vertical split has that split's seam on its **left**.
/// Dragging its left edge moves that seam; dragging its right edge must find a
/// different seam, or none at all. An axis cannot tell those two apart, so a
/// layout handed only "horizontal" moves whichever seam the walk to the root
/// meets first — which for a right-edge drag is the seam on the far side, and
/// the window's left edge then jumps while the edge under the pointer sits
/// still. Measured: the slot's x moved 209px in one frame while its width did
/// not change at all.
///
/// The compositor has known the side all along — `ResizeEdge` in `state.rs`,
/// which the floating path already uses — and reduced it to a pair of booleans
/// on the way to the layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    /// The low-x side, which a vertical split's seam bounds from the left.
    Left,
    /// The high-x side.
    Right,
    /// The low-y side, which a horizontal split's seam bounds from above.
    Top,
    /// The high-y side.
    Bottom,
}

impl Edge {
    /// The axis a seam along this side is cut on.
    #[must_use]
    pub const fn axis(self) -> Axis {
        match self {
            Self::Left | Self::Right => Axis::Vertical,
            Self::Top | Self::Bottom => Axis::Horizontal,
        }
    }

    /// Which child of a split has that split's seam on this side.
    ///
    /// A split's seam lies *between* its two children, so the child that has
    /// the seam on its left — or above it — is the second one, and the child
    /// that has it on its right or below is the first. This single index is
    /// what turns "a branch cut the right way" into "the branch whose seam is
    /// actually under the pointer".
    const fn child(self) -> usize {
        match self {
            Self::Left | Self::Top => 1,
            Self::Right | Self::Bottom => 0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Node {
    Window {
        id: u64,
    },
    Split {
        axis: Axis,
        ratio: f64,
        children: [usize; 2],
    },
}

/// A dwindle tree.
#[derive(Clone, Debug, Default)]
pub struct Tiling {
    nodes: Vec<Option<Node>>,
    root: Option<usize>,
}

impl Tiling {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    #[must_use]
    pub fn contains(&self, id: u64) -> bool {
        self.leaf(id).is_some()
    }

    /// Every window in the tree, in insertion-independent tree order.
    #[must_use]
    pub fn windows(&self) -> Vec<u64> {
        let mut out = Vec::new();
        if let Some(root) = self.root {
            self.walk(root, &mut out);
        }
        out
    }

    fn walk(&self, index: usize, out: &mut Vec<u64>) {
        match self.nodes.get(index).copied().flatten() {
            Some(Node::Window { id }) => out.push(id),
            Some(Node::Split { children, .. }) => {
                self.walk(children[0], out);
                self.walk(children[1], out);
            }
            None => {}
        }
    }

    /// Add a window by splitting `target`, or the whole area if there is none.
    ///
    /// `at` is where the pointer was. It decides which side of the split the
    /// new window lands on — dropping a window on the right half of another
    /// puts it on the right — which is the part that makes this feel like
    /// placing a window rather than appending to a list.
    ///
    /// Splits whatever it is pointed at, across that box's longer side,
    /// however small the halves come out: [`Settings::minimum`] is not read
    /// here. This is the last resort, `"allow"` in `config.tiling.overflow`;
    /// [`Self::insert_fitting`] is the same placement with the floor on it.
    pub fn insert(
        &mut self,
        id: u64,
        target: Option<u64>,
        at: Option<(f64, f64)>,
        area: Rect,
        settings: Settings,
    ) {
        if self.contains(id) {
            return;
        }
        let Some(root) = self.root else {
            self.root = Some(self.push(Node::Window { id }));
            return;
        };
        let boxes = self.layout(area, settings);
        let target = self.split_target(id, target, at, &boxes).unwrap_or(root);
        let rect = self.box_of(target, &boxes, area);
        self.split(id, target, rect, Axis::longer(rect), at, settings);
    }

    /// [`Self::insert`], unless that would make a tile smaller than
    /// [`Settings::minimum`] — in which case the other axis is tried, and
    /// failing that nothing is done at all.
    ///
    /// The same target as `insert` and the same side of it: this is "open
    /// where the pointer is" with a floor under it, not a different rule. The
    /// other axis first because a box that cannot be divided one way often can
    /// be divided the other — a wide, short tile that has no room side by side
    /// may still have room one above the other — and that is still the tile
    /// the user was pointing at.
    ///
    /// Returns whether the window is in the tree now. `false` leaves the tree
    /// exactly as it was, so the caller can try somewhere else; nothing is
    /// pushed until a split has been chosen, because a node pushed and then
    /// abandoned would be found by [`Self::contains`] for ever after. An empty
    /// tree always has room: its one window takes the whole area whatever
    /// size that is, since there is nothing to divide.
    pub fn insert_fitting(
        &mut self,
        id: u64,
        target: Option<u64>,
        at: Option<(f64, f64)>,
        area: Rect,
        settings: Settings,
    ) -> bool {
        if self.contains(id) {
            return true;
        }
        let Some(root) = self.root else {
            self.root = Some(self.push(Node::Window { id }));
            return true;
        };
        let boxes = self.layout(area, settings);
        let target = self.split_target(id, target, at, &boxes).unwrap_or(root);
        let rect = self.box_of(target, &boxes, area);
        let Some(axis) = room_in(rect, settings) else {
            return false;
        };
        self.split(id, target, rect, axis, at, settings);
        true
    }

    /// Split the largest tile that has room for another window without going
    /// under [`Settings::minimum`]: `"largest"` in `config.tiling.overflow`.
    ///
    /// Largest by area, and the largest *with room* rather than the largest:
    /// a big square tile can fail both ways where a smaller, longer one has
    /// room along its length, and skipping it is the difference between
    /// placing the window and sending it off this workspace. Ties go to the
    /// earlier tile in tree order, so the choice does not depend on anything
    /// the tree does not hold.
    ///
    /// Each tile is tried across its longer side and then the other, as
    /// [`Self::insert_fitting`] tries the one under the pointer. The new
    /// window takes the far side, as it does from `insert` with no pointer:
    /// the pointer is in some other tile by now, so it says nothing about this
    /// one.
    ///
    /// Only windows the tree holds have tiles here. A window a layout took out
    /// at `closing` (#128) has none, and the neighbour that grew into its
    /// space is measured at the size it is now.
    ///
    /// Returns whether the window is in the tree now; `false` leaves the tree
    /// as it was. An empty tree always has room, as for `insert_fitting`.
    pub fn insert_largest(&mut self, id: u64, area: Rect, settings: Settings) -> bool {
        if self.contains(id) {
            return true;
        }
        if self.root.is_none() {
            self.root = Some(self.push(Node::Window { id }));
            return true;
        }
        let mut boxes = self.layout(area, settings);
        // Stable, so equal areas keep tree order.
        boxes.sort_by(|(_, a), (_, b)| (b.w * b.h).total_cmp(&(a.w * a.h)));
        for (other, rect) in boxes {
            if let Some(axis) = room_in(rect, settings)
                && let Some(target) = self.leaf(other)
            {
                self.split(id, target, rect, axis, None, settings);
                return true;
            }
        }
        false
    }

    /// The leaf a new window would split: the one named, else the one under
    /// the pointer, else the one nearest it. `None` only for an empty tree.
    ///
    /// The last step matters more than it looks: falling back to the root
    /// instead means splitting the whole screen, and every new window then
    /// lands as another full-height column — which is not dwindle at all, and
    /// is exactly what this did before. Hyprland calls this `getClosestNode`.
    fn split_target(
        &self,
        id: u64,
        target: Option<u64>,
        at: Option<(f64, f64)>,
        boxes: &[(u64, Rect)],
    ) -> Option<usize> {
        // A window cannot be split by itself, and the caller can easily name
        // it: a new window is mapped and under the pointer before this runs,
        // so hit-testing the cursor answers with the very window being
        // inserted. Back when its node was pushed before this lookup, `leaf`
        // found that node, the branch took it as both children and hung from
        // nothing, and the window vanished without any error at all. The node
        // is pushed after the lookup now, in [`Self::split`], so `leaf` would
        // find nothing; the filter says so outright instead of leaving it to
        // the order of two calls. `a_window_named_as_its_own_target_is_still_added`
        // pins the outcome.
        //
        // Hyprland guards the same case in `addTarget`, calling it a fail-safe
        // and picking a different node. This is that guard.
        target
            .filter(|named| *named != id)
            .and_then(|named| self.leaf(named))
            .or_else(|| self.leaf_at(at, boxes))
            .or_else(|| self.closest_leaf(at, boxes))
    }

    /// The box a split target has now, which is what decides the axis: the
    /// split follows the shape of the space being divided rather than the
    /// shape of the screen.
    fn box_of(&self, target: usize, boxes: &[(u64, Rect)], area: Rect) -> Rect {
        self.id_of(target)
            .and_then(|id| boxes.iter().find(|(other, _)| *other == id))
            .map_or(area, |(_, rect)| *rect)
    }

    /// Hang window `id` beside `target`, which occupies `rect`, across `axis`.
    ///
    /// Everything that decides *whether* and *where* is done by the callers;
    /// this is the one place a node is pushed for a window that is not the
    /// root, so a caller that has decided against a split has pushed nothing.
    fn split(
        &mut self,
        id: u64,
        target: usize,
        rect: Rect,
        axis: Axis,
        at: Option<(f64, f64)>,
        settings: Settings,
    ) {
        let fresh = self.push(Node::Window { id });

        // Which side the new window takes: the half the pointer is in, and the
        // far side by default.
        let second = at.is_none_or(|(x, y)| match axis {
            Axis::Vertical => x >= rect.x + rect.w / 2.0,
            Axis::Horizontal => y >= rect.y + rect.h / 2.0,
        });
        let children = if second {
            [target, fresh]
        } else {
            [fresh, target]
        };

        // Where the target hung, found *before* the branch exists. Looking it
        // up afterwards finds the new branch instead — the branch holds the
        // target as a child — and hanging the branch inside itself makes a
        // cycle. That only shows up from the third window on, because until
        // then the target is the root and the root is handled separately.
        let hung_from = self.parent(target);
        let branch = self.push(Node::Split {
            axis,
            ratio: settings.split.clamp(0.05, 0.95),
            children,
        });
        self.attach(hung_from, target, branch);
    }

    /// Take a window out. Its sibling inherits the space, which is what makes
    /// closing a window a local change rather than a re-tile of the screen.
    pub fn remove(&mut self, id: u64) {
        let Some(leaf) = self.leaf(id) else {
            return;
        };
        let Some(parent) = self.parent(leaf) else {
            self.nodes[leaf] = None;
            self.root = None;
            return;
        };
        let Some(Node::Split { children, .. }) = self.nodes[parent] else {
            return;
        };
        let sibling = if children[0] == leaf {
            children[1]
        } else {
            children[0]
        };
        self.nodes[leaf] = None;
        self.replace(parent, sibling);
        self.nodes[parent] = None;
    }

    /// Put the seam beside a window's `edge` where `edge_at` says that edge
    /// belongs.
    ///
    /// `edge_at` is a position on each axis and only the one `edge` names is
    /// read — `.0` for a left or right side, `.1` for a top or bottom — which
    /// is what lets a corner drag call this twice with one pair and have each
    /// axis take its own edge. The coordinates are the ones [`Self::layout`]
    /// hands back, so "where this window's right edge should be" is directly
    /// comparable with the `x + w` of the slot this same tree produced for it.
    ///
    /// It is **not** where the pointer is, and calling it that was #124. The
    /// two coincide only for a drag that began exactly on the edge, and no
    /// gesture does: a border grab is a band sixteen pixels wide, and the
    /// modifier drag starts from wherever in the window the button went down.
    /// The compositor derives this from the rectangle the drag has produced —
    /// `solium::input::resize::dragged_edge` — so what arrives here has already
    /// moved exactly as far as the pointer has, from exactly where the edge
    /// already was. The arithmetic below did not change for #124 and did not
    /// need to.
    ///
    /// This is the **tiled** resize path: a tiled window has no size of its
    /// own, so an edge drag moves the division it shares with its neighbour
    /// and the compositor never resizes the window directly. The floating path
    /// is the other one — see `Solium::settle_resize`, which only reaches
    /// `hold_resize` when no layout claimed the drag — and the two are
    /// deliberately independent.
    ///
    /// Three things this does that adding a delta to the nearest parent
    /// cannot.
    ///
    /// It finds the seam on the side that was grabbed. [`Self::seam_beside`]
    /// carries the argument; the short version is that "the nearest ancestor
    /// cut on this axis" is not the same branch as "the branch whose seam is
    /// under the pointer", and taking the first for the second is #120.
    ///
    /// It reaches past the immediate parent. A window's parent may have been
    /// cut the other way — in a two-by-two, the seam beside a side edge is two
    /// levels up — which is why width could be dragged in some arrangements
    /// and not others.
    ///
    /// And it is idempotent. The ratio comes from a *position*, not from how
    /// far anything moved, so handling the same drag twice gives the same
    /// layout. Accumulating deltas fed the layout's own response back in as the
    /// next input, and the windows shook themselves apart for as long as the
    /// button was held. A relative gesture and a delta are not the same thing:
    /// #124 made the gesture relative by choosing a better position to send,
    /// and left this idempotence exactly where it was.
    pub fn drag_seam(
        &mut self,
        id: u64,
        edge: Edge,
        edge_at: (f64, f64),
        area: Rect,
        settings: Settings,
    ) {
        let Some(leaf) = self.leaf(id) else {
            return;
        };
        // No seam on that side: the window is flush against its container
        // there, and the thing beyond its edge is the screen, which does not
        // move. Doing nothing is the answer — moving some other seam instead
        // is the whole of the bug this replaced.
        let Some(seam) = self.seam_beside(leaf, edge) else {
            return;
        };

        let Some(rect) = self.node_box(seam, area, settings) else {
            return;
        };
        // Inverted from [`cut`], which is the only thing that decides where an
        // edge actually lands. Along the cut axis it lays out a first child of
        // `(rect.w - gap) * ratio`, then a `gap`-wide band, then the second
        // child — so a seam is not a line but a band, and the two windows
        // beside it have their edges on its two sides:
        //
        //     first child's trailing edge: rect.x + (rect.w - gap) * ratio
        //     second child's leading edge: rect.x + (rect.w - gap) * ratio + gap
        //
        // Solving each for `ratio` gives the two spellings below. The share is
        // of `rect.w - gap` and not of `rect.w` because the band is not part
        // of what the ratio divides, and the leading side subtracts a further
        // `gap` because the second child begins on the far side of it.
        //
        // Dividing by `rect.w` and ignoring the band — what this did — leaves
        // the grabbed edge near the asked-for place rather than on it. With the
        // shipped `gap` of 12, asking for x=1200 on a 1920 screen put a right
        // edge at 1192.5 and a left edge at 1204.5: one seam, grabbed from its
        // two sides, coming to rest a full gap apart. "The edge goes where it
        // was asked to" is the whole of #120, so this arithmetic has to be
        // right and not merely close. It is also why `edge_at` and `rect` have
        // to be measured in the same space, which they are: `node_box` and
        // [`Self::layout`] both start from `area.inset(gap)` and both descend
        // through [`cut`], so a slot's edge and a seam's box are two readings
        // off one arrangement. `dragged_edge_tests` pins that at a real gap,
        // where half a gap of skew would be the whole of #120 again.
        //
        // [`Edge::child`] already answers which of the two sides the hand is
        // on — it is the index [`Self::seam_beside`] matched the window
        // against — so reading it as a distance rather than as an index is
        // what keeps the two in step.
        let gap = settings.gap;
        let leading = if edge.child() == 1 { gap } else { 0.0 };
        let ratio = match edge.axis() {
            Axis::Vertical => (edge_at.0 - rect.x - leading) / (rect.w - gap).max(1.0),
            Axis::Horizontal => (edge_at.1 - rect.y - leading) / (rect.h - gap).max(1.0),
        };
        // **The only clamps on a tiled drag, which is why they have to be
        // here.** `edge_at` arrives unbounded: the compositor sends this pane's
        // own laid-out edge displaced by the pointer, and it deliberately does
        // not floor it. It cannot usefully. `resize::MINIMUM` is a floor on a
        // *window's* size, and applying it to a seam's position put a 120-pixel
        // window's floor on a tile that was narrower than that to begin with --
        // which reported an edge 120px from where it was on the first frame of
        // the drag, before the pointer had moved at all. A ratio of the seam's
        // own box is the quantity this layout actually owns, and the box is not
        // the window.
        //
        // Two bounds on that ratio, and [`Self::room`] works out both. One is
        // 0.05..0.95, because a seam driven to either end of its box leaves a
        // child of no width, and a window of no width cannot be grabbed again
        // to undo it. The other, since #134, is `settings.minimum`: no tile on
        // either side of the seam is taken under it, so a seam cannot be
        // dragged to make a tile smaller than a new window is allowed to open
        // in. A tile, not a window, so it is this layout's to hold -- and it
        // bends for a side that is already under it rather than snapping that
        // side up to it, which is what keeps the first frame of such a drag
        // where it was. See `room`.
        //
        // Neither is #115, which is about a client's own `min_size`: that is
        // still unread, and is a different quantity against a different
        // rectangle.
        //
        // This is also the tighter of two clamps on a *floating* drag, where
        // `resize::MINIMUM` does apply -- to the window, in `resized`, for the
        // floating path's own rectangle. The two never meet on one number.
        let (low, high) = self.room(seam, rect, settings);
        if let Some(Node::Split { ratio: current, .. }) = self.nodes[seam].as_mut() {
            *current = within(ratio, low, high, *current);
        }
    }

    /// The ratios a seam may take: 0.05..0.95, narrowed so that no tile on
    /// either side of it is made smaller than [`Settings::minimum`].
    ///
    /// `rect` is the seam's own box, from [`Self::node_box`]. Each side needs
    /// [`Self::floor`] of it along the seam's axis, and a side's share of the
    /// box less the gap is what the ratio divides, so the floors turn into
    /// ratios by the same arithmetic [`cut`] runs forwards.
    ///
    /// **A side already under its floor is held where it is rather than moved
    /// up to it.** Tiles under the minimum are ordinary: `"allow"` makes them,
    /// a monitor change or a smaller work area can, and so can raising the
    /// minimum in a running session. Clamping such a seam into its range on
    /// the first frame of a drag would throw it across the screen before the
    /// pointer had moved -- the #124 class of bug, reached through a floor
    /// instead of a skew. So the range is widened to take in the ratio the
    /// seam has now: that side can be grown, and cannot be made smaller still.
    /// With both sides under, the seam cannot move at all, because any move
    /// shrinks one of them.
    ///
    /// No minimum on the seam's axis leaves exactly 0.05..0.95, which is what
    /// this clamp was before #134.
    fn room(&self, seam: usize, rect: Rect, settings: Settings) -> (f64, f64) {
        let Some(Node::Split {
            axis,
            ratio,
            children,
        }) = self.nodes.get(seam).copied().flatten()
        else {
            return (0.05, 0.95);
        };
        let least = match axis {
            Axis::Vertical => settings.minimum.w,
            Axis::Horizontal => settings.minimum.h,
        };
        // NaN is spelled out because it compares false both ways, and a NaN
        // floor would make every ratio below NaN too.
        if least.is_nan() || least <= 0.0 {
            return (0.05, 0.95);
        }
        let span = match axis {
            Axis::Vertical => rect.w - settings.gap,
            Axis::Horizontal => rect.h - settings.gap,
        }
        .max(1.0);
        let low = self.floor(children[0], axis, least, settings.gap) / span;
        let high = 1.0 - self.floor(children[1], axis, least, settings.gap) / span;
        (low.min(ratio).max(0.05), high.max(ratio).min(0.95))
    }

    /// The least room along `axis` the subtree at `index` needs for every tile
    /// in it to be at least `least` long on that axis, with its own seams
    /// where they are now.
    ///
    /// At the ratios it has rather than at the best ratios it could have: a
    /// drag moves one seam, so a subtree's inner seams stay where they are and
    /// its tiles grow and shrink in proportion. A branch cut along `axis`
    /// needs each child's floor divided by that child's share, plus the gap
    /// between them. A branch cut the other way gives each child the whole of
    /// its extent along `axis`, so it needs only the larger of the two floors
    /// -- a column of stacked windows is as narrow as its widest floor, not
    /// the sum of them.
    fn floor(&self, index: usize, axis: Axis, least: f64, gap: f64) -> f64 {
        match self.nodes.get(index).copied().flatten() {
            Some(Node::Window { .. }) => least,
            Some(Node::Split {
                axis: cut_on,
                ratio,
                children,
            }) => {
                let first = self.floor(children[0], axis, least, gap);
                let second = self.floor(children[1], axis, least, gap);
                if cut_on == axis {
                    // Clamped as `cut` clamps it, since that is the share the
                    // children are actually laid out at.
                    let ratio = ratio.clamp(0.05, 0.95);
                    gap + (first / ratio).max(second / (1.0 - ratio))
                } else {
                    first.max(second)
                }
            }
            None => 0.0,
        }
    }

    /// The branch whose seam runs along `edge` of the window at `leaf`.
    ///
    /// Two conditions, and the old walk checked only the first. The branch has
    /// to be cut along the dragged axis, *and* this subtree has to be on the
    /// side of it that puts the seam under the grabbed edge — which is
    /// [`Edge::child`]. A window that is the second child of a vertical split
    /// has that seam on its left and nowhere else; matching it for a
    /// right-edge drag moves the window's far side while the pointer's side
    /// stands still.
    ///
    /// The walk continues towards the root rather than stopping at the first
    /// branch on the axis, because a branch on the wrong side is not a
    /// near-miss: the seam beside a right edge may be any number of levels up,
    /// past ancestors cut the other way and past ancestors cut the same way
    /// that this subtree happens to sit on the far side of.
    ///
    /// `None` when the walk reaches the root having found no such branch,
    /// which means exactly one thing: the window is flush against its
    /// container on that side, and there is no seam there to move.
    fn seam_beside(&self, leaf: usize, edge: Edge) -> Option<usize> {
        let axis = edge.axis();
        let side = edge.child();
        let mut node = leaf;
        while let Some(parent) = self.parent(node) {
            if matches!(
                self.nodes[parent],
                Some(Node::Split { axis: cut, children, .. })
                    if cut == axis && children[side] == node
            ) {
                return Some(parent);
            }
            node = parent;
        }
        None
    }

    /// The rectangle a node occupies, branches included.
    fn node_box(&self, wanted: usize, area: Rect, settings: Settings) -> Option<Rect> {
        fn find(
            tree: &Tiling,
            index: usize,
            rect: Rect,
            wanted: usize,
            settings: Settings,
        ) -> Option<Rect> {
            if index == wanted {
                return Some(rect);
            }
            match tree.nodes.get(index).copied().flatten() {
                Some(Node::Split {
                    axis,
                    ratio,
                    children,
                }) => {
                    let (first, second) = cut(rect, axis, ratio, settings.gap);
                    find(tree, children[0], first, wanted, settings)
                        .or_else(|| find(tree, children[1], second, wanted, settings))
                }
                _ => None,
            }
        }
        let root = self.root?;
        find(self, root, area.inset(settings.gap), wanted, settings)
    }

    /// Grow a window along `axis` by a fraction, moving one seam. Everything
    /// on the far side stays put.
    ///
    /// The keyboard path. It takes an axis and not an [`Edge`] because a
    /// keypress names neither side: `super+equal` means "make this wider", and
    /// which of the two seams beside it gives up the space is not something
    /// the user expressed. A drag is the opposite — the hand is on one
    /// specific edge — which is why the two take different arguments.
    ///
    /// So it prefers the trailing seam (right, or below) and falls back to the
    /// leading one, with the sign flipped so that positive `by` still grows
    /// the window either way. Only a window with no seam on either side —
    /// which on that axis means the only window on the screen — does nothing.
    ///
    /// This used to adjust the immediate parent and nothing else, which is the
    /// same shortcut [`Self::drag_seam`] made: in an ordinary four-window
    /// dwindle every leaf's parent is a horizontal split, so the centre
    /// vertical seam could not be reached from the keyboard at all and
    /// `super+equal` silently changed the height instead.
    ///
    /// Bounded as a drag is, by [`Self::room`]: a press does not make a tile
    /// smaller than [`Settings::minimum`], nor one already under it any
    /// smaller. That is why this takes the area and the settings, which it did
    /// not need before #134 -- the floor is in pixels, and the seam's box is
    /// what turns pixels into a ratio.
    pub fn resize(&mut self, id: u64, axis: Axis, by: f64, area: Rect, settings: Settings) {
        let Some(leaf) = self.leaf(id) else {
            return;
        };
        let (trailing, leading) = match axis {
            Axis::Vertical => (Edge::Right, Edge::Left),
            Axis::Horizontal => (Edge::Bottom, Edge::Top),
        };
        // A seam on the trailing side is the first child's, so raising its
        // ratio grows this window; a seam on the leading side is the second
        // child's, where the same move shrinks it. Hence the negation, which
        // is the one thing about this that reads backwards.
        let (seam, towards) = match self.seam_beside(leaf, trailing) {
            Some(seam) => (seam, by),
            None => match self.seam_beside(leaf, leading) {
                Some(seam) => (seam, -by),
                None => return,
            },
        };
        // `seam_beside` only returns a branch it reached by walking up from a
        // leaf of this tree, so `node_box` finds it walking down; the whole
        // area is a fallback that keeps some bound rather than none.
        let rect = self.node_box(seam, area, settings).unwrap_or(area);
        let (low, high) = self.room(seam, rect, settings);
        if let Some(Node::Split { ratio, .. }) = self.nodes[seam].as_mut() {
            *ratio = within(*ratio + towards, low, high, *ratio);
        }
    }

    /// Where every window goes.
    #[must_use]
    pub fn layout(&self, area: Rect, settings: Settings) -> Vec<(u64, Rect)> {
        let mut out = Vec::new();
        if let Some(root) = self.root {
            self.place(root, area.inset(settings.gap), settings, &mut out);
        }
        out
    }

    fn place(&self, index: usize, rect: Rect, settings: Settings, out: &mut Vec<(u64, Rect)>) {
        match self.nodes.get(index).copied().flatten() {
            Some(Node::Window { id }) => out.push((id, rect)),
            Some(Node::Split {
                axis,
                ratio,
                children,
            }) => {
                let (first, second) = cut(rect, axis, ratio, settings.gap);
                self.place(children[0], first, settings, out);
                self.place(children[1], second, settings, out);
            }
            None => {}
        }
    }

    fn push(&mut self, node: Node) -> usize {
        self.nodes.push(Some(node));
        self.nodes.len() - 1
    }

    fn leaf(&self, id: u64) -> Option<usize> {
        self.nodes
            .iter()
            .position(|node| matches!(node, Some(Node::Window { id: other }) if *other == id))
    }

    fn id_of(&self, index: usize) -> Option<u64> {
        match self.nodes.get(index).copied().flatten() {
            Some(Node::Window { id }) => Some(id),
            _ => None,
        }
    }

    /// The window a point lands in.
    fn leaf_at(&self, at: Option<(f64, f64)>, boxes: &[(u64, Rect)]) -> Option<usize> {
        let (x, y) = at?;
        boxes
            .iter()
            .find(|(_, rect)| rect.contains(x, y))
            .and_then(|(id, _)| self.leaf(*id))
    }

    /// The window nearest a point, by distance from its centre.
    ///
    /// Used when the pointer is over no window at all — off the edge, or on a
    /// gap. There is always a window to split; the question is only which.
    fn closest_leaf(&self, at: Option<(f64, f64)>, boxes: &[(u64, Rect)]) -> Option<usize> {
        let (x, y) = at.unwrap_or_else(|| {
            boxes
                .first()
                .map_or((0.0, 0.0), |(_, rect)| (rect.x, rect.y))
        });
        boxes
            .iter()
            .min_by(|(_, a), (_, b)| {
                let distance = |rect: &Rect| {
                    let dx = rect.x + rect.w / 2.0 - x;
                    let dy = rect.y + rect.h / 2.0 - y;
                    dx.mul_add(dx, dy * dy)
                };
                distance(a).total_cmp(&distance(b))
            })
            .and_then(|(id, _)| self.leaf(*id))
    }

    fn parent(&self, index: usize) -> Option<usize> {
        self.nodes.iter().position(
            |node| matches!(node, Some(Node::Split { children, .. }) if children.contains(&index)),
        )
    }

    /// Put `with` wherever `what` was.
    fn replace(&mut self, what: usize, with: usize) {
        let parent = self.parent(what);
        self.attach(parent, what, with);
    }

    /// Put `with` where `what` hung, given the parent it hung from.
    ///
    /// Taking the parent as an argument rather than looking it up is what
    /// makes this safe to call once the replacement already references the
    /// thing being replaced.
    fn attach(&mut self, parent: Option<usize>, what: usize, with: usize) {
        let Some(parent) = parent else {
            if self.root == Some(what) {
                self.root = Some(with);
            }
            return;
        };
        if let Some(Node::Split { children, .. }) = self.nodes[parent].as_mut() {
            for child in children.iter_mut() {
                if *child == what {
                    *child = with;
                }
            }
        }
    }
}

/// Which way `rect` can be split with neither half under
/// [`Settings::minimum`]: across its longer side if that has room, else across
/// the other, else neither.
///
/// Measured with [`cut`] itself, at the configured split and gap, so "has
/// room" means exactly what the layout will then draw. Both halves on both
/// sides: a split across the width leaves the height alone, and a tile that is
/// already too short does not become tall enough by being divided.
fn room_in(rect: Rect, settings: Settings) -> Option<Axis> {
    let longer = Axis::longer(rect);
    [longer, longer.across()].into_iter().find(|axis| {
        let (first, second) = cut(rect, *axis, settings.split, settings.gap);
        [first, second].iter().all(|half| {
            half.w + SLACK >= settings.minimum.w && half.h + SLACK >= settings.minimum.h
        })
    })
}

/// `value` held to `low..=high`, or `otherwise` when that range is empty.
///
/// Not `f64::clamp` alone, which panics when `low > high` or either is NaN --
/// and a compositor that panics takes the session with it. [`Tiling::room`]
/// always includes the current ratio, so its range is empty only when that
/// ratio is itself NaN; `otherwise` is then the seam left as it was.
fn within(value: f64, low: f64, high: f64, otherwise: f64) -> f64 {
    if low <= high {
        value.clamp(low, high)
    } else {
        otherwise
    }
}

fn cut(rect: Rect, axis: Axis, ratio: f64, gap: f64) -> (Rect, Rect) {
    let ratio = ratio.clamp(0.05, 0.95);
    match axis {
        Axis::Vertical => {
            let first = ((rect.w - gap) * ratio).max(1.0);
            let second = (rect.w - gap - first).max(1.0);
            (
                Rect::new(rect.x, rect.y, first, rect.h),
                Rect::new(rect.x + first + gap, rect.y, second, rect.h),
            )
        }
        Axis::Horizontal => {
            let first = ((rect.h - gap) * ratio).max(1.0);
            let second = (rect.h - gap - first).max(1.0);
            (
                Rect::new(rect.x, rect.y, rect.w, first),
                Rect::new(rect.x, rect.y + first + gap, rect.w, second),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Axis, Tiling};
    use crate::{Rect, Settings};

    fn area() -> Rect {
        Rect::new(0.0, 0.0, 1000.0, 600.0)
    }

    fn settings() -> Settings {
        Settings {
            gap: 0.0,
            split: 0.5,
            ..Settings::default()
        }
    }

    fn rect_of(tiling: &Tiling, id: u64) -> Rect {
        tiling
            .layout(area(), settings())
            .into_iter()
            .find(|(other, _)| *other == id)
            .expect("window is in the tree")
            .1
    }

    #[test]
    fn the_first_window_takes_everything() {
        let mut tiling = Tiling::new();
        tiling.insert(1, None, None, area(), settings());
        assert!((rect_of(&tiling, 1).w - 1000.0).abs() < 1.0);
    }

    /// The split runs across the longer axis of *the window being split*, not
    /// of the screen. This is the property a count-based spiral cannot have,
    /// because it never knows which window is being divided.
    #[test]
    fn a_wide_window_splits_side_by_side_and_a_tall_one_does_not() {
        let mut tiling = Tiling::new();
        tiling.insert(1, None, None, area(), settings());
        tiling.insert(2, Some(1), None, area(), settings());
        assert!(
            (rect_of(&tiling, 1).w - 500.0).abs() < 1.0,
            "cut down the middle"
        );

        // Window 2 is now 500x600 — taller than wide — so splitting *it* cuts
        // across, not down.
        tiling.insert(3, Some(2), None, area(), settings());
        let two = rect_of(&tiling, 2);
        let three = rect_of(&tiling, 3);
        assert!((two.w - 500.0).abs() < 1.0, "width untouched");
        assert!((two.h - 300.0).abs() < 1.0, "split horizontally: {two:?}");
        assert!(three.y > two.y, "the new window went below");
    }

    /// Where the pointer is decides which side the window lands on. This is
    /// what "the window opens where the cursor is" means, and it is the whole
    /// difference between placing a window and appending to a list.
    #[test]
    fn the_pointer_decides_which_side_the_new_window_takes() {
        let mut left = Tiling::new();
        left.insert(1, None, None, area(), settings());
        left.insert(2, Some(1), Some((100.0, 300.0)), area(), settings());
        assert!(
            rect_of(&left, 2).x < rect_of(&left, 1).x,
            "dropped left, opens left"
        );

        let mut right = Tiling::new();
        right.insert(1, None, None, area(), settings());
        right.insert(2, Some(1), Some((900.0, 300.0)), area(), settings());
        assert!(
            rect_of(&right, 2).x > rect_of(&right, 1).x,
            "dropped right, opens right"
        );
    }

    /// With no target named, the window under the pointer is the one that
    /// gets split — which is how Hyprland picks `OPENINGON`.
    #[test]
    fn with_no_target_the_window_under_the_pointer_is_split() {
        let mut tiling = Tiling::new();
        tiling.insert(1, None, None, area(), settings());
        tiling.insert(2, Some(1), Some((900.0, 300.0)), area(), settings());
        // Point into window 1's half and open a third with no explicit target.
        tiling.insert(3, None, Some((100.0, 300.0)), area(), settings());
        assert!(
            (rect_of(&tiling, 2).w - 500.0).abs() < 1.0,
            "the untouched half kept its width"
        );
        // Window 1 was 500x600 — taller than wide — so dividing it cuts
        // across: its width is untouched and its height halves.
        let one = rect_of(&tiling, 1);
        assert!((one.w - 500.0).abs() < 1.0, "width unchanged: {one:?}");
        assert!(
            (one.h - 300.0).abs() < 1.0,
            "the pointed-at window was divided: {one:?}"
        );
    }

    /// Closing a window hands its space to its neighbour instead of re-tiling
    /// the screen. Everything else must stay exactly where it was.
    #[test]
    fn removing_a_window_gives_its_space_to_its_sibling() {
        let mut tiling = Tiling::new();
        tiling.insert(1, None, None, area(), settings());
        tiling.insert(2, Some(1), None, area(), settings());
        tiling.insert(3, Some(2), None, area(), settings());
        let before = rect_of(&tiling, 1);

        tiling.remove(3);
        assert_eq!(tiling.windows().len(), 2);
        let after = rect_of(&tiling, 1);
        assert!(
            (before.w - after.w).abs() < 1.0,
            "the far side did not move"
        );
        assert!(
            (rect_of(&tiling, 2).h - 600.0).abs() < 1.0,
            "the sibling took the space"
        );
    }

    #[test]
    fn removing_the_last_window_empties_the_tree() {
        let mut tiling = Tiling::new();
        tiling.insert(1, None, None, area(), settings());
        tiling.remove(1);
        assert!(tiling.is_empty());
        assert!(tiling.layout(area(), settings()).is_empty());
    }

    /// Resizing moves one seam. A tree that re-derived every box from a count
    /// could not do this at all.
    #[test]
    fn resizing_moves_one_seam_and_leaves_the_rest() {
        let mut tiling = Tiling::new();
        tiling.insert(1, None, None, area(), settings());
        tiling.insert(2, Some(1), None, area(), settings());
        tiling.insert(3, Some(2), None, area(), settings());
        let untouched = rect_of(&tiling, 1);

        tiling.resize(2, Axis::Horizontal, 0.2, area(), settings());
        assert!(rect_of(&tiling, 2).h > 300.0, "the seam moved");
        assert!(
            (rect_of(&tiling, 1).w - untouched.w).abs() < 1.0,
            "the other branch is untouched"
        );
    }

    #[test]
    fn every_window_gets_a_box_and_none_collapses() {
        let mut tiling = Tiling::new();
        for id in 1..=10 {
            tiling.insert(id, Some(id.saturating_sub(1)), None, area(), settings());
        }
        let boxes = tiling.layout(area(), settings());
        assert_eq!(boxes.len(), 10);
        for (id, rect) in boxes {
            assert!(
                rect.w >= 1.0 && rect.h >= 1.0,
                "window {id} collapsed: {rect:?}"
            );
        }
    }

    #[test]
    fn a_window_is_never_added_twice() {
        let mut tiling = Tiling::new();
        tiling.insert(1, None, None, area(), settings());
        tiling.insert(1, None, None, area(), settings());
        assert_eq!(tiling.windows(), vec![1]);
    }

    #[test]
    fn axes_are_named_for_how_the_children_sit() {
        assert_ne!(Axis::Vertical, Axis::Horizontal);
    }
}

#[cfg(test)]
mod fallback_tests {
    use super::Tiling;
    use crate::{Rect, Settings};

    fn area() -> Rect {
        Rect::new(0.0, 0.0, 1000.0, 600.0)
    }
    fn settings() -> Settings {
        Settings {
            gap: 0.0,
            split: 0.5,
            ..Settings::default()
        }
    }

    /// With the pointer nowhere near a window, the split must still land on a
    /// *window*. Falling back to the root splits the whole screen instead, and
    /// every window after the first then becomes another full-height column —
    /// three terminals in a row rather than a dwindle.
    #[test]
    fn a_pointer_off_the_edge_still_divides_a_window_not_the_screen() {
        let mut tiling = Tiling::new();
        tiling.insert(1, None, None, area(), settings());
        tiling.insert(2, None, Some((-500.0, -500.0)), area(), settings());
        tiling.insert(3, None, Some((-500.0, -500.0)), area(), settings());

        let boxes = tiling.layout(area(), settings());
        let full_height = boxes.iter().filter(|(_, r)| r.h > 599.0).count();
        assert!(
            full_height < 3,
            "all three are full-height columns, so the screen was split each time: {boxes:?}"
        );
    }
}

#[cfg(test)]
mod insert_sequence {
    use super::Tiling;
    use crate::{Rect, Settings};

    /// The exact sequence the tiling script performs: a work area with a gap,
    /// a pointer that is outside every window, and no named target — so the
    /// closest-leaf fallback chooses. Three windows opened this way must all
    /// be in the tree.
    #[test]
    fn opening_three_windows_at_the_origin_keeps_all_three() {
        let area = Rect::new(0.0, 0.0, 1600.0, 900.0);
        let settings = Settings {
            gap: 12.0,
            split: 0.5,
            ..Settings::default()
        };
        let mut tiling = Tiling::new();
        for id in 1..=3 {
            tiling.insert(id, None, Some((0.0, 0.0)), area, settings);
            println!("after {id}: {:?}", tiling.windows());
        }
        assert_eq!(tiling.windows().len(), 3, "got {:?}", tiling.windows());
    }
}

#[cfg(test)]
mod self_target {
    use super::Tiling;
    use crate::{Rect, Settings};

    fn area() -> Rect {
        Rect::new(0.0, 0.0, 1600.0, 900.0)
    }
    fn settings() -> Settings {
        Settings {
            gap: 12.0,
            split: 0.5,
            ..Settings::default()
        }
    }

    /// Naming the window being inserted as its own split target must not lose
    /// it. The tiling script does exactly this by accident: a new window is
    /// mapped and under the pointer before the layout runs, so hit-testing the
    /// cursor answers with the window being opened. Every window after the
    /// first was silently dropped — the tree kept one, and three terminals
    /// showed one.
    #[test]
    fn a_window_named_as_its_own_target_is_still_added() {
        let mut tiling = Tiling::new();
        for id in 1..=4 {
            tiling.insert(id, Some(id), Some((0.0, 0.0)), area(), settings());
        }
        assert_eq!(
            tiling.windows().len(),
            4,
            "windows were dropped: {:?}",
            tiling.windows()
        );
        for (id, rect) in tiling.layout(area(), settings()) {
            assert!(rect.w >= 1.0 && rect.h >= 1.0, "window {id} collapsed");
        }
    }
}

#[cfg(test)]
mod seam_tests {
    use super::{Edge, Tiling};
    use crate::{Rect, Settings};

    fn area() -> Rect {
        Rect::new(0.0, 0.0, 1000.0, 600.0)
    }
    fn settings() -> Settings {
        Settings {
            gap: 0.0,
            split: 0.5,
            ..Settings::default()
        }
    }
    fn rect_of(tiling: &Tiling, id: u64) -> Rect {
        tiling
            .layout(area(), settings())
            .into_iter()
            .find(|(other, _)| *other == id)
            .expect("in the tree")
            .1
    }

    /// A two-by-two: every window's immediate parent is a horizontal split, so
    /// dragging a side edge has to reach a seam two levels up. Adjusting only
    /// the parent does nothing at all, which is why width could be dragged in
    /// some arrangements and not this one.
    fn quad() -> Tiling {
        let mut tiling = Tiling::new();
        tiling.insert(1, None, None, area(), settings());
        tiling.insert(2, Some(1), None, area(), settings());
        tiling.insert(3, Some(1), None, area(), settings());
        tiling.insert(4, Some(2), None, area(), settings());
        tiling
    }

    #[test]
    fn a_two_by_two_can_have_its_width_dragged() {
        let mut tiling = quad();
        let before = rect_of(&tiling, 1).w;
        assert!((before - 500.0).abs() < 1.0, "starts halved: {before}");

        tiling.drag_seam(1, Edge::Right, (700.0, 300.0), area(), settings());
        let after = rect_of(&tiling, 1).w;
        assert!(
            (after - 700.0).abs() < 2.0,
            "the seam went where it was asked: {after}"
        );
    }

    /// Dragging to the same place twice gives the same layout. Deltas fed the
    /// layout's own response back in and the windows shook themselves apart.
    #[test]
    fn dragging_to_the_same_place_twice_is_the_same_layout() {
        let mut tiling = quad();
        tiling.drag_seam(1, Edge::Right, (650.0, 300.0), area(), settings());
        let once = rect_of(&tiling, 1).w;
        for _ in 0..20 {
            tiling.drag_seam(1, Edge::Right, (650.0, 300.0), area(), settings());
        }
        let many = rect_of(&tiling, 1).w;
        assert!((once - many).abs() < f64::EPSILON, "{once} then {many}");
    }

    #[test]
    fn height_drags_find_the_horizontal_seam() {
        let mut tiling = quad();
        tiling.drag_seam(1, Edge::Bottom, (250.0, 400.0), area(), settings());
        let one = rect_of(&tiling, 1);
        assert!(
            (one.h - 400.0).abs() < 2.0,
            "went where it was asked: {one:?}"
        );
    }

    #[test]
    fn a_lone_window_has_no_seam_to_drag() {
        let mut tiling = Tiling::new();
        tiling.insert(1, None, None, area(), settings());
        tiling.drag_seam(1, Edge::Right, (700.0, 300.0), area(), settings());
        assert!((rect_of(&tiling, 1).w - 1000.0).abs() < 1.0, "unchanged");
    }
}

/// #120: the seam that moves is the one beside the edge that was grabbed.
///
/// The measurement this pins came off a real drag. Dragging one edge of a
/// tiled window slowly, the drag's own rectangle moved smoothly — 808, 809,
/// 817, 835 — while the slot the layout produced went 806, 1015, 1016, 1024:
/// a 209px jump in one frame, three frames of unchanged width, then another
/// leap. That is the signature of moving the seam on the *far* side, because
/// the far side moving is the near side standing still.
///
/// Every test below is written against [`quad`], where each window's immediate
/// parent is cut the opposite way to the split its side edges touch, so
/// "nearest ancestor on the axis" and "seam beside the edge" are different
/// branches and the difference is visible in the rectangles.
#[cfg(test)]
mod dragged_edge_tests {
    use super::{Axis, Edge, Tiling};
    use crate::{Rect, Settings};

    fn area() -> Rect {
        Rect::new(0.0, 0.0, 1000.0, 600.0)
    }
    fn settings() -> Settings {
        Settings {
            gap: 0.0,
            split: 0.5,
            ..Settings::default()
        }
    }
    fn rect_of(tiling: &Tiling, id: u64) -> Rect {
        tiling
            .layout(area(), settings())
            .into_iter()
            .find(|(other, _)| *other == id)
            .expect("in the tree")
            .1
    }
    fn every_rect(tiling: &Tiling) -> Vec<(u64, Rect)> {
        let mut out = tiling.layout(area(), settings());
        out.sort_by_key(|(id, _)| *id);
        out
    }

    /// Two columns of two. Window 1 is top-left, 2 top-right, 3 bottom-left,
    /// 4 bottom-right; the columns are divided by one vertical seam at the
    /// root, and each column by a horizontal seam of its own.
    ///
    /// The same shape as `seam_tests::quad`, restated here so this module
    /// reads on its own — the arrangement *is* the argument, and a fixture
    /// imported from elsewhere is one the reader has to go and look up.
    fn quad() -> Tiling {
        let mut tiling = Tiling::new();
        tiling.insert(1, None, None, area(), settings());
        tiling.insert(2, Some(1), None, area(), settings());
        tiling.insert(3, Some(1), None, area(), settings());
        tiling.insert(4, Some(2), None, area(), settings());
        tiling
    }

    /// Window 1 is the first child of the root's vertical split, so that seam
    /// is on its right. Dragging the right edge to 700 has to leave the left
    /// edge at 0 — the failure being pinned is precisely a left edge that
    /// moves instead — and has to carry window 3 with it, because they share
    /// the column the root seam bounds.
    #[test]
    fn a_right_edge_drag_moves_the_seam_on_the_right() {
        let mut tiling = quad();
        tiling.drag_seam(1, Edge::Right, (700.0, 150.0), area(), settings());

        let one = rect_of(&tiling, 1);
        assert!(
            (one.x - 0.0).abs() < 1.0,
            "the left edge stood still: {one:?}"
        );
        assert!(
            (one.w - 700.0).abs() < 2.0,
            "the right edge followed: {one:?}"
        );
        assert!(
            (rect_of(&tiling, 3).w - 700.0).abs() < 2.0,
            "the root seam moved, so the whole column did"
        );
        assert!((one.h - 300.0).abs() < 1.0, "nothing horizontal moved");
    }

    /// The mirror. Window 2 is the *second* child of the root's vertical
    /// split, so that same seam is on its left, and a left-edge drag is what
    /// moves it. Its right edge must stay against the container at 1000.
    #[test]
    fn a_left_edge_drag_moves_the_seam_on_the_left() {
        let mut tiling = quad();
        tiling.drag_seam(2, Edge::Left, (700.0, 150.0), area(), settings());

        let two = rect_of(&tiling, 2);
        assert!(
            (two.x - 700.0).abs() < 2.0,
            "the left edge followed: {two:?}"
        );
        assert!(
            (two.x + two.w - 1000.0).abs() < 2.0,
            "the right edge stood still: {two:?}"
        );
    }

    /// Window 1 is the first child of its column's horizontal split, so that
    /// seam is below it. Its top edge is the container's, and its width is
    /// bounded by a seam that a vertical drag must not touch.
    #[test]
    fn a_bottom_edge_drag_moves_the_seam_below() {
        let mut tiling = quad();
        tiling.drag_seam(1, Edge::Bottom, (250.0, 400.0), area(), settings());

        let one = rect_of(&tiling, 1);
        assert!(
            (one.y - 0.0).abs() < 1.0,
            "the top edge stood still: {one:?}"
        );
        assert!(
            (one.h - 400.0).abs() < 2.0,
            "the bottom edge followed: {one:?}"
        );
        assert!((one.w - 500.0).abs() < 1.0, "nothing vertical moved");
        assert!(
            (rect_of(&tiling, 2).h - 300.0).abs() < 1.0,
            "the other column's seam is a different seam and did not move"
        );
    }

    /// And the mirror of that: window 3 is the second child of the left
    /// column's split, so the seam is above it and its bottom edge is the
    /// container's.
    #[test]
    fn a_top_edge_drag_moves_the_seam_above() {
        let mut tiling = quad();
        tiling.drag_seam(3, Edge::Top, (250.0, 200.0), area(), settings());

        let three = rect_of(&tiling, 3);
        assert!(
            (three.y - 200.0).abs() < 2.0,
            "the top edge followed: {three:?}"
        );
        assert!(
            (three.y + three.h - 600.0).abs() < 2.0,
            "the bottom edge stood still: {three:?}"
        );
        assert!(
            (rect_of(&tiling, 1).h - 200.0).abs() < 2.0,
            "the neighbour above gave up the space"
        );
    }

    /// The four cases where the grabbed edge is the container's own, which is
    /// where the old walk did its damage: it found the nearest ancestor cut on
    /// the axis regardless of which side of it the window sat, so every one of
    /// these dragged the seam on the *opposite* side and the window jumped.
    ///
    /// Nothing at all is the right answer. The thing beyond that edge is the
    /// screen, and the screen does not move.
    #[test]
    fn a_window_against_the_container_has_no_seam_on_that_side() {
        // Window 1 is top-left: its left edge and its top edge are the
        // container's. Window 2 is top-right, so its right edge is; window 3
        // is bottom-left, so its bottom edge is.
        let cases = [
            (1_u64, Edge::Left, (200.0, 150.0)),
            (1, Edge::Top, (250.0, 200.0)),
            (2, Edge::Right, (800.0, 150.0)),
            (3, Edge::Bottom, (250.0, 450.0)),
        ];
        for (id, edge, at) in cases {
            let mut tiling = quad();
            let before = every_rect(&tiling);
            tiling.drag_seam(id, edge, at, area(), settings());
            assert_eq!(
                every_rect(&tiling),
                before,
                "dragging window {id}'s {edge:?} edge moved something"
            );
        }
    }

    /// A three-level tree, where the seam beside an edge is provably *not* the
    /// nearest ancestor cut on that axis — the case the two-by-two cannot show
    /// because there the nearest ancestor is the only one.
    ///
    /// Splitting window 4 again gives the bottom-right corner its own vertical
    /// seam. Window 4's right edge touches that inner seam; its *left* edge
    /// touches the root's, two branches further up, past a horizontal split
    /// and past a vertical one it is on the wrong side of. Walking to the
    /// nearest vertical ancestor finds the inner seam for both.
    fn nested() -> Tiling {
        let mut tiling = quad();
        tiling.insert(5, Some(4), None, area(), settings());
        tiling
    }

    #[test]
    fn the_seam_beside_an_edge_is_not_always_the_nearest_ancestor() {
        let mut tiling = nested();
        assert!(
            (rect_of(&tiling, 4).w - 250.0).abs() < 1.0,
            "the corner was split again: {:?}",
            rect_of(&tiling, 4)
        );

        // The left edge of window 4 is the root's seam. Moving it to 700 has
        // to widen the left column to 700 and leave window 5 — on the far side
        // of the inner seam — with its right edge still against the container.
        tiling.drag_seam(4, Edge::Left, (700.0, 450.0), area(), settings());
        assert!(
            (rect_of(&tiling, 1).w - 700.0).abs() < 2.0,
            "the root seam moved: {:?}",
            rect_of(&tiling, 1)
        );
        let five = rect_of(&tiling, 5);
        assert!(
            (five.x + five.w - 1000.0).abs() < 2.0,
            "the far side of the inner seam is still against the edge: {five:?}"
        );
    }

    /// Window 5 sits in the bottom-right corner, so its right edge is the
    /// container's even though it has a vertical seam on its other side. The
    /// nearest-ancestor walk finds that inner seam and drags it, which moves
    /// the window's left edge for a drag on its right.
    #[test]
    fn a_nested_window_at_the_far_edge_still_moves_nothing() {
        let mut tiling = nested();
        let before = every_rect(&tiling);
        tiling.drag_seam(5, Edge::Right, (900.0, 450.0), area(), settings());
        assert_eq!(every_rect(&tiling), before, "the inner seam was dragged");
    }

    /// The keyboard path, which took the same shortcut from the other end: it
    /// adjusted the immediate parent, and in this arrangement every leaf's
    /// parent is a horizontal split. `super+equal` meaning "wider" could not
    /// reach the centre vertical seam at all — it changed the height instead.
    #[test]
    fn the_keyboard_can_reach_the_centre_vertical_seam() {
        let mut tiling = quad();
        tiling.resize(1, Axis::Vertical, 0.1, area(), settings());

        let one = rect_of(&tiling, 1);
        assert!((one.w - 600.0).abs() < 2.0, "it got wider: {one:?}");
        assert!((one.h - 300.0).abs() < 1.0, "and not taller: {one:?}");
    }

    /// Growing is growing from either side. Window 2 has no seam on its right
    /// — it is against the container — so the seam on its left gives up the
    /// space, and the sign has to flip for the window to get bigger rather
    /// than smaller.
    #[test]
    fn a_window_against_the_container_grows_from_the_other_side() {
        let mut tiling = quad();
        tiling.resize(2, Axis::Vertical, 0.1, area(), settings());

        let two = rect_of(&tiling, 2);
        assert!((two.w - 600.0).abs() < 2.0, "it got wider: {two:?}");
        assert!(
            (two.x + two.w - 1000.0).abs() < 2.0,
            "from the left, because the right is the container's: {two:?}"
        );
    }

    /// The gap the compositor actually ships with.
    ///
    /// Every test above this point runs at `gap: 0.0`, and zero is the single
    /// value at which the seam band has no width — so "a share of the box" and
    /// "a share of the box less the gap" give the same answer, and the two
    /// sides of a seam are the same line. A suite written only at zero cannot
    /// tell whether a grabbed edge lands under the pointer or several pixels
    /// off it, which is the only thing #120 is about. Hence the cases below.
    fn gapped() -> Settings {
        Settings {
            gap: 12.0,
            split: 0.5,
            ..Settings::default()
        }
    }

    /// [`quad`] built at a given gap.
    ///
    /// The same arrangement, not a different one: `insert` picks each split's
    /// axis from the target box's aspect ratio, and insetting a 1000x600
    /// screen by 12 changes neither which side of the root box is longer nor
    /// which side of a column's is. The tree is identical; only the pixels
    /// move.
    fn quad_at(settings: Settings) -> Tiling {
        let mut tiling = Tiling::new();
        tiling.insert(1, None, None, area(), settings);
        tiling.insert(2, Some(1), None, area(), settings);
        tiling.insert(3, Some(1), None, area(), settings);
        tiling.insert(4, Some(2), None, area(), settings);
        tiling
    }

    fn rect_at(tiling: &Tiling, id: u64, settings: Settings) -> Rect {
        tiling
            .layout(area(), settings)
            .into_iter()
            .find(|(other, _)| *other == id)
            .expect("in the tree")
            .1
    }

    /// The whole of #120 as a coordinate: the edge the hand is on comes to rest
    /// exactly where it was asked to.
    ///
    /// Named for the ask and not for the pointer, because since #124 they are
    /// not the same place -- the compositor sends where the *edge* belongs.
    /// What #120 was about is unaffected: whatever the caller asks for, the
    /// grabbed edge has to land on it and not a gap away from it.
    ///
    /// Both halves drag the *same* seam — the root's vertical one — to the
    /// same x, one of them by window 1's right edge and one by window 2's
    /// left. Each has to put its own grabbed edge at exactly 700, and so the
    /// two drags have to agree with each other about where 700 is.
    ///
    /// Against the formula this replaced — a share of `rect.w`, with no term
    /// for the gap — the right edge came to rest at 691.5 and the left at
    /// 703.5. Each wrong, wrong in opposite directions, and a full gap apart
    /// from each other; all three assertions below fail on it. At `gap: 0.0`
    /// not one of them does, which is how the defect passed a green suite.
    #[test]
    fn a_dragged_vertical_edge_lands_where_it_was_asked_across_the_gap() {
        let settings = gapped();

        let mut trailing = quad_at(settings);
        trailing.drag_seam(1, Edge::Right, (700.0, 150.0), area(), settings);
        let one = rect_at(&trailing, 1, settings);
        assert!(
            (one.x + one.w - 700.0).abs() < 0.5,
            "the grabbed right edge is where it was asked: {one:?}"
        );

        let mut leading = quad_at(settings);
        leading.drag_seam(2, Edge::Left, (700.0, 150.0), area(), settings);
        let two = rect_at(&leading, 2, settings);
        assert!(
            (two.x - 700.0).abs() < 0.5,
            "the grabbed left edge is where it was asked: {two:?}"
        );

        // One seam, grabbed from either side, put in one place. This is the
        // assertion the old arithmetic missed by exactly `gap`.
        assert!(
            ((one.x + one.w) - two.x).abs() < 0.5,
            "the two sides of one seam disagree: {one:?} against {two:?}"
        );
    }

    /// The same, on the other axis, because the gap term is spelled once per
    /// axis and a fix to one of them is not a fix to the other. Window 1's
    /// bottom edge and window 3's top edge are the two sides of the left
    /// column's horizontal seam.
    #[test]
    fn a_dragged_horizontal_edge_lands_where_it_was_asked_across_the_gap() {
        let settings = gapped();

        let mut trailing = quad_at(settings);
        trailing.drag_seam(1, Edge::Bottom, (250.0, 400.0), area(), settings);
        let one = rect_at(&trailing, 1, settings);
        assert!(
            (one.y + one.h - 400.0).abs() < 0.5,
            "the grabbed bottom edge is where it was asked: {one:?}"
        );

        let mut leading = quad_at(settings);
        leading.drag_seam(3, Edge::Top, (250.0, 400.0), area(), settings);
        let three = rect_at(&leading, 3, settings);
        assert!(
            (three.y - 400.0).abs() < 0.5,
            "the grabbed top edge is where it was asked: {three:?}"
        );

        assert!(
            ((one.y + one.h) - three.y).abs() < 0.5,
            "the two sides of one seam disagree: {one:?} against {three:?}"
        );
    }

    /// #124: a window handed its own edge does not move.
    ///
    /// **Internal to this tree, and the #124 review is why that is now written
    /// down.** This was billed as the space check — the thing that pins the
    /// compositor's rectangle for a pane against the one [`Tiling::node_box`]
    /// measures a seam in. It is not, and structurally cannot be: the edge it
    /// hands `drag_seam` comes from `rect_at`, which is [`Tiling::layout`], so
    /// both ends of the round trip are this same tree. A skew introduced
    /// anywhere on the compositor's side of the call leaves it green, which is
    /// exactly what it did.
    ///
    /// What it *does* pin is worth keeping and is a narrower claim: within one
    /// arrangement, `node_box` and `layout` agree. Both begin at
    /// `area.inset(gap)` and both descend through [`cut`], so a slot's edge and
    /// a seam's box are two readings off one thing, and the inversion in
    /// [`Tiling::drag_seam`] undoes `cut` exactly rather than approximately. A
    /// half-gap error in either — #120's symptom by another route — shows here.
    ///
    /// The check this one was mistaken for lives in the compositor, because
    /// only the compositor has both rectangles:
    /// `solium::state::tests::real_client::a_client_that_rounds_its_size_does_not_move_the_seam`
    /// lays a real tree out through `Solium::place`, lets a real client commit a
    /// cell less than it was asked for, and feeds the edge the compositor would
    /// send on the first frame back into the tree. That is the boundary, and it
    /// cannot be crossed from this crate at all.
    ///
    /// At [`gapped`] rather than at zero, which is the first reason this can
    /// say anything: at `gap: 0` the two sides of a seam are one line and every
    /// skew it could catch is zero pixels wide.
    ///
    /// **And off-centre, which is the second, discovered by checking that the
    /// test can fail.** Skewing [`Tiling::node_box`]'s own inset by half a gap
    /// and re-running it — the exact defect it is here to catch — left it
    /// green. At ratio 0.5 that skew is self-cancelling: moving the box's
    /// origin out by 6 and its width out by 12 leaves its midpoint exactly
    /// where it was, so a seam sitting at the midpoint does not move and a
    /// fixture built at `split: 0.5` cannot tell. Each case therefore drags its
    /// seam somewhere deliberately lopsided first, and asserts that it went. On
    /// the same skew the strengthened version fails on its first case, by
    /// 1.87px — which is why the tolerance is 0.5px and not "a few pixels". A
    /// skew does not have to be as wide as the gap to be a skew.
    ///
    /// Unlike the compositor-side tests in
    /// `solium::input::resize::dragged_edge_tests`, this one passes against
    /// `bf80265` and against `ec1da24` too: `drag_seam` did not change for #124
    /// and has not changed since. It is a guard on this tree's own internal
    /// consistency, not a regression test for the value the compositor sends.
    #[test]
    fn a_window_handed_its_own_edge_does_not_move() {
        let settings = gapped();
        // One case per side, each on a window that actually has a seam there:
        // 1 has the root's vertical seam on its right and its column's
        // horizontal seam below, 2 has the root's on its left, 3 has its
        // column's above. A window flush against the container on the side
        // tested would pass this vacuously, `drag_seam` having returned early.
        //
        // The third number is where that seam goes first. Every one of them is
        // well inside the 0.05..0.95 clamp -- the extremes of these two seams
        // are 60..928 and 40..548 -- because a clamped ratio would hold still
        // for the second drag whatever space it was measured in, which is the
        // other way this test could pass without meaning anything.
        for (id, edge, lopsided) in [
            (1_u64, Edge::Right, (340.0, 150.0)),
            (2, Edge::Left, (640.0, 150.0)),
            (1, Edge::Bottom, (250.0, 200.0)),
            (3, Edge::Top, (250.0, 420.0)),
        ] {
            let mut tiling = quad_at(settings);
            let centred = rect_at(&tiling, id, settings);
            tiling.drag_seam(id, edge, lopsided, area(), settings);
            let before = rect_at(&tiling, id, settings);
            assert!(
                (before.w - centred.w).abs() > 50.0 || (before.h - centred.h).abs() > 50.0,
                "the fixture has to be off-centre before it proves anything: \
                 window {id} is still {before:?}"
            );

            let own_edge = match edge {
                Edge::Left => (before.x, before.y + before.h / 2.0),
                Edge::Right => (before.x + before.w, before.y + before.h / 2.0),
                Edge::Top => (before.x + before.w / 2.0, before.y),
                Edge::Bottom => (before.x + before.w / 2.0, before.y + before.h),
            };

            tiling.drag_seam(id, edge, own_edge, area(), settings);
            let after = rect_at(&tiling, id, settings);

            assert!(
                (after.x - before.x).abs() < 0.5
                    && (after.y - before.y).abs() < 0.5
                    && (after.w - before.w).abs() < 0.5
                    && (after.h - before.h).abs() < 0.5,
                "window {id} handed its own {edge:?} edge at {own_edge:?} moved: \
                 {before:?} became {after:?}"
            );
        }
    }

    /// Idempotence at a real gap.
    ///
    /// The doc on [`Tiling::drag_seam`] promises that dragging to the same
    /// place twice gives the same layout, and that promise is what stops the
    /// windows shaking while the button is held. It is worth pinning at a
    /// non-zero gap specifically.
    ///
    /// Unlike the two above, this one passed against the old formula too —
    /// being wrong by a constant offset is still being wrong in the same place
    /// every time. It is a guard and not a witness: what it catches is a
    /// future inversion that reads its own output back, the shape the
    /// delta-accumulating version had, and only a non-zero gap would make that
    /// term visible.
    #[test]
    fn dragging_to_the_same_place_twice_changes_nothing_at_a_real_gap() {
        let settings = gapped();
        let mut tiling = quad_at(settings);

        tiling.drag_seam(1, Edge::Right, (700.0, 150.0), area(), settings);
        let once = rect_at(&tiling, 1, settings);
        tiling.drag_seam(1, Edge::Right, (700.0, 150.0), area(), settings);
        let twice = rect_at(&tiling, 1, settings);

        assert!(
            (once.w - twice.w).abs() < 0.001,
            "the second drag moved it: {once:?} then {twice:?}"
        );
    }
}

/// #134: a floor under the tiles, and the seams that honour it.
///
/// Every arrangement here is built with `insert`, which does not read the
/// minimum, and with drags at [`free`] -- no minimum, the tree as it was
/// before #134 -- and only then is a question asked with a minimum in force.
/// A fixture built under the floor would already have been bent by it, and
/// the question would be about the fixture.
#[cfg(test)]
mod minimum_tests {
    use super::{Axis, Edge, Tiling};
    use crate::{Minimum, Rect, Settings};

    fn free(gap: f64) -> Settings {
        Settings {
            gap,
            split: 0.5,
            ..Settings::default()
        }
    }

    fn floored(gap: f64, w: f64, h: f64) -> Settings {
        Settings {
            minimum: Minimum { w, h },
            ..free(gap)
        }
    }

    /// What `config.tiling.minimum` ships as, and `config.gap`.
    fn shipped() -> Settings {
        floored(12.0, 160.0, 96.0)
    }

    fn rect_in(tiling: &Tiling, id: u64, area: Rect, settings: Settings) -> Rect {
        tiling
            .layout(area, settings)
            .into_iter()
            .find(|(other, _)| *other == id)
            .expect("in the tree")
            .1
    }

    fn every_rect(tiling: &Tiling, area: Rect, settings: Settings) -> Vec<(u64, Rect)> {
        let mut out = tiling.layout(area, settings);
        out.sort_by_key(|(id, _)| *id);
        out
    }

    fn near(left: Rect, right: Rect) -> bool {
        (left.x - right.x).abs() < 0.5
            && (left.y - right.y).abs() < 0.5
            && (left.w - right.w).abs() < 0.5
            && (left.h - right.h).abs() < 0.5
    }

    /// **The tile under the pointer tries the other direction first.** A wide,
    /// short tile with no room side by side still has room one above the
    /// other, and that is still the tile the user pointed at.
    ///
    /// 300x200 at no gap: side by side is two 150-wide halves, under a
    /// 160-wide floor; one above the other is two 300x100s, over a 96-high one.
    /// `insert` -- the rule before #134, and `"allow"` now -- cuts it side by
    /// side regardless, which is the contrast that makes this a test of the
    /// turn rather than of the arrangement.
    #[test]
    fn a_tile_with_no_room_side_by_side_splits_one_above_the_other() {
        let area = Rect::new(0.0, 0.0, 300.0, 200.0);
        let settings = floored(0.0, 160.0, 96.0);
        let mut tiling = Tiling::new();
        tiling.insert(1, None, None, area, settings);

        let mut allowed = tiling.clone();
        allowed.insert(2, None, Some((150.0, 150.0)), area, settings);
        assert!(
            (rect_in(&allowed, 2, area, settings).w - 150.0).abs() < 0.5,
            "the premise: `insert` cuts this tile side by side"
        );

        // The pointer in the lower half, so the new window goes below.
        assert!(tiling.insert_fitting(2, None, Some((150.0, 150.0)), area, settings));
        let (one, two) = (
            rect_in(&tiling, 1, area, settings),
            rect_in(&tiling, 2, area, settings),
        );
        assert!(
            (one.w - 300.0).abs() < 0.5 && (two.w - 300.0).abs() < 0.5,
            "split one above the other, full width each: {one:?} {two:?}"
        );
        assert!(
            (one.h - 100.0).abs() < 0.5 && (two.h - 100.0).abs() < 0.5,
            "and half the height each: {one:?} {two:?}"
        );
        assert!(two.y > one.y, "on the pointer's side, below: {two:?}");
    }

    /// **A tile with room neither way is left alone**, and so is the whole
    /// tree: `false` is only useful to a caller if trying somewhere else
    /// starts from where it was. Then `insert` still takes the window, once,
    /// which is what a node pushed and abandoned by the refusal would break --
    /// `contains` would find it, and `insert` would return without adding it.
    #[test]
    fn a_tile_with_no_room_either_way_is_left_alone() {
        let area = Rect::new(0.0, 0.0, 300.0, 180.0);
        let settings = floored(0.0, 160.0, 96.0);
        let mut tiling = Tiling::new();
        tiling.insert(1, None, None, area, settings);
        let before = every_rect(&tiling, area, settings);

        assert!(!tiling.insert_fitting(2, None, Some((150.0, 90.0)), area, settings));
        assert!(!tiling.insert_largest(2, area, settings));
        assert!(!tiling.contains(2), "a refused window is not in the tree");
        assert_eq!(every_rect(&tiling, area, settings), before);

        tiling.insert(2, None, Some((150.0, 90.0)), area, settings);
        assert_eq!(tiling.windows().len(), 2, "{:?}", tiling.windows());
    }

    /// **Exactly the minimum is enough; a pixel less is not.** 356 wide less
    /// a 12 gap on each side is a 332 tile, and 332 less the 12 between the
    /// halves is two of exactly 160. One pixel narrower and each half is
    /// 159.5, and turning it does not help: 200 high splits into two of 94,
    /// under 96.
    #[test]
    fn a_split_that_lands_exactly_on_the_minimum_has_room() {
        let settings = shipped();
        let exact = Rect::new(0.0, 0.0, 356.0, 224.0);
        let mut tiling = Tiling::new();
        tiling.insert(1, None, None, exact, settings);
        assert!(
            tiling.insert_fitting(2, None, Some((300.0, 100.0)), exact, settings),
            "two halves of exactly the minimum were refused"
        );
        for id in [1, 2] {
            let rect = rect_in(&tiling, id, exact, settings);
            assert!(
                (rect.w - 160.0).abs() < 1e-9,
                "window {id} is not at the minimum: {rect:?}"
            );
        }

        let short = Rect::new(0.0, 0.0, 355.0, 224.0);
        let mut tiling = Tiling::new();
        tiling.insert(1, None, None, short, settings);
        assert!(
            !tiling.insert_fitting(2, None, Some((300.0, 100.0)), short, settings),
            "halves of 159.5 were let under a 160 minimum"
        );
    }

    /// An empty tree has room whatever size it is: its one window is not a
    /// division of anything, so there is no tile to keep above the floor.
    #[test]
    fn an_empty_tree_always_has_room() {
        let tiny = Rect::new(0.0, 0.0, 100.0, 50.0);
        let mut fitting = Tiling::new();
        assert!(fitting.insert_fitting(1, None, None, tiny, shipped()));
        let mut largest = Tiling::new();
        assert!(largest.insert_largest(1, tiny, shipped()));
        assert_eq!(fitting.windows(), vec![1]);
        assert_eq!(largest.windows(), vec![1]);
    }

    /// Window 1 on the left, 599x700; windows 2 over 3 on the right, 601x401
    /// and 601x299. Built without a minimum, then asked about one of 300x360.
    ///
    /// Window 1 is the largest and has no room: across it is two of 299.5,
    /// under 300, and down it is two of 350, under 360. Window 3 is already
    /// shorter than 360 and no split makes it taller. Window 2 has room across,
    /// two of 300.5 by 401.
    fn crowded() -> (Tiling, Rect) {
        let area = Rect::new(0.0, 0.0, 1200.0, 700.0);
        let settings = free(0.0);
        let mut tiling = Tiling::new();
        tiling.insert(1, None, None, area, settings);
        tiling.insert(2, Some(1), None, area, settings);
        tiling.insert(3, Some(2), None, area, settings);
        tiling.drag_seam(1, Edge::Right, (599.0, 0.0), area, settings);
        tiling.drag_seam(2, Edge::Bottom, (0.0, 401.0), area, settings);
        (tiling, area)
    }

    /// **`"largest"` splits the largest tile that has room, which is not
    /// always the largest tile.** See [`crowded`].
    #[test]
    fn largest_splits_the_largest_tile_that_has_room() {
        let (mut tiling, area) = crowded();
        let settings = floored(0.0, 300.0, 360.0);
        let one = rect_in(&tiling, 1, area, settings);
        let three = rect_in(&tiling, 3, area, settings);
        assert!(
            (one.w - 599.0).abs() < 0.5 && (three.h - 299.0).abs() < 0.5,
            "the fixture is not the one described: {:?}",
            every_rect(&tiling, area, settings)
        );

        // Pointing into window 3, which has no room.
        assert!(!tiling.insert_fitting(4, None, Some((900.0, 600.0)), area, settings));
        assert!(tiling.insert_largest(4, area, settings));

        let (two, four) = (
            rect_in(&tiling, 2, area, settings),
            rect_in(&tiling, 4, area, settings),
        );
        assert!(
            (two.w - 300.5).abs() < 0.5 && (four.w - 300.5).abs() < 0.5,
            "window 2's tile was not the one split: {:?}",
            every_rect(&tiling, area, settings)
        );
        assert!(four.x > two.x, "the new window takes the far side");
        assert!(near(rect_in(&tiling, 1, area, settings), one), "1 moved");
        assert!(near(rect_in(&tiling, 3, area, settings), three), "3 moved");
    }

    /// Two columns of two at a real gap, with no minimum, exactly as
    /// `dragged_edge_tests::quad_at` builds them: 1 top-left, 2 top-right, 3
    /// bottom-left, 4 bottom-right, each 482x282 in a 1000x600 area.
    fn quad(settings: Settings) -> Tiling {
        let area = area();
        let mut tiling = Tiling::new();
        tiling.insert(1, None, None, area, settings);
        tiling.insert(2, Some(1), None, area, settings);
        tiling.insert(3, Some(1), None, area, settings);
        tiling.insert(4, Some(2), None, area, settings);
        tiling
    }

    fn area() -> Rect {
        Rect::new(0.0, 0.0, 1000.0, 600.0)
    }

    /// **No drag takes a tile under the minimum**, on any of the four sides
    /// that have a seam. The box is 976 wide less a 12 gap, so a 300 floor on
    /// either side of the root seam leaves 364 of travel, and each is dragged
    /// well past it.
    #[test]
    fn a_seam_cannot_be_dragged_to_make_a_tile_smaller_than_the_minimum() {
        let settings = floored(12.0, 300.0, 150.0);
        for (id, edge, to, measured, wanted) in [
            (1_u64, Edge::Right, (100.0, 150.0), 1_u64, 300.0),
            (2, Edge::Left, (950.0, 150.0), 2, 300.0),
            (1, Edge::Bottom, (250.0, 50.0), 1, 150.0),
            (3, Edge::Top, (250.0, 580.0), 3, 150.0),
        ] {
            let mut tiling = quad(free(12.0));
            tiling.drag_seam(id, edge, to, area(), settings);
            let rect = rect_in(&tiling, measured, area(), settings);
            let size = match edge.axis() {
                Axis::Vertical => rect.w,
                Axis::Horizontal => rect.h,
            };
            assert!(
                (size - wanted).abs() < 0.5,
                "dragging window {id}'s {edge:?} edge to {to:?} left window {measured} at \
                 {rect:?}, not stopped at the {wanted} minimum"
            );
        }
    }

    /// **The floor counts every tile beyond the seam, not only the one beside
    /// it.** Window 4 is split again, so the right column holds 2 above a pair
    /// side by side, and the root seam -- window 4's left edge -- has to leave
    /// room for both halves of that pair: 160, the gap, 160.
    #[test]
    fn a_seam_leaves_room_for_every_tile_beyond_it() {
        let settings = shipped();
        let mut tiling = quad(free(12.0));
        tiling.insert(5, Some(4), None, area(), free(12.0));
        tiling.drag_seam(4, Edge::Left, (900.0, 450.0), area(), settings);
        let (four, five) = (
            rect_in(&tiling, 4, area(), settings),
            rect_in(&tiling, 5, area(), settings),
        );
        assert!(
            (four.w - 160.0).abs() < 0.5 && (five.w - 160.0).abs() < 0.5,
            "the pair beyond the seam was squeezed under the minimum: {four:?} {five:?}"
        );
        assert!(
            (five.x + five.w - 988.0).abs() < 0.5,
            "and the far edge stayed against the container: {five:?}"
        );
    }

    /// **A drag on a seam that is already under the minimum does not jump.**
    /// The #124 class of bug, through a floor instead of a skew: clamping a
    /// ratio that is outside its range moves the seam on the first frame of
    /// the drag, with the pointer still where it pressed.
    ///
    /// Window 1 is dragged to 328 wide with no minimum, then a 600 minimum is
    /// put on it. Handed its own edge, nothing moves. Asked to shrink further,
    /// nothing moves. Asked to grow, it follows -- until window 2, at 636 and
    /// so over the floor, reaches 600 on the other side of the seam.
    #[test]
    fn a_drag_on_a_seam_already_under_the_minimum_does_not_jump() {
        let settings = floored(12.0, 600.0, 96.0);
        let lopsided = || {
            let mut tiling = quad(free(12.0));
            tiling.drag_seam(1, Edge::Right, (340.0, 150.0), area(), free(12.0));
            tiling
        };
        let before = every_rect(&lopsided(), area(), settings);
        let one = rect_in(&lopsided(), 1, area(), settings);
        assert!(
            (one.x + one.w - 340.0).abs() < 0.5,
            "the premise: window 1's right edge at 340: {one:?}"
        );

        for (to, says) in [
            (340.0, "handed its own edge"),
            (300.0, "asked to shrink a tile already under the minimum"),
        ] {
            let mut tiling = lopsided();
            tiling.drag_seam(1, Edge::Right, (to, 150.0), area(), settings);
            assert_eq!(
                every_rect(&tiling, area(), settings),
                before,
                "{says}, the seam moved"
            );
        }

        let mut tiling = lopsided();
        tiling.drag_seam(1, Edge::Right, (350.0, 150.0), area(), settings);
        let one = rect_in(&tiling, 1, area(), settings);
        assert!(
            (one.x + one.w - 350.0).abs() < 0.5,
            "growing the small side was refused: {one:?}"
        );

        tiling.drag_seam(1, Edge::Right, (900.0, 150.0), area(), settings);
        let two = rect_in(&tiling, 2, area(), settings);
        assert!(
            (two.w - 600.0).abs() < 0.5,
            "the far side went under its own minimum: {two:?}"
        );
    }

    /// With both sides of a seam under the minimum, any move shrinks one of
    /// them, so the seam holds still. The quad's columns are 482 each, both
    /// under 600.
    #[test]
    fn a_seam_with_both_sides_under_the_minimum_holds_still() {
        let settings = floored(12.0, 600.0, 96.0);
        for to in [100.0, 494.0, 900.0] {
            let mut tiling = quad(free(12.0));
            let before = every_rect(&tiling, area(), settings);
            tiling.drag_seam(1, Edge::Right, (to, 150.0), area(), settings);
            assert_eq!(
                every_rect(&tiling, area(), settings),
                before,
                "dragged to {to}, a seam with no room on either side moved"
            );
        }
    }

    /// **The shipped minimum changes none of the #120 and #124 drags.** Each
    /// is the drag from `dragged_edge_tests` -- the four lopsided moves of
    /// `a_window_handed_its_own_edge_does_not_move`, then the window handed
    /// its own edge, and the two-sided 700 of the #120 case -- run once with
    /// no minimum and once at 160x96, and the two have to agree to the pixel.
    /// Every tile involved stays over 160x96, so a floor that moved any of
    /// them would be a floor clamping where it has no business to.
    #[test]
    fn the_shipped_minimum_leaves_every_existing_drag_where_it_was() {
        let cases: [(u64, Edge, (f64, f64)); 6] = [
            (1, Edge::Right, (340.0, 150.0)),
            (2, Edge::Left, (640.0, 150.0)),
            (1, Edge::Bottom, (250.0, 200.0)),
            (3, Edge::Top, (250.0, 420.0)),
            (1, Edge::Right, (700.0, 150.0)),
            (2, Edge::Left, (700.0, 150.0)),
        ];
        for (id, edge, to) in cases {
            let run = |settings: Settings| {
                let mut tiling = quad(settings);
                tiling.drag_seam(id, edge, to, area(), settings);
                let placed = rect_in(&tiling, id, area(), settings);
                let own = match edge {
                    Edge::Left => (placed.x, placed.y + placed.h / 2.0),
                    Edge::Right => (placed.x + placed.w, placed.y + placed.h / 2.0),
                    Edge::Top => (placed.x + placed.w / 2.0, placed.y),
                    Edge::Bottom => (placed.x + placed.w / 2.0, placed.y + placed.h),
                };
                tiling.drag_seam(id, edge, own, area(), settings);
                every_rect(&tiling, area(), settings)
            };
            let (without, with) = (run(free(12.0)), run(shipped()));
            assert!(
                without
                    .iter()
                    .zip(&with)
                    .all(|((a, left), (b, right))| a == b && near(*left, *right)),
                "window {id}'s {edge:?} drag to {to:?} came out differently under the \
                 shipped minimum: {without:?} against {with:?}"
            );
        }
    }

    /// **The keyboard is held to the same floor**, and to the same rule for a
    /// tile already under it. At a 400 minimum window 1 shrinks to 400 and no
    /// further. Then, lopsided to 328 and put under a 600 minimum, a press
    /// that would shrink it does nothing and one that grows it stops where
    /// window 2 reaches 600.
    #[test]
    fn the_keyboard_does_not_shrink_a_tile_under_the_minimum() {
        let settings = floored(12.0, 400.0, 96.0);
        let mut tiling = quad(free(12.0));
        for _ in 0..5 {
            tiling.resize(1, Axis::Vertical, -0.05, area(), settings);
        }
        let one = rect_in(&tiling, 1, area(), settings);
        assert!((one.w - 400.0).abs() < 0.5, "shrunk past 400: {one:?}");

        let settings = floored(12.0, 600.0, 96.0);
        let mut tiling = quad(free(12.0));
        tiling.drag_seam(1, Edge::Right, (340.0, 150.0), area(), free(12.0));
        let before = every_rect(&tiling, area(), settings);
        tiling.resize(1, Axis::Vertical, -0.05, area(), settings);
        assert_eq!(
            every_rect(&tiling, area(), settings),
            before,
            "a press shrank a tile already under the minimum"
        );
        tiling.resize(1, Axis::Vertical, 0.05, area(), settings);
        let two = rect_in(&tiling, 2, area(), settings);
        assert!(
            (two.w - 600.0).abs() < 0.5,
            "growing window 1 took window 2 under the minimum, or stopped short: {two:?}"
        );
    }

    /// A split of NaN -- `split = 0/0` in a configuration -- is a seam whose
    /// ratio is NaN, and [`Tiling::room`] then cannot widen its range to take
    /// that ratio in. With a minimum no ratio can satisfy -- 600 on each side
    /// of a 964 span -- the range is empty, which is exactly where `f64::clamp`
    /// panics, and a panic here is the session gone. The press and the drag
    /// below have to come back; that is the assertion, with every rect finite
    /// besides.
    #[test]
    fn a_nan_split_leaves_the_clamp_standing() {
        let broken = Settings {
            split: f64::NAN,
            ..floored(12.0, 600.0, 96.0)
        };
        let mut tiling = Tiling::new();
        tiling.insert(1, None, None, area(), broken);
        tiling.insert(2, Some(1), None, area(), broken);
        tiling.resize(1, Axis::Vertical, 0.05, area(), broken);
        tiling.drag_seam(1, Edge::Right, (500.0, 300.0), area(), broken);
        for (id, rect) in tiling.layout(area(), broken) {
            assert!(
                rect.w.is_finite() && rect.h.is_finite(),
                "window {id} was laid out at {rect:?}"
            );
        }
    }
}
