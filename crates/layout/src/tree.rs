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
        let target = target
            .and_then(|id| self.leaf(id))
            .or_else(|| self.leaf_at(at, &boxes))
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

    /// Move one seam. Everything on the far side of the tree stays put.
    pub fn resize(&mut self, id: u64, by: f64) {
        let Some(leaf) = self.leaf(id) else {
            return;
        };
        let Some(parent) = self.parent(leaf) else {
            return;
        };
        if let Some(Node::Split {
            ratio, children, ..
        }) = self.nodes[parent].as_mut()
        {
            let towards = if children[0] == leaf { by } else { -by };
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

        tiling.resize(2, 0.2);
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
