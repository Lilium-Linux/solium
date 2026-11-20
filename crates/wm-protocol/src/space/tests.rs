use super::*;
use crate::{Size, Split};

fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect { x, y, w, h }
}

fn split(parent: Option<u32>, id: u64, axis: Axis, gap: f64, share: f64) -> SpaceBox {
    SpaceBox {
        parent,
        id,
        kind: SpaceKind::Split(Split {
            axis,
            gap,
            resizable: true,
        }),
        share,
        ..SpaceBox::default()
    }
}

fn window(parent: u32, window: u64, share: f64) -> SpaceBox {
    SpaceBox {
        parent: Some(parent),
        id: 100 + window,
        kind: SpaceKind::Window(window),
        share,
        client_min: true,
        client_max: true,
        ..SpaceBox::default()
    }
}

fn none(_: u64) -> Limits {
    Limits::default()
}

fn min_of(id: u64, w: i32, h: i32) -> impl Fn(u64) -> Limits {
    move |window| {
        if window == id {
            Limits {
                min: Some(Size { w, h }),
                max: None,
            }
        } else {
            Limits::default()
        }
    }
}

fn tiles(layout: &Layout) -> Vec<(u64, Rect)> {
    layout
        .windows
        .iter()
        .map(|solved| (solved.window, solved.rect.clone()))
        .collect()
}

fn close(a: &Rect, b: &Rect) -> bool {
    (a.x - b.x).abs() < 1e-6
        && (a.y - b.y).abs() < 1e-6
        && (a.w - b.w).abs() < 1e-6
        && (a.h - b.h).abs() < 1e-6
}

fn assert_tiles(layout: &Layout, want: &[(u64, Rect)]) {
    let got = tiles(layout);
    assert_eq!(got.len(), want.len(), "{got:?}");
    for ((id, got), (want_id, want)) in got.iter().zip(want) {
        assert_eq!(id, want_id, "{got:?}");
        assert!(close(got, want), "window {id}: {got:?}, not {want:?}");
    }
}

/// Two windows side by side, `gap` apart, over 1000 by 500.
fn pair(gap: f64, first: f64) -> Tree {
    Tree::of(
        &rect(0.0, 0.0, 1000.0, 500.0),
        &[
            split(None, 1, Axis::X, gap, 1.0),
            window(0, 7, first),
            window(0, 8, 1.0 - first),
        ],
    )
    .expect("a tree")
}

#[test]
fn one_window_fills_the_area() {
    let tree = Tree::of(
        &rect(12.0, 12.0, 800.0, 600.0),
        &[SpaceBox {
            kind: SpaceKind::Window(7),
            ..SpaceBox::default()
        }],
    )
    .expect("a tree");
    assert_tiles(&tree.solve(none), &[(7, rect(12.0, 12.0, 800.0, 600.0))]);
}

#[test]
fn a_split_shares_its_room_once_the_gaps_are_out() {
    assert_tiles(
        &pair(10.0, 0.5).solve(none),
        &[
            (7, rect(0.0, 0.0, 495.0, 500.0)),
            (8, rect(505.0, 0.0, 495.0, 500.0)),
        ],
    );
}

#[test]
fn a_split_of_three_shares_by_their_shares() {
    let tree = Tree::of(
        &rect(0.0, 0.0, 400.0, 100.0),
        &[
            split(None, 1, Axis::X, 0.0, 1.0),
            window(0, 1, 1.0),
            window(0, 2, 2.0),
            window(0, 3, 1.0),
        ],
    )
    .expect("a tree");
    assert_tiles(
        &tree.solve(none),
        &[
            (1, rect(0.0, 0.0, 100.0, 100.0)),
            (2, rect(100.0, 0.0, 200.0, 100.0)),
            (3, rect(300.0, 0.0, 100.0, 100.0)),
        ],
    );
}

#[test]
fn every_child_keeps_a_twentieth() {
    assert_tiles(
        &pair(0.0, 0.0).solve(none),
        &[
            (7, rect(0.0, 0.0, 50.0, 500.0)),
            (8, rect(50.0, 0.0, 950.0, 500.0)),
        ],
    );
    assert_eq!(floored(vec![0.02, 0.98]), vec![0.05, 0.95]);
}

#[test]
fn a_window_under_its_least_takes_room_from_its_sibling() {
    let layout = pair(0.0, 0.5).solve(min_of(7, 700, 0));
    assert_tiles(
        &layout,
        &[
            (7, rect(0.0, 0.0, 700.0, 500.0)),
            (8, rect(700.0, 0.0, 300.0, 500.0)),
        ],
    );
    assert!(layout.windows.iter().all(|solved| !solved.cramped));
}

#[test]
fn a_sibling_gives_no_more_than_it_holds() {
    let mut boxes = vec![
        split(None, 1, Axis::X, 0.0, 1.0),
        window(0, 7, 0.5),
        window(0, 8, 0.5),
    ];
    boxes[2].min = Some(Size { w: 400, h: 0 });
    let tree = Tree::of(&rect(0.0, 0.0, 1000.0, 500.0), &boxes).expect("a tree");
    let layout = tree.solve(min_of(7, 700, 0));
    assert_tiles(
        &layout,
        &[
            (7, rect(0.0, 0.0, 600.0, 500.0)),
            (8, rect(600.0, 0.0, 400.0, 500.0)),
        ],
    );
    assert!(layout.windows[0].cramped, "600 is under its 700");
    assert!(!layout.windows[1].cramped);
}

#[test]
fn two_windows_that_cannot_both_fit_keep_their_shares() {
    let both = |window: u64| Limits {
        min: Some(Size {
            w: if window == 7 { 700 } else { 600 },
            h: 0,
        }),
        max: None,
    };
    assert_tiles(
        &pair(0.0, 0.5).solve(both),
        &[
            (7, rect(0.0, 0.0, 500.0, 500.0)),
            (8, rect(500.0, 0.0, 500.0, 500.0)),
        ],
    );
}

#[test]
fn an_application_least_not_counted_takes_nothing() {
    let mut boxes = vec![
        split(None, 1, Axis::X, 0.0, 1.0),
        window(0, 7, 0.5),
        window(0, 8, 0.5),
    ];
    boxes[1].client_min = false;
    let tree = Tree::of(&rect(0.0, 0.0, 1000.0, 500.0), &boxes).expect("a tree");
    let layout = tree.solve(min_of(7, 700, 0));
    assert_eq!(layout.windows[0].rect.w, 500.0);
    assert!(!layout.windows[0].cramped);
}

#[test]
fn a_window_that_will_not_grow_sits_in_the_middle_of_its_box() {
    let small = |_| Limits {
        min: None,
        max: Some(Size { w: 300, h: 200 }),
    };
    let layout = pair(0.0, 0.5).solve(small);
    let first = &layout.windows[0];
    assert!(close(&first.tile, &rect(0.0, 0.0, 500.0, 500.0)));
    assert!(close(&first.rect, &rect(100.0, 150.0, 300.0, 200.0)));
}

/// A on the left; B over C on the right.
fn three() -> Tree {
    Tree::of(
        &rect(0.0, 0.0, 1000.0, 1000.0),
        &[
            split(None, 1, Axis::X, 0.0, 1.0),
            window(0, 1, 0.5),
            split(Some(0), 2, Axis::Y, 0.0, 0.5),
            window(2, 2, 0.5),
            window(2, 3, 0.5),
        ],
    )
    .expect("a tree")
}

#[test]
fn the_seam_beside_an_edge_is_found_up_the_tree() {
    let tree = three();
    let root = Seam {
        split: 0,
        after: 0,
        second: true,
    };
    assert_eq!(tree.seam_beside(2, Side::Left), Some(root));
    assert_eq!(tree.seam_beside(3, Side::Left), Some(root));
    assert_eq!(
        tree.seam_beside(1, Side::Right),
        Some(Seam {
            second: false,
            ..root
        })
    );
    assert_eq!(
        tree.seam_beside(2, Side::Bottom),
        Some(Seam {
            split: 2,
            after: 0,
            second: false
        })
    );
    assert_eq!(
        tree.seam_beside(3, Side::Top),
        Some(Seam {
            split: 2,
            after: 0,
            second: true
        })
    );
    assert_eq!(tree.seam_beside(1, Side::Left), None, "against the area");
    assert_eq!(tree.seam_beside(3, Side::Right), None);
    assert_eq!(tree.seam_beside(3, Side::Bottom), None);
}

#[test]
fn a_split_that_is_not_resizable_has_no_seam() {
    let mut boxes = vec![
        split(None, 1, Axis::X, 0.0, 1.0),
        window(0, 7, 0.5),
        window(0, 8, 0.5),
    ];
    boxes[0].kind = SpaceKind::Split(Split {
        axis: Axis::X,
        gap: 0.0,
        resizable: false,
    });
    let tree = Tree::of(&rect(0.0, 0.0, 1000.0, 500.0), &boxes).expect("a tree");
    assert_eq!(tree.seam_beside(7, Side::Right), None);
}

#[test]
fn a_dragged_edge_lands_where_it_was_asked_across_the_gap() {
    let mut tree = pair(12.0, 0.5);
    let seam = tree.seam_beside(8, Side::Left).expect("a seam");
    tree.drag(seam, 412.0, none);
    let layout = tree.solve(none);
    assert!(
        (layout.windows[1].rect.x - 412.0).abs() < 1e-9,
        "{layout:?}"
    );
    assert!((layout.windows[0].rect.w - 400.0).abs() < 1e-9);

    let mut tree = pair(12.0, 0.5);
    let seam = tree.seam_beside(7, Side::Right).expect("a seam");
    tree.drag(seam, 300.0, none);
    assert!((tree.solve(none).windows[0].rect.w - 300.0).abs() < 1e-9);
}

#[test]
fn the_same_drag_twice_is_the_same_arrangement() {
    let mut tree = three();
    let seam = tree.seam_beside(1, Side::Right).expect("a seam");
    tree.drag(seam, 640.0, none);
    let once = tree.clone();
    tree.drag(seam, 640.0, none);
    assert_eq!(tree, once);
}

#[test]
fn a_seam_stops_at_a_neighbours_least() {
    let mut tree = pair(0.0, 0.5);
    let seam = tree.seam_beside(7, Side::Right).expect("a seam");
    tree.drag(seam, 900.0, min_of(8, 300, 0));
    assert!((tree.solve(min_of(8, 300, 0)).windows[0].rect.w - 700.0).abs() < 1e-9);
}

#[test]
fn a_seam_stops_at_a_boxs_own_least_beyond_it() {
    let mut boxes = vec![
        split(None, 1, Axis::X, 0.0, 1.0),
        window(0, 7, 0.5),
        window(0, 8, 0.5),
    ];
    boxes[2].min = Some(Size { w: 250, h: 0 });
    let mut tree = Tree::of(&rect(0.0, 0.0, 1000.0, 500.0), &boxes).expect("a tree");
    let seam = tree.seam_beside(7, Side::Right).expect("a seam");
    tree.drag(seam, 990.0, none);
    assert!((tree.solve(none).windows[0].rect.w - 750.0).abs() < 1e-9);
}

#[test]
fn a_drag_on_a_seam_already_past_its_room_does_not_jump() {
    // The second window may be no less than 600 wide, and is 500 wide: the
    // seam is already past where its room ends, and a drag from there moves
    // it no further that way but does not throw it back either.
    let mut boxes = vec![
        split(None, 1, Axis::X, 0.0, 1.0),
        window(0, 7, 0.5),
        window(0, 8, 0.5),
    ];
    boxes[2].min = Some(Size { w: 600, h: 0 });
    let mut tree = Tree::of(&rect(0.0, 0.0, 1000.0, 500.0), &boxes).expect("a tree");
    let seam = tree.seam_beside(7, Side::Right).expect("a seam");
    tree.drag(seam, 520.0, none);
    assert!((tree.solve(none).windows[0].rect.w - 500.0).abs() < 1e-9);
    tree.drag(seam, 450.0, none);
    assert!((tree.solve(none).windows[0].rect.w - 450.0).abs() < 1e-9);
}

#[test]
fn growing_moves_the_seam_after_or_else_the_one_before() {
    let mut tree = pair(0.0, 0.5);
    let moved = tree.grow(7, Axis::X, 0.1, none).expect("a seam");
    assert_eq!(moved.split, 0);
    assert_eq!(tree.shares(0).map(|(id, _)| id), Some(1));
    assert!((tree.solve(none).windows[0].rect.w - 600.0).abs() < 1e-9);
    // Against the right edge, it grows from the left.
    tree.grow(8, Axis::X, 0.1, none).expect("a seam");
    assert!((tree.solve(none).windows[1].rect.w - 500.0).abs() < 1e-9);
    assert_eq!(tree.grow(7, Axis::Y, 0.1, none), None, "no seam on y");
}

#[test]
fn a_free_box_puts_each_child_where_it_says() {
    let tree = Tree::of(
        &rect(0.0, 0.0, 1000.0, 500.0),
        &[
            SpaceBox {
                kind: SpaceKind::Free,
                ..SpaceBox::default()
            },
            SpaceBox {
                parent: Some(0),
                kind: SpaceKind::Window(7),
                at: Some(rect(-300.0, 10.0, 400.0, 480.0)),
                ..SpaceBox::default()
            },
            SpaceBox {
                parent: Some(0),
                kind: SpaceKind::Window(8),
                at: Some(rect(112.0, 10.0, 400.0, 480.0)),
                ..SpaceBox::default()
            },
        ],
    )
    .expect("a tree");
    assert_tiles(
        &tree.solve(none),
        &[
            (7, rect(-300.0, 10.0, 400.0, 480.0)),
            (8, rect(112.0, 10.0, 400.0, 480.0)),
        ],
    );
    assert_eq!(
        tree.seam_beside(7, Side::Right),
        None,
        "a free box has no seams"
    );
}

#[test]
fn a_window_let_go_leaves_its_box_empty_and_nothing_moves() {
    let mut tree = three();
    let before = tree.solve(none);
    assert!(tree.let_go(2));
    assert!(!tree.contains(2));
    let after = tree.solve(none);
    assert_tiles(
        &after,
        &[
            (1, before.windows[0].rect.clone()),
            (3, before.windows[2].rect.clone()),
        ],
    );
    assert!(!tree.let_go(2));
}

#[test]
fn what_is_not_a_tree_is_refused() {
    let area = &rect(0.0, 0.0, 100.0, 100.0);
    let root = split(None, 1, Axis::X, 0.0, 1.0);
    assert!(Tree::of(area, &[]).is_err());
    assert!(
        Tree::of(area, &[window(0, 1, 1.0)]).is_err(),
        "a root in a box"
    );
    assert!(
        Tree::of(area, &[root.clone(), window(1, 1, 1.0)]).is_err(),
        "in itself"
    );
    assert!(
        Tree::of(area, &[root.clone(), window(0, 1, 1.0), window(1, 2, 1.0)]).is_err(),
        "in a window"
    );
    assert!(
        Tree::of(area, &[root.clone(), window(0, 1, 1.0), window(0, 1, 1.0)]).is_err(),
        "one window twice"
    );
    assert!(Tree::of(area, &[root.clone(), window(0, 1, f64::NAN)]).is_err());
    assert!(Tree::of(area, &[root.clone(), window(0, 1, -1.0)]).is_err());
    assert!(
        Tree::of(
            &rect(0.0, 0.0, f64::INFINITY, 1.0),
            std::slice::from_ref(&root)
        )
        .is_err()
    );
    let mut nan_gap = root;
    nan_gap.kind = SpaceKind::Split(Split {
        axis: Axis::X,
        gap: f64::NAN,
        resizable: true,
    });
    assert!(Tree::of(area, &[nan_gap]).is_err());
}

#[test]
fn a_tree_too_deep_is_refused() {
    let mut boxes = vec![split(None, 0, Axis::X, 0.0, 1.0)];
    for index in 0..=DEEPEST {
        boxes.push(split(
            Some(u32::try_from(index).expect("small")),
            0,
            Axis::X,
            0.0,
            1.0,
        ));
    }
    assert!(Tree::of(&rect(0.0, 0.0, 1.0, 1.0), &boxes).is_err());
}
