//! **A tree of boxes, solved**: the arithmetic behind the `arrange` command.
//!
//! A window manager says how room is shared (splits, their shares and gaps,
//! boxes placed where it likes) and which window goes in which box; this
//! turns that into rectangles, with every window's application's own least
//! and largest size taken into account, and moves a split's seam for a
//! dragged edge. The compositor runs it to place the windows and to drag
//! seams frame by frame; a window manager in Rust may run the same code to
//! know the answer before it lands.
//!
//! It knows no arrangement. A tree of two-way splits is how a dwindle
//! layout is told, a split along `x` of splits along `y` is a row of
//! columns, and either is only boxes here.
//!
//! **Sharing.** A split's children take their shares of its room, once the
//! gaps are taken out, each at least a twentieth of it (or an equal part,
//! with more than twenty), and never less than a pixel. A child too small
//! for a window's least size takes the shortfall from its siblings that have
//! room above their own least, and no more than they have; a child's own
//! share is never rewritten by that, so a window whose least size goes away
//! gives the room back. With every child short, or none, the shares stand.
//!
//! For two children this is the arithmetic `layouts/dwindle.lua` did in Lua
//! before it moved here, number for number.

use std::collections::HashSet;

use crate::{Arrange, Axis, Rect, Size, SpaceBox, SpaceKind};

/// How far under a size a box may come out and still count as at it: a
/// split's children have been through a division or two, and a box laid out
/// at exactly a least size can land a rounding error short of it.
const SLACK: f64 = 1e-6;

/// The least share of a split's room any child keeps, when the split has
/// room for each to keep it.
const LEAST_SHARE: f64 = 0.05;

/// The most boxes one tree may have, and the deepest one may nest: past
/// either, an `arrange` is refused rather than solved.
pub const MOST_BOXES: usize = 4096;
pub const DEEPEST: usize = 256;

/// What a window's application says of its size, frame included. A side of
/// 0 or less says nothing about that side.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Limits {
    pub min: Option<Size>,
    pub max: Option<Size>,
}

/// Where a window of a tree comes out.
#[derive(Clone, Debug, PartialEq)]
pub struct Solved {
    pub window: u64,
    /// Where the window lives: its box, or the middle of its box at its
    /// application's largest size.
    pub rect: Rect,
    /// Its box.
    pub tile: Rect,
    /// Whether its box is smaller than its application's least size.
    pub cramped: bool,
}

/// A side of a window, as an edge drag names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    /// The side by its name in the `resized` event: `left`, `right`, `top`
    /// or `bottom`.
    pub fn named(name: &str) -> Option<Self> {
        match name {
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "top" => Some(Self::Top),
            "bottom" => Some(Self::Bottom),
            _ => None,
        }
    }

    fn axis(self) -> Axis {
        match self {
            Self::Left | Self::Right => Axis::X,
            Self::Top | Self::Bottom => Axis::Y,
        }
    }

    /// Whether the window is before the seam on this side: its right or
    /// bottom edge is the seam.
    fn before(self) -> bool {
        matches!(self, Self::Right | Self::Bottom)
    }
}

/// A seam of a tree: between child `after` and child `after + 1` of the
/// split at `split`, and which of the two the dragged window is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seam {
    pub split: usize,
    pub after: usize,
    /// Whether the dragged window is in the second of the two, so its
    /// leading edge is a gap past the seam.
    pub second: bool,
}

/// A rectangle, as the arithmetic here passes it around.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Area {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl Area {
    fn of(rect: &Rect) -> Self {
        Self {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
        }
    }

    fn rect(self) -> Rect {
        Rect {
            x: self.x,
            y: self.y,
            w: self.w,
            h: self.h,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Split {
        axis: Axis,
        gap: f64,
        resizable: bool,
    },
    Free,
    Window(u64),
    Empty,
}

#[derive(Clone, Debug, PartialEq)]
struct Node {
    id: u64,
    parent: Option<usize>,
    kind: Kind,
    share: f64,
    at: Option<Area>,
    min: (f64, f64),
    client_min: bool,
    client_max: bool,
    children: Vec<usize>,
}

/// A tree of boxes from an `arrange`, checked.
#[derive(Clone, Debug, PartialEq)]
pub struct Tree {
    area: Area,
    nodes: Vec<Node>,
}

/// A tree solved: each box's rectangle, each split's shares as drawn, and
/// where each window comes out, in the tree's order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    rects: Vec<Option<Area>>,
    shares: Vec<Vec<f64>>,
    pub windows: Vec<Solved>,
}

impl Layout {
    /// The box at `index`'s rectangle, when it was laid out.
    pub fn rect(&self, index: usize) -> Option<Rect> {
        self.area(index).map(Area::rect)
    }

    fn area(&self, index: usize) -> Option<Area> {
        self.rects.get(index).copied().flatten()
    }
}

fn finite(value: f64) -> bool {
    value.is_finite()
}

fn finite_rect(rect: &Rect) -> bool {
    finite(rect.x) && finite(rect.y) && finite(rect.w) && finite(rect.h)
}

/// The most and least of two numbers, a NaN giving way to the other.
fn most(a: f64, b: f64) -> f64 {
    a.max(b)
}

fn least(a: f64, b: f64) -> f64 {
    a.min(b)
}

/// `value` held to `low..high`, or `otherwise` when that range is empty or
/// `value` is not a number: a seam with nowhere to go stays where it is.
fn within(value: f64, low: f64, high: f64, otherwise: f64) -> f64 {
    if low <= high && !value.is_nan() {
        value.clamp(low, high)
    } else {
        otherwise
    }
}

fn along(axis: Axis, rect: &Area) -> f64 {
    match axis {
        Axis::X => rect.w,
        Axis::Y => rect.h,
    }
}

fn start(axis: Axis, rect: &Area) -> f64 {
    match axis {
        Axis::X => rect.x,
        Axis::Y => rect.y,
    }
}

fn pick(axis: Axis, size: (f64, f64)) -> f64 {
    match axis {
        Axis::X => size.0,
        Axis::Y => size.1,
    }
}

fn size_of(size: Option<&Size>) -> (f64, f64) {
    size.map_or((0.0, 0.0), |size| {
        (f64::from(size.w.max(0)), f64::from(size.h.max(0)))
    })
}

/// The least share each of `n` children keeps.
fn least_share(n: usize) -> f64 {
    if n == 0 {
        return 0.0;
    }
    #[allow(
        clippy::cast_precision_loss,
        reason = "a split has at most MOST_BOXES children"
    )]
    let equal = 1.0 / n as f64;
    least(LEAST_SHARE, equal)
}

/// Shares made fractions of one, each at least its least: one under it is
/// raised to it, and what that takes comes from the others in proportion to
/// what they have above it. For two, this is holding the first to
/// `0.05..0.95`.
fn floored(mut shares: Vec<f64>) -> Vec<f64> {
    let n = shares.len();
    if n == 0 {
        return shares;
    }
    let total: f64 = shares.iter().sum();
    if total > 0.0 && total.is_finite() {
        for share in &mut shares {
            *share /= total;
        }
    } else {
        #[allow(
            clippy::cast_precision_loss,
            reason = "a split has at most MOST_BOXES children"
        )]
        let equal = 1.0 / n as f64;
        shares.fill(equal);
    }
    let floor = least_share(n);
    // Each round raises at least one more child to the floor, so `n` rounds
    // settle it.
    for _ in 0..n {
        let deficit: f64 = shares.iter().map(|share| most(floor - share, 0.0)).sum();
        if deficit <= 0.0 {
            break;
        }
        let spare: f64 = shares.iter().map(|share| most(share - floor, 0.0)).sum();
        if spare <= 0.0 {
            break;
        }
        let take = least(deficit, spare);
        for share in &mut shares {
            if *share < floor {
                *share = floor;
            } else {
                *share -= (*share - floor) * take / spare;
            }
        }
    }
    shares
}

/// What a box asks of the room on an axis: its windows' least sizes to be
/// given (`pull`), and the most it can be taken down to when a sibling pulls
/// (`hold`), which is that or its boxes' own least, whichever is larger.
#[derive(Clone, Copy, Debug, Default)]
struct Need {
    pull: f64,
    hold: f64,
}

impl Tree {
    /// The tree an `arrange` describes, or why it is not one: a box in a
    /// box that holds none, a box before the box it is in, a number that is
    /// not one, a window twice.
    pub fn new(arrange: &Arrange) -> Result<Self, String> {
        Self::of(&arrange.area, &arrange.boxes)
    }

    /// The tree `boxes` describe over `area`.
    pub fn of(area: &Rect, boxes: &[SpaceBox]) -> Result<Self, String> {
        if !finite_rect(area) {
            return Err("its area is not a rectangle of numbers".to_owned());
        }
        if boxes.is_empty() {
            return Err("it has no boxes".to_owned());
        }
        if boxes.len() > MOST_BOXES {
            return Err(format!(
                "it has {} boxes, and a tree has at most {MOST_BOXES}",
                boxes.len()
            ));
        }
        let mut nodes: Vec<Node> = Vec::with_capacity(boxes.len());
        let mut windows = HashSet::new();
        let mut depth: Vec<usize> = Vec::with_capacity(boxes.len());
        for (index, each) in boxes.iter().enumerate() {
            let parent = match (index, each.parent) {
                (0, None) => None,
                (0, Some(_)) => return Err("its first box, the root, is in another".to_owned()),
                (_, None) => return Err(format!("box {index} is in no box, and only the root is")),
                (_, Some(parent)) => {
                    let parent = usize::try_from(parent).unwrap_or(usize::MAX);
                    if parent >= index {
                        return Err(format!(
                            "box {index} is in box {parent}, which does not come before it"
                        ));
                    }
                    if !matches!(nodes[parent].kind, Kind::Split { .. } | Kind::Free) {
                        return Err(format!(
                            "box {index} is in box {parent}, which is a window or empty and holds nothing"
                        ));
                    }
                    Some(parent)
                }
            };
            let level = parent.map_or(0, |parent| depth[parent] + 1);
            if level > DEEPEST {
                return Err(format!("box {index} is nested deeper than {DEEPEST}"));
            }
            depth.push(level);
            let kind = match &each.kind {
                SpaceKind::Split(split) => {
                    if !finite(split.gap) || split.gap < 0.0 {
                        return Err(format!("box {index}'s gap is not a size"));
                    }
                    Kind::Split {
                        axis: split.axis,
                        gap: split.gap,
                        resizable: split.resizable,
                    }
                }
                SpaceKind::Free => Kind::Free,
                SpaceKind::Window(id) => {
                    if !windows.insert(*id) {
                        return Err(format!("window {id} is in two boxes"));
                    }
                    Kind::Window(*id)
                }
                SpaceKind::Empty => Kind::Empty,
            };
            if !finite(each.share) || each.share < 0.0 {
                return Err(format!("box {index}'s share is not a share"));
            }
            if each.at.as_ref().is_some_and(|at| !finite_rect(at)) {
                return Err(format!("box {index}'s place is not a rectangle of numbers"));
            }
            nodes.push(Node {
                id: each.id,
                parent,
                kind,
                share: each.share,
                at: each.at.as_ref().map(Area::of),
                min: size_of(each.min.as_ref()),
                client_min: each.client_min,
                client_max: each.client_max,
                children: Vec::new(),
            });
            if let Some(parent) = parent {
                nodes[parent].children.push(index);
            }
        }
        Ok(Self {
            area: Area::of(area),
            nodes,
        })
    }

    /// The rectangle the root fills.
    pub fn area(&self) -> Rect {
        self.area.rect()
    }

    /// Every window in the tree, in its order.
    pub fn windows(&self) -> impl Iterator<Item = u64> + '_ {
        self.nodes.iter().filter_map(|node| match node.kind {
            Kind::Window(id) => Some(id),
            _ => None,
        })
    }

    pub fn contains(&self, window: u64) -> bool {
        self.leaf(window).is_some()
    }

    fn leaf(&self, window: u64) -> Option<usize> {
        self.nodes
            .iter()
            .position(|node| node.kind == Kind::Window(window))
    }

    /// Take a window out, leaving its box holding nothing, so nothing else
    /// moves. Whether it was in the tree.
    pub fn let_go(&mut self, window: u64) -> bool {
        match self.leaf(window) {
            Some(leaf) => {
                self.nodes[leaf].kind = Kind::Empty;
                true
            }
            None => false,
        }
    }

    /// The split at `index`'s id, and its children's shares as fractions of
    /// one: what `seam-moved` says.
    pub fn shares(&self, index: usize) -> Option<(u64, Vec<f64>)> {
        let node = self.nodes.get(index)?;
        matches!(node.kind, Kind::Split { .. }).then(|| (node.id, self.own(index)))
    }

    /// The split at `index`'s children's own shares, as fractions of one.
    fn own(&self, index: usize) -> Vec<f64> {
        floored(
            self.nodes[index]
                .children
                .iter()
                .map(|&child| self.nodes[child].share)
                .collect(),
        )
    }

    /// The least size window leaf `index` holds out for, from its
    /// application, when that counts.
    fn client_floor(&self, index: usize, limits: &impl Fn(u64) -> Limits) -> (f64, f64) {
        let node = &self.nodes[index];
        match node.kind {
            Kind::Window(id) if node.client_min => size_of(limits(id).min.as_ref()),
            _ => (0.0, 0.0),
        }
    }

    /// What the box at `index` asks of the room on `axis`.
    fn need(&self, index: usize, axis: Axis, limits: &impl Fn(u64) -> Limits) -> Need {
        let node = &self.nodes[index];
        match node.kind {
            Kind::Window(_) | Kind::Empty => {
                let pull = pick(axis, self.client_floor(index, limits));
                Need {
                    pull,
                    hold: most(pull, pick(axis, node.min)),
                }
            }
            Kind::Free => node.children.iter().fold(Need::default(), |sum, &child| {
                let each = self.need(child, axis, limits);
                Need {
                    pull: most(sum.pull, each.pull),
                    hold: most(sum.hold, each.hold),
                }
            }),
            Kind::Split { axis: own, gap, .. } => {
                let needs: Vec<Need> = node
                    .children
                    .iter()
                    .map(|&child| self.need(child, axis, limits))
                    .collect();
                if own == axis {
                    let mut hold = 0.0;
                    if needs.iter().any(|need| need.hold > 0.0) {
                        hold = needs.iter().map(|need| need.hold).sum::<f64>()
                            + gaps(gap, needs.len());
                    }
                    let pull = if needs.iter().any(|need| need.pull > 0.0) {
                        hold
                    } else {
                        0.0
                    };
                    Need { pull, hold }
                } else {
                    needs.iter().fold(Need::default(), |sum, each| Need {
                        pull: most(sum.pull, each.pull),
                        hold: most(sum.hold, each.hold),
                    })
                }
            }
        }
    }

    /// The shares the split at `index` is drawn at in `rect`: its own,
    /// unless some children are under their windows' least and others have
    /// room to give.
    fn drawn(&self, index: usize, rect: &Area, limits: &impl Fn(u64) -> Limits) -> Vec<f64> {
        let own = self.own(index);
        let node = &self.nodes[index];
        let Kind::Split { axis, gap, .. } = node.kind else {
            return own;
        };
        let needs: Vec<Need> = node
            .children
            .iter()
            .map(|&child| self.need(child, axis, limits))
            .collect();
        if needs.iter().all(|need| need.pull <= 0.0) {
            return own;
        }
        let span = most(along(axis, rect) - gaps(gap, own.len()), 1.0);
        let mut lengths: Vec<f64> = own.iter().map(|share| span * share).collect();
        let short: Vec<f64> = needs
            .iter()
            .zip(&lengths)
            .map(|(need, length)| need.pull - length)
            .collect();
        let is_short: Vec<bool> = short.iter().map(|&short| short > SLACK).collect();
        let deficit: f64 = short
            .iter()
            .zip(&is_short)
            .filter(|(_, short)| **short)
            .map(|(short, _)| short)
            .sum();
        let spare: f64 = needs
            .iter()
            .zip(&lengths)
            .zip(&is_short)
            .filter(|(_, short)| !**short)
            .map(|((need, length), _)| most(length - need.hold, 0.0))
            .sum();
        if deficit <= 0.0 || is_short.iter().all(|short| *short) {
            return own;
        }
        let give = least(deficit, spare);
        if give <= 0.0 {
            return own;
        }
        for (index, length) in lengths.iter_mut().enumerate() {
            if is_short[index] {
                *length += short[index] * give / deficit;
            } else {
                *length -= most(*length - needs[index].hold, 0.0) * give / spare;
            }
        }
        lengths.iter().map(|length| length / span).collect()
    }

    /// The least room on `axis` the box at `index` needs for every box in
    /// it to have its own least, its own seams staying where they are.
    fn floor(&self, index: usize, axis: Axis) -> f64 {
        let node = &self.nodes[index];
        match node.kind {
            Kind::Window(_) | Kind::Empty => pick(axis, node.min),
            Kind::Free => node
                .children
                .iter()
                .map(|&child| self.floor(child, axis))
                .fold(0.0, most),
            Kind::Split { axis: own, gap, .. } => {
                if own == axis {
                    let shares = self.own(index);
                    let widest = node
                        .children
                        .iter()
                        .zip(&shares)
                        .map(|(&child, share)| self.floor(child, axis) / share)
                        .fold(0.0, most);
                    // With no least anywhere in it, it asks nothing, not
                    // even its gaps: a seam is held only by what a box says
                    // it needs.
                    if widest > 0.0 {
                        gaps(gap, shares.len()) + widest
                    } else {
                        0.0
                    }
                } else {
                    node.children
                        .iter()
                        .map(|&child| self.floor(child, axis))
                        .fold(0.0, most)
                }
            }
        }
    }

    /// Solve the tree: where every box and window comes out, given what
    /// each window's application says of its size.
    pub fn solve(&self, limits: impl Fn(u64) -> Limits) -> Layout {
        let mut layout = Layout {
            rects: vec![None; self.nodes.len()],
            shares: vec![Vec::new(); self.nodes.len()],
            windows: Vec::new(),
        };
        self.lay(0, self.area, &limits, &mut layout);
        layout
    }

    fn lay(&self, index: usize, rect: Area, limits: &impl Fn(u64) -> Limits, out: &mut Layout) {
        out.rects[index] = Some(rect);
        let node = &self.nodes[index];
        match node.kind {
            Kind::Empty => {}
            Kind::Window(window) => {
                let floor = self.client_floor(index, limits);
                let cramped = rect.w + SLACK < floor.0 || rect.h + SLACK < floor.1;
                let mut placed = rect;
                if node.client_max {
                    let (most_w, most_h) = size_of(limits(window).max.as_ref());
                    let w = if most_w > 0.0 && rect.w > most_w {
                        most_w
                    } else {
                        rect.w
                    };
                    let h = if most_h > 0.0 && rect.h > most_h {
                        most_h
                    } else {
                        rect.h
                    };
                    placed = Area {
                        x: rect.x + (rect.w - w) / 2.0,
                        y: rect.y + (rect.h - h) / 2.0,
                        w,
                        h,
                    };
                    if w == rect.w && h == rect.h {
                        placed = rect;
                    }
                }
                out.windows.push(Solved {
                    window,
                    rect: placed.rect(),
                    tile: rect.rect(),
                    cramped,
                });
            }
            Kind::Free => {
                for &child in &node.children {
                    let at = self.nodes[child].at.unwrap_or(rect);
                    self.lay(child, at, limits, out);
                }
            }
            Kind::Split { axis, gap, .. } => {
                let shares = floored(self.drawn(index, &rect, limits));
                let rects = cut(&rect, axis, &shares, gap);
                out.shares[index] = shares;
                for (&child, child_rect) in node.children.iter().zip(rects) {
                    self.lay(child, child_rect, limits, out);
                }
            }
        }
    }

    /// The seam along `side` of `window`: the split cut on that side's axis
    /// nearest the window with the window's box on the side that puts the
    /// seam under the edge. None when the window is against its tree's edge
    /// there, inside a box placed freely, or when that split's seams are not
    /// resizable.
    pub fn seam_beside(&self, window: u64, side: Side) -> Option<Seam> {
        let mut at = self.leaf(window)?;
        loop {
            let parent = self.nodes[at].parent?;
            let node = &self.nodes[parent];
            match node.kind {
                Kind::Free => return None,
                Kind::Split {
                    axis, resizable, ..
                } if axis == side.axis() => {
                    let place = node.children.iter().position(|&child| child == at)?;
                    let after = if side.before() {
                        (place + 1 < node.children.len()).then_some(place)
                    } else {
                        place.checked_sub(1)
                    };
                    if let Some(after) = after {
                        return resizable.then_some(Seam {
                            split: parent,
                            after,
                            second: !side.before(),
                        });
                    }
                }
                _ => {}
            }
            at = parent;
        }
    }

    /// The shares child `seam.after` of the seam's split may be drawn at,
    /// as the seam moves: from a twentieth, narrowed so no box on either
    /// side goes under its own least and no window under its
    /// application's, and always taking in the share drawn now, so the
    /// first frame of a drag never jumps.
    fn room(&self, seam: Seam, layout: &Layout, limits: &impl Fn(u64) -> Limits) -> (f64, f64) {
        let node = &self.nodes[seam.split];
        let Kind::Split { axis, gap, .. } = node.kind else {
            return (0.0, 1.0);
        };
        let rect = layout.area(seam.split).unwrap_or(self.area);
        let drawn = &layout.shares[seam.split];
        let (Some(&current), Some(&next)) = (drawn.get(seam.after), drawn.get(seam.after + 1))
        else {
            return (0.0, 1.0);
        };
        let floor = least_share(drawn.len());
        let pair = current + next;
        let span = most(along(axis, &rect) - gaps(gap, drawn.len()), 1.0);
        let first = node.children[seam.after];
        let second = node.children[seam.after + 1];
        let mut low = floor;
        let mut high = pair - floor;
        low = most(low, self.floor(first, axis) / span);
        high = least(high, pair - self.floor(second, axis) / span);
        let first_need = self.need(first, axis, limits);
        let second_need = self.need(second, axis, limits);
        if first_need.pull > 0.0 {
            low = most(low, first_need.pull / span);
        }
        if second_need.pull > 0.0 {
            high = least(high, pair - second_need.pull / span);
        }
        (
            most(least(low, current), floor),
            least(most(high, current), pair - floor),
        )
    }

    /// Give the seam's first child `share` of the split, its second what
    /// the two had less that, every other child keeping its own.
    fn set(&mut self, seam: Seam, share: f64) {
        let mut own = self.own(seam.split);
        let pair = own[seam.after] + own[seam.after + 1];
        own[seam.after] = share;
        own[seam.after + 1] = pair - share;
        let children = self.nodes[seam.split].children.clone();
        for (child, share) in children.into_iter().zip(own) {
            self.nodes[child].share = share;
        }
    }

    /// Move `seam` so the dragged window's edge comes to rest at `edge` on
    /// the seam's axis, in the tree's space: a position and not a distance,
    /// so the same drag handled twice is the same arrangement.
    pub fn drag(&mut self, seam: Seam, edge: f64, limits: impl Fn(u64) -> Limits) {
        let layout = self.solve(&limits);
        let node = &self.nodes[seam.split];
        let Kind::Split { axis, gap, .. } = node.kind else {
            return;
        };
        let Some(rect) = layout.area(seam.split) else {
            return;
        };
        let drawn = &layout.shares[seam.split];
        let span = most(along(axis, &rect) - gaps(gap, drawn.len()), 1.0);
        // Where the seam's first child starts, as drawn.
        let mut from = start(axis, &rect);
        for &child in &node.children[..seam.after] {
            if let Some(child) = layout.area(child) {
                from = start(axis, &child) + along(axis, &child) + gap;
            }
        }
        let leading = if seam.second { gap } else { 0.0 };
        let share = (edge - from - leading) / span;
        let (low, high) = self.room(seam, &layout, &limits);
        let own = self.own(seam.split)[seam.after];
        self.set(seam, within(share, low, high, own));
    }

    /// Grow `window` on `axis` by `by`, a share of its split's room, by
    /// moving the seam after it, or the one before it the other way when
    /// there is none after. The seam moved, if any.
    pub fn grow(
        &mut self,
        window: u64,
        axis: Axis,
        by: f64,
        limits: impl Fn(u64) -> Limits,
    ) -> Option<Seam> {
        let (trailing, leading) = match axis {
            Axis::X => (Side::Right, Side::Left),
            Axis::Y => (Side::Bottom, Side::Top),
        };
        let (seam, towards) = match self.seam_beside(window, trailing) {
            Some(seam) => (seam, by),
            None => (self.seam_beside(window, leading)?, -by),
        };
        let layout = self.solve(&limits);
        let current = layout.shares[seam.split].get(seam.after).copied()?;
        let (low, high) = self.room(seam, &layout, &limits);
        let own = self.own(seam.split)[seam.after];
        self.set(seam, within(current + towards, low, high, own));
        Some(seam)
    }
}

/// The room `n` children of a split give to the gaps between them.
fn gaps(gap: f64, n: usize) -> f64 {
    #[allow(
        clippy::cast_precision_loss,
        reason = "a split has at most MOST_BOXES children"
    )]
    let between = n.saturating_sub(1) as f64;
    gap * between
}

/// A split's children's rectangles: each its share of what is left once the
/// gaps are taken out, never less than a pixel, the last taking what is
/// left. For two, `layouts/dwindle.lua`'s `cut`.
fn cut(rect: &Area, axis: Axis, shares: &[f64], gap: f64) -> Vec<Area> {
    let n = shares.len();
    let total = along(axis, rect);
    let room = total - gaps(gap, n);
    let mut out = Vec::with_capacity(n);
    let mut at = start(axis, rect);
    let mut used = 0.0;
    for (index, share) in shares.iter().enumerate() {
        let length = if index + 1 == n {
            most(room - used, 1.0)
        } else {
            most(room * share, 1.0)
        };
        used += length;
        out.push(match axis {
            Axis::X => Area {
                x: at,
                y: rect.y,
                w: length,
                h: rect.h,
            },
            Axis::Y => Area {
                x: rect.x,
                y: at,
                w: rect.w,
                h: length,
            },
        });
        at += length + gap;
    }
    out
}

#[cfg(test)]
mod tests;
