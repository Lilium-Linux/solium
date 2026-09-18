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

use crate::{Rect, Settings};

/// Which way a branch was cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Children sit side by side.
    Vertical,
    /// Children sit one above the other.
    Horizontal,
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
        let fresh = self.push(Node::Window { id });

        let Some(root) = self.root else {
            self.root = Some(fresh);
            return;
        };

        // The target's *current* box decides the axis, so the split follows the
        // shape of the space being divided rather than the shape of the screen.
        let boxes = self.layout(area, settings);
        // Named target, else the window under the pointer, else the window
        // *nearest* it. The last step matters more than it looks: falling back
        // to the root instead means splitting the whole screen, and every new
        // window then lands as another full-height column — which is not
        // dwindle at all, and is exactly what this did before. Hyprland calls
        // this `getClosestNode`.
        // A window cannot be split by itself. The caller can easily name it —
        // a new window is mapped and under the pointer before this runs, so
        // hit-testing the cursor answers with the very window being inserted —
        // and `leaf` would then find the node pushed a moment ago. The branch
        // would take that node as both children and hang from nothing, and the
        // window would vanish from the tree without any error at all.
        //
        // Hyprland guards the same case in `addTarget`, calling it a fail-safe
        // and picking a different node. This is that guard.
        let target = target
            .filter(|named| *named != id)
            .and_then(|id| self.leaf(id))
            .filter(|found| *found != fresh)
            .or_else(|| self.leaf_at(at, &boxes))
            .or_else(|| self.closest_leaf(at, &boxes))
            .unwrap_or(root);

        let box_of = self
            .id_of(target)
            .and_then(|id| boxes.iter().find(|(other, _)| *other == id))
            .map_or(area, |(_, rect)| *rect);

        let axis = if box_of.w >= box_of.h {
            Axis::Vertical
        } else {
            Axis::Horizontal
        };

        // Which side the new window takes: the half the pointer is in, and the
        // far side by default.
        let second = at.is_none_or(|(x, y)| match axis {
            Axis::Vertical => x >= box_of.x + box_of.w / 2.0,
            Axis::Horizontal => y >= box_of.y + box_of.h / 2.0,
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

    /// Drag the seam beside a window's `edge` to where the pointer is.
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
    /// And it is idempotent. The ratio comes from where the pointer *is*, not
    /// from how far it moved, so dragging to the same place twice gives the
    /// same layout. Accumulating deltas fed the layout's own response back in
    /// as the next input, and the windows shook themselves apart for as long
    /// as the button was held.
    pub fn drag_seam(
        &mut self,
        id: u64,
        edge: Edge,
        at: (f64, f64),
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
        let ratio = match edge.axis() {
            Axis::Vertical => (at.0 - rect.x) / rect.w.max(1.0),
            Axis::Horizontal => (at.1 - rect.y) / rect.h.max(1.0),
        };
        if let Some(Node::Split { ratio: current, .. }) = self.nodes[seam].as_mut() {
            *current = ratio.clamp(0.05, 0.95);
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
    pub fn resize(&mut self, id: u64, axis: Axis, by: f64) {
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
        if let Some(Node::Split { ratio, .. }) = self.nodes[seam].as_mut() {
            *ratio = (*ratio + towards).clamp(0.05, 0.95);
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

        tiling.resize(2, Axis::Horizontal, 0.2);
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
            "seam followed the pointer: {after}"
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
        assert!((one.h - 400.0).abs() < 2.0, "followed the pointer: {one:?}");
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
        tiling.resize(1, Axis::Vertical, 0.1);

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
        tiling.resize(2, Axis::Vertical, 0.1);

        let two = rect_of(&tiling, 2);
        assert!((two.w - 600.0).abs() < 2.0, "it got wider: {two:?}");
        assert!(
            (two.x + two.w - 1000.0).abs() < 2.0,
            "from the left, because the right is the container's: {two:?}"
        );
    }
}
