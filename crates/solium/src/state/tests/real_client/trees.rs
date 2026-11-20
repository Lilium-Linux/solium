//! **Trees of boxes, in the compositor** (`arrange`): a seam follows a
//! dragged edge on every frame with no question to the window manager, which
//! hears where it was left once; a tree is solved again when an
//! application's least size changes; `grow` moves a seam and says so.

use super::client_sizes::Desk;
use super::*;

/// Two windows side by side in one tree over 1000x600, with no gap, and a
/// window manager that counts what it is asked about an edge drag and
/// writes down each seam it is told of. Arranged on every `layout`, from
/// the shares it was last told, as a layout keeps them.
fn two_side_by_side(
    desk: &mut Desk,
) -> (
    Window,
    Window,
    xdg_toplevel::XdgToplevel,
    wl_surface::WlSurface,
) {
    let (left, left_toplevel, left_surface) = desk.open();
    let (right, _right_toplevel, _right_surface) = desk.open();
    let (a, b) = (desk.state.window_id(&left), desk.state.window_id(&right));
    desk.install(&format!(
        "asked, seams, share = 0, {{}}, 0.5\n\
         sol.on(\"resize\", function() asked = asked + 1 end)\n\
         sol.on(\"seam\", function(key, id, shares)\n\
             share = shares[1]\n\
             seams[#seams + 1] = string.format(\"%s %d %.3f\", key, id, shares[1])\n\
         end)\n\
         sol.on(\"layout\", function()\n\
             sol.arrange(\"desk\", {{ x = 0, y = 0, w = 1000, h = 600 }}, {{\n\
                 {{ id = 9, split = {{ axis = \"x\", gap = 0 }} }},\n\
                 {{ parent = 1, window = {a}, share = share, client_min = true }},\n\
                 {{ parent = 1, window = {b}, share = 1 - share, client_min = true }},\n\
             }})\n\
         end)"
    ));
    desk.state.trigger_relayout();
    (left, right, left_toplevel, left_surface)
}

fn tile(desk: &Desk, window: &Window) -> Rectangle<i32, Logical> {
    let pane = desk.state.panes.id_of(window).expect("a pane");
    desk.state
        .panes
        .get(pane)
        .and_then(Pane::placed)
        .expect("a window of a tree is in a tile")
}

/// **A seam follows the dragged edge on every frame, and the window manager
/// is never asked.** Three frames of a drag of the left window's right
/// edge: on each, the seam is where the edge was dragged, both windows'
/// tiles meet there, and no `resize` reached the window manager. Its
/// `seam` comes once, after the release, with where the seam was left.
#[test]
fn a_seam_follows_the_edge_on_every_frame_with_no_question_to_the_window_manager() {
    let mut desk = Desk::new();
    let (left, right, _, _) = two_side_by_side(&mut desk);
    assert_eq!(tile(&desk, &left), at(0, 0, 500, 600), "the premise");
    assert_eq!(tile(&desk, &right), at(500, 0, 500, 600), "the premise");

    desk.state.begin_resize(&left);
    for edge in [520, 560, 610] {
        desk.state.pending_resize = Some(ResizeRequest {
            window: left.clone(),
            wanted: at(0, 0, edge, 600),
            edge_at: (f64::from(edge), 300.0),
            edges: ResizeEdge::Right,
        });
        desk.state.settle_resize();
        assert_eq!(tile(&desk, &left), at(0, 0, edge, 600), "frame at {edge}");
        assert_eq!(
            tile(&desk, &right),
            at(edge, 0, 1000 - edge, 600),
            "frame at {edge}"
        );
        assert_eq!(
            desk.evaluate("return table.concat(seams, \",\")"),
            "",
            "told of the seam before the drag was over"
        );
    }
    desk.state.release_resize(&left);
    desk.state.settle_resize();
    assert_eq!(
        desk.evaluate("return tostring(asked)"),
        "0",
        "the window manager was asked about an edge its tree's seam answers"
    );
    assert_eq!(
        desk.evaluate("return table.concat(seams, \",\")"),
        "desk 9 0.610"
    );
    desk.state.settle_resize();
    assert_eq!(
        desk.evaluate("return table.concat(seams, \",\")"),
        "desk 9 0.610",
        "told twice"
    );

    // Its next arrangement carries the shares, so nothing moves.
    desk.state.trigger_relayout();
    assert_eq!(tile(&desk, &left), at(0, 0, 610, 600));
}

/// **A window whose application's least size grows is given the room by
/// the compositor**, from its tree's own solving, and hands it back when
/// the least size goes away.
#[test]
fn a_tree_is_solved_again_when_an_application_says_another_least_size() {
    let mut desk = Desk::new();
    let (left, right, toplevel, surface) = two_side_by_side(&mut desk);
    toplevel.set_min_size(700, 0);
    surface.commit();
    desk.pump();
    desk.state.settle_resize();
    assert_eq!(tile(&desk, &left), at(0, 0, 700, 600));
    assert_eq!(tile(&desk, &right), at(700, 0, 300, 600));

    toplevel.set_min_size(0, 0);
    surface.commit();
    desk.pump();
    desk.state.settle_resize();
    assert_eq!(
        tile(&desk, &left),
        at(0, 0, 500, 600),
        "the room handed back"
    );
}

/// **`grow` moves the seam after the window, and the window manager is told
/// where it was left**, once.
#[test]
fn grow_moves_a_seam_and_says_where() {
    let mut desk = Desk::new();
    let (left, _right, _, _) = two_side_by_side(&mut desk);
    let id = desk.state.window_id(&left);
    desk.place(Command::Grow {
        window: id,
        axis: solium_wm_protocol::Axis::X,
        by: 0.1,
        animation: AnimationSpec {
            duration: Duration::ZERO,
            ..AnimationSpec::default()
        },
    });
    assert_eq!(tile(&desk, &left), at(0, 0, 600, 600));
    desk.state.settle_resize();
    assert_eq!(
        desk.evaluate("return table.concat(seams, \",\")"),
        "desk 9 0.600"
    );
}
