//! The trees of boxes a window manager arranges (`arrange`), kept solved:
//! each window placed in its box, the tree solved again when an
//! application's least or largest size changes, and a seam moved by the
//! compositor itself, frame by frame, when a window's edge is dragged.
//!
//! The arithmetic is `solium_wm_protocol::space`'s; this is where its
//! answers meet the windows. A tree names no arrangement: a window manager
//! tells a dwindle layout, a row of columns or anything else as boxes.

use std::collections::BTreeMap;

use solium_wm_protocol::space::{Limits as BoxLimits, Side, Solved, Tree};

use super::*;

/// One tree, and what it was last solved with.
#[derive(Debug)]
struct Kept {
    tree: Tree,
    /// How its windows move when it is solved again on its own.
    animation: AnimationSpec,
    /// Each window's limits as the last solve read them, in the tree's
    /// order: a change in any is what solves it again.
    read: Vec<(u64, BoxLimits)>,
}

/// Every tree the window manager has arranged, by its key.
#[derive(Debug, Default)]
pub(crate) struct Trees {
    kept: BTreeMap<String, Kept>,
    /// The seams an edge drag or a `grow` has moved and the window manager
    /// has not been told of: the tree's key and the split's place in it.
    moved: Vec<(String, usize)>,
    /// Whether an edge drag has moved a seam, and whether its button has
    /// come up: its seams are told once it has and its last motion has
    /// been applied, which may be a frame after the release.
    dragging: bool,
    released: bool,
}

impl Trees {
    /// The key of the tree `window` is in.
    fn holding(&self, window: u64) -> Option<String> {
        self.kept
            .iter()
            .find(|(_, kept)| kept.tree.contains(window))
            .map(|(key, _)| key.clone())
    }

    /// An edge drag let go of: what it moved is told once its last motion
    /// is in.
    pub(crate) fn released(&mut self) {
        if self.dragging {
            self.released = true;
        }
    }

    fn note(&mut self, key: &str, split: usize) {
        if !self
            .moved
            .iter()
            .any(|(held, at)| held == key && *at == split)
        {
            self.moved.push((key.to_owned(), split));
        }
    }
}

/// An edge drag lasts no time at all: the window follows the pointer.
const AT_ONCE: AnimationSpec = AnimationSpec {
    duration: Duration::ZERO,
    easing: solium_animation::Curve::Linear,
};

fn box_size(size: Option<Size<i32, Logical>>) -> Option<solium_wm_protocol::Size> {
    size.map(|size| solium_wm_protocol::Size {
        w: size.w,
        h: size.h,
    })
}

impl Solium {
    /// What `window`'s application says of its size, frame included: the
    /// same numbers its row in the window list has, so a window manager
    /// solving the tree itself comes to the same answer.
    fn box_limits(&self, window: u64) -> BoxLimits {
        let Some(pane) = self.panes.by_script_id(window) else {
            return BoxLimits::default();
        };
        let Some(client) = pane.client() else {
            return BoxLimits::default();
        };
        let limits = limits_of(client);
        let insets = self.insets_of(pane.id());
        BoxLimits {
            min: box_size(snapshot::in_pane(limits.min, insets)),
            max: box_size(snapshot::in_pane(limits.max, insets)),
        }
    }

    /// Hold a tree under `key` and place its windows, or take the tree
    /// away. A window in it leaves any other tree. Whether a window's
    /// `cramped` changed.
    pub(super) fn arrange(
        &mut self,
        key: String,
        tree: Option<Tree>,
        animation: AnimationSpec,
        moved: Option<u64>,
        now: Duration,
    ) -> bool {
        let Some(tree) = tree else {
            self.trees.kept.remove(&key);
            return false;
        };
        for window in tree.windows() {
            for (other, kept) in &mut self.trees.kept {
                if *other != key {
                    kept.tree.let_go(window);
                }
            }
        }
        self.trees.kept.insert(
            key.clone(),
            Kept {
                tree,
                animation,
                read: Vec::new(),
            },
        );
        self.place_tree(&key, animation, moved, now)
    }

    /// A window placed or let go of by the window manager itself is in no
    /// tree from now on: its box holds nothing, and nothing else moves.
    pub(super) fn out_of_trees(&mut self, window: u64) {
        for kept in self.trees.kept.values_mut() {
            kept.tree.let_go(window);
        }
    }

    /// Solve the tree under `key` and put each of its windows where it
    /// comes out. Whether a window's `cramped` changed.
    ///
    /// A fullscreen or maximised window a key is moving (`moved`) goes to the
    /// monitor its box is on and stays so there, as `Command::Place::moved`
    /// has it; every other one keeps where it is (`Solium::move_pane`).
    fn place_tree(
        &mut self,
        key: &str,
        animation: AnimationSpec,
        moved: Option<u64>,
        now: Duration,
    ) -> bool {
        let Some(kept) = self.trees.kept.get(key) else {
            return false;
        };
        let read = self.tree_limits(key);
        let layout = kept.tree.solve(|window| looked_up(&read, window));
        if let Some(kept) = self.trees.kept.get_mut(key) {
            kept.read = read;
        }
        let mut cramped_changed = false;
        for solved in layout.windows {
            cramped_changed |= self.place_solved(&solved, animation, moved, now);
        }
        cramped_changed
    }

    /// One window where its tree put it: held in its box, and `cramped`
    /// when the box is smaller than its application's least size.
    fn place_solved(
        &mut self,
        solved: &Solved,
        animation: AnimationSpec,
        moved: Option<u64>,
        now: Duration,
    ) -> bool {
        let rect = Rect {
            x: solved.rect.x,
            y: solved.rect.y,
            w: solved.rect.w,
            h: solved.rect.h,
        };
        let standing = if solved.rect == solved.tile {
            Standing::Tile
        } else {
            Standing::Within(outer_of(Rect {
                x: solved.tile.x,
                y: solved.tile.y,
                w: solved.tile.w,
                h: solved.tile.h,
            }))
        };
        if moved == Some(solved.window) {
            self.carry_across(solved.window, rect, standing);
        }
        self.place(solved.window, rect, animation, now, standing);
        let Some(pane) = self.panes.by_script_id(solved.window).map(Pane::id) else {
            return false;
        };
        let Some(pane) = self.panes.get_mut(pane) else {
            return false;
        };
        let changed = pane.cramped() != solved.cramped;
        pane.set_cramped(solved.cramped);
        changed
    }

    /// Grow `window` in its tree by moving a seam, and place the windows it
    /// moves. Whether a window's `cramped` changed.
    pub(super) fn grow(
        &mut self,
        window: u64,
        axis: solium_wm_protocol::Axis,
        by: f64,
        animation: AnimationSpec,
        now: Duration,
    ) -> bool {
        let Some(key) = self.trees.holding(window) else {
            return false;
        };
        let limits = self.tree_limits(&key);
        let lookup = |id: u64| looked_up(&limits, id);
        let Some(seam) = self
            .trees
            .kept
            .get_mut(&key)
            .and_then(|kept| kept.tree.grow(window, axis, by, lookup))
        else {
            return false;
        };
        let changed = self.place_tree(&key, animation, None, now);
        self.trees.note(&key, seam.split);
        changed
    }

    /// An edge drag on a window in a tree with a seam on the dragged side:
    /// the seam follows the edge, and every window it moves is placed now,
    /// with no question to the window manager. None when the window is in
    /// no tree, or no seam lies along the side dragged: then the window
    /// manager is asked, as for any window.
    pub(super) fn drag_seams(&mut self, request: &ResizeRequest) -> Option<bool> {
        let window = self.window_id(&request.window);
        let key = self.trees.holding(window)?;
        let (horizontal, vertical) = crate::input::resize::sides(request.edges);
        let limits = self.tree_limits(&key);
        let lookup = |id: u64| looked_up(&limits, id);
        let mut dragged = Vec::new();
        for (side, edge) in [
            (horizontal, request.edge_at.0),
            (vertical, request.edge_at.1),
        ] {
            let Some(side) = side.and_then(Side::named) else {
                continue;
            };
            let kept = self.trees.kept.get_mut(&key)?;
            let Some(seam) = kept.tree.seam_beside(window, side) else {
                continue;
            };
            kept.tree.drag(seam, edge, lookup);
            dragged.push(seam.split);
        }
        if dragged.is_empty() {
            return None;
        }
        let now = self.clock.now();
        let cramped_changed = self.place_tree(&key, AT_ONCE, None, now);
        for split in dragged {
            self.trees.note(&key, split);
        }
        self.trees.dragging = true;
        if cramped_changed {
            self.retell_cramped();
        }
        Some(true)
    }

    /// Tell the window manager where each moved seam was left, once the
    /// drag that moved it is over: its next `arrange` carries the shares,
    /// so nothing moves when it comes.
    pub(super) fn tell_seams(&mut self) {
        let moved = std::mem::take(&mut self.trees.moved);
        for (key, split) in moved {
            let Some((id, shares)) = self
                .trees
                .kept
                .get(&key)
                .and_then(|kept| kept.tree.shares(split))
            else {
                continue;
            };
            let snapshot = self.snapshot();
            let Some(mut scripts) = self.scripts.take() else {
                return;
            };
            let outcome = scripts.seam_moved(key, id, shares, snapshot);
            self.scripts = Some(scripts);
            self.apply(outcome);
        }
    }

    /// Each window's limits, in the order of the tree under `key`.
    fn tree_limits(&self, key: &str) -> Vec<(u64, BoxLimits)> {
        self.trees.kept.get(key).map_or_else(Vec::new, |kept| {
            kept.tree
                .windows()
                .map(|window| (window, self.box_limits(window)))
                .collect()
        })
    }

    /// The layouts run once more for a window whose `cramped` changed, with a
    /// snapshot that has it, and once only: see the end of `Self::apply`.
    fn retell_cramped(&mut self) {
        if !self.retelling_cramped {
            self.retelling_cramped = true;
            self.trigger_relayout();
            self.retelling_cramped = false;
        }
    }

    /// Once a frame: solve again each tree a window of which says another
    /// least or largest size than it was solved with, moving its windows by
    /// the tree's own motion; and tell the seams a drag or a `grow` moved,
    /// once no drag is under way. Costs a comparison of each tree's
    /// windows' limits when nothing changed. Whether anything moved.
    pub(crate) fn settle_trees(&mut self) -> bool {
        let drag_over =
            !self.trees.dragging || (self.trees.released && self.pending_resize.is_none());
        if !self.trees.moved.is_empty() && drag_over {
            self.trees.dragging = false;
            self.trees.released = false;
            self.tell_seams();
        }
        let stale: Vec<(String, AnimationSpec)> = self
            .trees
            .kept
            .iter()
            .filter(|(_, kept)| {
                let now: Vec<(u64, BoxLimits)> = kept
                    .tree
                    .windows()
                    .map(|window| (window, self.box_limits(window)))
                    .collect();
                now != kept.read
            })
            .map(|(key, kept)| (key.clone(), kept.animation))
            .collect();
        if stale.is_empty() {
            return false;
        }
        let now = self.clock.now();
        let mut cramped_changed = false;
        for (key, animation) in &stale {
            cramped_changed |= self.place_tree(key, *animation, None, now);
        }
        if cramped_changed {
            self.retell_cramped();
        }
        self.redraw = true;
        true
    }
}

/// `id`'s limits out of what was read for a tree.
fn looked_up(limits: &[(u64, BoxLimits)], id: u64) -> BoxLimits {
    limits
        .iter()
        .find(|(each, _)| *each == id)
        .map(|(_, limits)| limits.clone())
        .unwrap_or_default()
}
