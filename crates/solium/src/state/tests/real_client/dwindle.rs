//! A dwindle-shaped tree of boxes, held from Rust for the tests that lay
//! windows out through `Solium::place` and drag their seams: each split a
//! two-way split as `layouts/dwindle.lua` hands one to `sol.arrange`, solved
//! and dragged by `solium_wm_protocol::space`, the compositor's own
//! arithmetic for a tree it was handed.

use solium_wm_protocol::space::{Limits, Side, Tree};
use solium_wm_protocol::{Axis, SpaceBox, SpaceKind, Split};

/// A rectangle as the tree hands it back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Rect {
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) w: f64,
    pub(super) h: f64,
}

impl Rect {
    pub(super) fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }
}

/// What the tree is laid out with besides the area.
#[derive(Clone, Copy, Debug)]
pub(super) struct Settings {
    pub(super) gap: f64,
    pub(super) split: f64,
}

/// A window's side, as the `resize` event names it.
#[derive(Clone, Copy, Debug)]
pub(super) enum Edge {
    Right,
}

#[derive(Clone, Copy, Debug)]
enum Node {
    Window(u64),
    Split {
        axis: Axis,
        ratio: f64,
        first: usize,
        second: usize,
    },
}

/// A tree: its nodes, by number, and its root.
#[derive(Clone, Debug, Default)]
pub(super) struct Tiling {
    nodes: Vec<Node>,
    root: Option<usize>,
}

fn no_limits(_: u64) -> Limits {
    Limits::default()
}

impl Tiling {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// The tree as boxes, each box named by its node's number.
    fn boxes(&self, settings: Settings) -> Vec<SpaceBox> {
        let mut out = Vec::new();
        let mut ahead = self
            .root
            .map(|root| vec![(root, None, 1.0)])
            .unwrap_or_default();
        while let Some((at, parent, share)) = ahead.pop() {
            let here = u32::try_from(out.len()).expect("a small tree");
            match self.nodes[at] {
                Node::Window(id) => out.push(SpaceBox {
                    parent,
                    id: at as u64,
                    kind: SpaceKind::Window(id),
                    share,
                    client_min: true,
                    ..SpaceBox::default()
                }),
                Node::Split {
                    axis,
                    ratio,
                    first,
                    second,
                } => {
                    out.push(SpaceBox {
                        parent,
                        id: at as u64,
                        kind: SpaceKind::Split(Split {
                            axis,
                            gap: settings.gap,
                            resizable: true,
                        }),
                        share,
                        ..SpaceBox::default()
                    });
                    // Popped first-side first, so a parent's children come
                    // in order.
                    ahead.push((second, Some(here), 1.0 - ratio));
                    ahead.push((first, Some(here), ratio));
                }
            }
        }
        out
    }

    /// The tree to solve: the area with the gap off every side, as
    /// `dwindle.lua` hands it over.
    fn tree(&self, area: Rect, settings: Settings) -> Option<Tree> {
        let boxes = self.boxes(settings);
        if boxes.is_empty() {
            return None;
        }
        let inset = solium_wm_protocol::Rect {
            x: area.x + settings.gap,
            y: area.y + settings.gap,
            w: (area.w - settings.gap * 2.0).max(1.0),
            h: (area.h - settings.gap * 2.0).max(1.0),
        };
        Some(Tree::of(&inset, &boxes).expect("a tree"))
    }

    fn leaf(&self, id: u64) -> Option<usize> {
        self.nodes
            .iter()
            .position(|node| matches!(node, Node::Window(each) if *each == id))
    }

    /// Add window `id` beside `target`, or beside the first window, across
    /// the longer side of its tile, on the far side of the pointer `at`, or
    /// on the far side when there is none: `Tree:insert` in `dwindle.lua`.
    pub(super) fn insert(
        &mut self,
        id: u64,
        target: Option<u64>,
        at: Option<(f64, f64)>,
        area: Rect,
        settings: Settings,
    ) {
        if self.leaf(id).is_some() {
            return;
        }
        let Some(root) = self.root else {
            self.nodes.push(Node::Window(id));
            self.root = Some(self.nodes.len() - 1);
            return;
        };
        let tiles = self.layout(area, settings);
        let target = target
            .filter(|target| *target != id)
            .and_then(|target| self.leaf(target))
            .or_else(|| tiles.first().and_then(|(first, _)| self.leaf(*first)))
            .unwrap_or(root);
        let tile = match self.nodes[target] {
            Node::Window(window) => tiles
                .iter()
                .find(|(each, _)| *each == window)
                .map_or(area, |(_, tile)| *tile),
            Node::Split { .. } => area,
        };
        let axis = if tile.w >= tile.h { Axis::X } else { Axis::Y };
        let goes_second = at.is_none_or(|(x, y)| match axis {
            Axis::X => x >= tile.x + tile.w / 2.0,
            Axis::Y => y >= tile.y + tile.h / 2.0,
        });
        self.nodes.push(Node::Window(id));
        let fresh = self.nodes.len() - 1;
        let moved = self.nodes.len();
        self.nodes.push(self.nodes[target]);
        let (first, second) = if goes_second {
            (moved, fresh)
        } else {
            (fresh, moved)
        };
        self.nodes[target] = Node::Split {
            axis,
            ratio: settings.split.clamp(0.05, 0.95),
            first,
            second,
        };
    }

    /// What the compositor does with window `id`'s `edge` dragged to `at`:
    /// the seam beside it follows, and the tree keeps where it was left.
    pub(super) fn drag_seam(
        &mut self,
        id: u64,
        edge: Edge,
        at: (f64, f64),
        area: Rect,
        settings: Settings,
    ) {
        let mut tree = self.tree(area, settings).expect("a tree to drag in");
        let side = match edge {
            Edge::Right => Side::Right,
        };
        let seam = tree
            .seam_beside(id, side)
            .unwrap_or_else(|| panic!("{edge:?} of window {id} has a seam"));
        tree.drag(seam, at.0, no_limits);
        let (node, shares) = tree.shares(seam.split).expect("a split");
        let node = usize::try_from(node).expect("a node's number");
        if let Node::Split { ratio, .. } = &mut self.nodes[node] {
            *ratio = shares[0];
        }
    }

    /// Where every window goes, in tree order.
    pub(super) fn layout(&self, area: Rect, settings: Settings) -> Vec<(u64, Rect)> {
        let Some(tree) = self.tree(area, settings) else {
            return Vec::new();
        };
        tree.solve(no_limits)
            .windows
            .into_iter()
            .map(|solved| {
                (
                    solved.window,
                    Rect::new(solved.tile.x, solved.tile.y, solved.tile.w, solved.tile.h),
                )
            })
            .collect()
    }
}
