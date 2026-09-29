//! The tests of `state`: its rules asked directly (which monitor a rectangle is on, what is on
//! stage, which chrome or client a point belongs to, where a menu goes, what a grab may do), and
//! `real_client` and `drag_icon`, which stand up a display and drive it from a real client.

use super::*;

/// Two monitors side by side, 1920 wide each, as `space.output_geometry`
/// would report them.
fn two_monitors() -> [Rectangle<i32, Logical>; 2] {
    [
        Rectangle::new((0, 0).into(), (1920, 1080).into()),
        Rectangle::new((1920, 0).into(), (1920, 1080).into()),
    ]
}

fn at(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
    Rectangle::new((x, y).into(), (w, h).into())
}

/// **The cull `render::prepare` runs before it captures anything.**
///
/// A hidden workspace is not unmapped -- it is drawn a screen away -- so
/// "is any of this rectangle on a monitor" is the question that separates
/// a window worth capturing from one nothing will ever draw. Getting it
/// wrong in the cheap direction costs a permanent offscreen pass per
/// hidden window; getting it wrong in the other direction stops a visible
/// window being captured, which is a blank corner. So both directions are
/// asserted, and each line names an implementation it rules out.
#[test]
fn a_pane_is_on_a_monitor_only_if_some_monitor_covers_part_of_it() {
    let screens = two_monitors();

    assert!(
        anywhere_on(at(100, 100, 800, 600), screens),
        "a window in the middle of the first monitor is on it"
    );
    // Rules out `all(..)` in place of `any(..)`: this one touches only the
    // second screen, and with two monitors mapped that is the common case.
    assert!(
        anywhere_on(at(2000, 100, 800, 600), screens),
        "and one on the second monitor is on that"
    );
    assert!(
        anywhere_on(at(1800, 100, 400, 600), screens),
        "a window dragged across the bezel is on both"
    );

    // The case the cull exists for: a workspace hidden by being parked one
    // screen to the left of the desk. Rules out `|_| true`.
    assert!(
        !anywhere_on(at(-1920, 0, 1920, 1080), screens),
        "a workspace parked a screen away is on no monitor, which is how a \
         workspace switch hides one"
    );
    assert!(
        !anywhere_on(at(0, -2000, 800, 600), screens),
        "and so is one parked above the desk"
    );

    // Exclusive, matching `render::elements`. Rules out
    // `overlaps_or_touches`, which differs from `overlaps` only here.
    assert!(
        !anywhere_on(at(-800, 0, 800, 1080), screens),
        "a window whose right edge is exactly the monitor's left edge has \
         no pixel on it"
    );

    // Rules out a constant `true`, and covers the moment between a monitor
    // going away and the session noticing.
    assert!(!anywhere_on(at(100, 100, 800, 600), []));
}

/// **What the notice at the end of a reload is looking at.**
///
/// The recovery half of #116 shipped with no test at all, which is how the
/// inversion below went out with it. This is the decision itself: three
/// answers, and the third is the one that matters — a *lost* desktop and an
/// *empty* workspace are the same picture, so `None` and `Some(true)` are
/// different claims and the empty cases must not come back as "everything
/// is off stage".
#[test]
fn a_desktop_is_off_stage_only_when_there_is_a_desktop_and_it_is_off() {
    let screens = two_monitors().to_vec();
    // Each a window's slot and where it is drawn: both living on the first
    // monitor, one drawn there and one carried clear of every screen.
    let slot = at(100, 100, 800, 600);
    let on = (slot, slot.to_f64());
    let off = (slot, at(-4000, 0, 1920, 1080).to_f64());

    assert_eq!(
        nothing_on_stage([off], &screens),
        Some(true),
        "a window carried clear of every screen is the picture this warns about"
    );
    // Rules out `all(..)` for `any(..)` twice over: one window still on a
    // screen is a desktop that is there, whichever order they come in.
    assert_eq!(
        nothing_on_stage([off, on], &screens),
        Some(false),
        "one window still on a screen means the desktop is not lost"
    );
    assert_eq!(
        nothing_on_stage([on, off], &screens),
        Some(false),
        "and the answer does not depend on which window is looked at first"
    );

    // Neither of these is a question with an answer, and neither may come
    // back as `Some(true)`: the caller warns on exactly that, and a session
    // with nothing open would then be told its desktop had been carried
    // away.
    assert_eq!(
        nothing_on_stage([], &screens),
        None,
        "no windows is not an off-stage desktop"
    );
    assert_eq!(
        nothing_on_stage([on], &[]),
        None,
        "and neither is no screens, which is every window off every one of \
         them by arithmetic"
    );
}

/// **#134's third review: a window is on stage only on a monitor that
/// draws it.**
///
/// Two monitors side by side, and the left one's workspace 2 as the shipped
/// `workspaces.lua` parks it: the slot tiling gave it on the left monitor,
/// drawn 1.06 screens to the right -- squarely on the right monitor. The
/// renderer draws a pane only on the monitors its slot is on
/// (`render::drawn_on`), so none of it is on screen, and asked against
/// every screen, as this was, it was on stage.
#[test]
fn a_window_is_on_stage_only_on_a_monitor_that_draws_it() {
    let screens = two_monitors().to_vec();
    let slot = at(12, 12, 1896, 1056);
    let carried = Rectangle::<f64, Logical>::new(
        (12.0 + 1920.0 * 1.06, 12.0).into(),
        (1896.0, 1056.0).into(),
    );

    assert_eq!(
        nothing_on_stage([(slot, carried)], &screens),
        Some(true),
        "a window living on the left monitor, carried onto the right one, is on stage \
         -- where the right monitor never draws it"
    );
    // Rules out asking only the slot.
    assert_eq!(
        nothing_on_stage([(slot, slot.to_f64())], &screens),
        Some(false),
        "the same window drawn where it lives is on stage"
    );
    let right = at(1932, 12, 1896, 1056);
    assert_eq!(
        nothing_on_stage([(right, right.to_f64())], &screens),
        Some(false),
        "and so is one living and drawn on the right monitor"
    );
    // A slot across the bezel is on both, so a transform may carry it
    // around either: containment, not a single "its monitor".
    let straddling = at(1800, 100, 400, 600);
    let moved = Rectangle::<f64, Logical>::new((2000.0, 100.0).into(), (400.0, 600.0).into());
    assert_eq!(
        nothing_on_stage([(straddling, moved)], &screens),
        Some(false),
        "a window across the bezel moved onto the right monitor is still on stage"
    );
}

/// **#134's sixth review: a desk is put away when its selection is headed
/// anywhere, or to nothing.** See [`put_away`]. Until then it was put away
/// only when its whole screen was carried clear of itself, so a desk
/// parked by a work area narrower than the screen was not, and before the
/// fifth review a fade to nothing was not either.
#[test]
fn a_desk_is_put_away_when_its_selection_is_headed_anywhere_or_to_nothing() {
    use crate::group::Shift;
    let by = |dx: f64, dy: f64| Shift {
        dx,
        dy,
        ..Shift::NONE
    };

    assert!(
        put_away(by(1920.0 * 1.06, 0.0)),
        "a workspace parked a screen and a bit to the right, as the shipped `spread` parks \
         one, is on stage"
    );
    assert!(
        put_away(by(-1920.0, 0.0)) && put_away(by(0.0, 1080.0)),
        "a workspace exactly a screen to the left, or a screen down, is on stage"
    );
    assert!(
        put_away(by(-(1920.0 - 64.0), 0.0)) && put_away(by(0.0, 1080.0 - 64.0)),
        "a workspace parked a work area away at `spread = 1.0`, with a 64-pixel panel across \
         the slide, is on stage"
    );
    assert!(
        put_away(by(100.0, 40.0)),
        "a desk nudged and left on its screen is on stage"
    );
    assert!(
        put_away(Shift {
            opacity: 0.0,
            ..Shift::NONE
        }),
        "a desk faded to nothing where it is is on stage"
    );

    assert!(!put_away(Shift::NONE), "the desk in view is put away");
    assert!(
        !put_away(Shift {
            opacity: 0.4,
            ..Shift::NONE
        }),
        "a dimmed desk is put away"
    );
    assert!(
        !put_away(Shift {
            matrix: crate::mat4::Mat4::rotate_z(0.3),
            ..Shift::NONE
        }),
        "a turned desk is put away"
    );
}

/// **The hit tests' half of the same rule**: a point is on a window only
/// on a monitor that draws it. See [`shown_at`].
#[test]
fn a_point_is_on_a_window_only_on_a_monitor_that_draws_it() {
    let screens = two_monitors();
    let left = at(12, 12, 1896, 1056);
    let on_left = Point::<f64, Logical>::from((500.0, 500.0));
    let on_right = Point::<f64, Logical>::from((2500.0, 500.0));

    assert!(
        !shown_at(left, false, on_right, &screens),
        "a window living on the left monitor is under a point on the right one -- \
         which is where its hidden workspace is carried, and never drawn"
    );
    assert!(
        shown_at(left, false, on_left, &screens),
        "and it is under one on its own"
    );
    assert!(
        shown_at(at(1800, 100, 400, 600), false, on_right, &screens),
        "a window across the bezel is drawn on both monitors, so on either"
    );
    assert!(
        !shown_at(left, false, (-10.0, 500.0).into(), &screens),
        "a point on no monitor is on nothing, because nothing is drawn there"
    );
    assert!(
        shown_at(left, false, on_right, &[]),
        "with no monitors at all nothing is known, and nothing is refused"
    );
    // #126: what is left of a window whose client has gone, on the monitor
    // that draws it, and with no monitors to go by.
    assert!(
        !shown_at(left, true, on_left, &screens) && !shown_at(left, true, on_right, &[]),
        "a point is on what is left of a window whose client has gone"
    );
}

/// **The focus half measures a frame by its rectangle; the renderer by its
/// bleed, and not at all when it is transformed.** Pinned as they stand, so
/// that the two are not taken for one rule -- the note beside
/// [`crate::render::drawn_on`] says why it is harmless and why it matters.
///
/// A window living on the left monitor and drawn just past its left edge,
/// tilted as a mode might tilt it: [`staged`] says it is off stage, from
/// the rectangle, while `render::elements` skips its bleed cull for any
/// frame with a matrix or a deform and so still draws it. The renderer
/// needs a GPU the build container does not have, so its half is pinned by
/// the source text, as `render.rs`'s drag-icon test pins its ordering --
/// by its tokens, with the whitespace squeezed out of both, so a rustfmt
/// reflow of that line moves nothing here and a change to the condition
/// does.
#[test]
fn the_focus_half_counts_a_frame_by_its_rectangle_where_the_renderer_counts_its_bleed_and_every_transform()
 {
    let screens = two_monitors();
    let slot = at(12, 12, 400, 300);
    let tilted = Frame {
        rect: Rectangle::<f64, Logical>::new((-500.0, 12.0).into(), (400.0, 300.0).into()),
        matrix: crate::mat4::Mat4::rotate_z(0.3),
        ..Frame::real(slot)
    };
    assert!(
        !staged(slot, tilted, &screens),
        "a window whose rectangle is off every screen counts as on stage: the focus half \
         is measuring something other than its rectangle now, and the note beside \
         `render::drawn_on` is out of date"
    );

    let tokens = |text: &str| text.split_whitespace().collect::<String>();
    assert!(
        tokens(include_str!("../render.rs")).contains(&tokens(
            "if frame.matrix.is_identity()
                     && frame.deform.is_none()
                     && !reach.overlaps(screen.to_f64())"
        )),
        "`render::elements` no longer culls by the bleed, or now culls a transformed frame: \
         the note beside `render::drawn_on` is out of date"
    );
}

/// **And when it looks, which is the whole of the fault.**
///
/// `everything_is_off_stage` ran at `self.clock.now()`, immediately after
/// the reload's three dispatches. A workspace slide is an animation, and at
/// its own start instant it has moved nothing — so the notice described the
/// session that had just been thrown away. A reload that *rescued* an
/// off-stage desktop warned about it, and one that carried the desktop off
/// said nothing at all.
///
/// Asked of a real [`Groups`] carrying a real [`present::Transform`],
/// through the same `Shift::apply` that [`Solium::drawn_at`] ends in. Both
/// instants are asserted, because "sampled later" is only worth anything if
/// the earlier sample really does give the other answer — otherwise this
/// would pass against the code it is pinned against.
///
/// **The honest limit.** This pins [`SETTLED`] and what it is for; it
/// cannot see `everything_is_off_stage` choosing to sample somewhere else,
/// because that method needs a `Display` and a mapped `Space` and so cannot
/// be called here at all. Setting `SETTLED` back to `ZERO` — which is what
/// the code did — fails the second assertion.
#[test]
fn a_slide_that_has_not_started_yet_is_not_where_the_desks_end_up() {
    let start = Duration::from_secs(10);
    let screens = two_monitors().to_vec();
    let real = at(100, 100, 800, 600);

    let mut groups = crate::group::Groups::default();
    groups.declare(
        "desk-2",
        crate::group::Selection {
            members: vec![crate::group::Member::Window(7)],
            on: None,
        },
        start,
    );
    // A screen and a bit to the left, over 300ms: `workspaces.lua`'s own
    // numbers, and what a reload onto another workspace asks for.
    groups.present(
        "desk-2",
        crate::group::Shift {
            dx: -1920.0 * 1.06,
            ..crate::group::Shift::NONE
        },
        start,
        Duration::from_millis(300),
        present::Curve::OutCubic,
    );

    let where_it_is = |now| {
        (
            real,
            groups.on_window(7, None, now).apply(Frame::real(real)).rect,
        )
    };

    assert_eq!(
        nothing_on_stage([where_it_is(start)], &screens),
        Some(false),
        "at the instant the reload finishes, the slide it started has moved \
         nothing -- so this is the session before the reload, and reporting it \
         is reporting the wrong one"
    );
    assert_eq!(
        nothing_on_stage([where_it_is(start + SETTLED)], &screens),
        Some(true),
        "SETTLED is not far enough ahead for the transforms this reload started \
         to have landed, so the notice still describes the previous session"
    );
}

/// **#115: a client's limits, in the terms a layout reads them.**
mod limits {
    use super::*;

    /// **A limit is read in the window's own terms, frame included**: the
    /// frame's insets added to each side the client limited, and nothing to a
    /// side it did not, so 0 still reads as no limit. A titlebar is on top.
    #[test]
    fn a_limit_is_read_in_the_windows_own_terms_frame_included() {
        let titled = Insets {
            top: 32,
            left: 2,
            right: 2,
            bottom: 2,
        };
        let in_pane = super::snapshot::in_pane;
        assert_eq!(
            in_pane(Size::from((800, 600)), titled),
            Some(Size::from((804, 634)))
        );
        assert_eq!(
            in_pane(Size::from((800, 0)), titled),
            Some(Size::from((804, 0)))
        );
        assert_eq!(in_pane(Size::from((0, 0)), titled), None);
    }

    /// **X11 hints that say nothing are no limit**: smithay answers `None`
    /// for a flag the client left unset, and that side is 0 both ways.
    #[test]
    fn x11_hints_that_say_nothing_are_no_limit() {
        assert_eq!(
            Limits::from_hints(None, Some(Size::from((1024, 768)))),
            Limits {
                min: Size::from((0, 0)),
                max: Size::from((1024, 768)),
            }
        );
        assert_eq!(
            Limits::from_hints(Some(Size::from((320, 200))), None),
            Limits {
                min: Size::from((320, 200)),
                max: Size::from((0, 0)),
            }
        );
    }

    /// **A limit past any screen is read as the most there is.** A client
    /// may say `i32::MAX`, and a column laid out that wide put the next one
    /// past the end of `i32`.
    #[test]
    fn a_limit_past_any_screen_is_read_as_the_most_there_is() {
        assert_eq!(
            Limits::from_hints(
                Some(Size::from((i32::MAX, 0))),
                Some(Size::from((i32::MAX, i32::MAX)))
            ),
            Limits {
                min: Size::from((snapshot::MOST, 0)),
                max: Size::from((snapshot::MOST, snapshot::MOST)),
            }
        );
    }
}

#[test]
fn a_frame_reserves_what_it_always_reserved() {
    // The three answers `insets_of` used to assemble from two tables,
    // now read off one value.
    assert_eq!(
        insets_for(&crate::pane::Frame::Pending),
        Insets {
            top: TITLEBAR_HEIGHT,
            ..Insets::NONE
        },
        "a frame that has not been built yet still reserves room for one, \
         or the window changes shape the moment it arrives"
    );
    assert_eq!(
        insets_for(&crate::pane::Frame::None),
        Insets::NONE,
        "a pane that will never have a frame reserves nothing -- a \
         titlebar's worth of blank space with no titlebar in it is what \
         an Electron application looked like here"
    );
    // The third answer -- that a built frame reserves what its decoration
    // asked for, on every side, so that a bar along the left and a border
    // are the same mechanism -- was asserted here against a `Styled` arm
    // carrying a plain `Insets`. That arm carries the `Decoration` itself
    // now, and `Decoration::new` is private to `decoration.rs`, so the case
    // cannot be written here.
    //
    // It *can* be written there, and is: `decoration.rs`'s tests build real
    // frames. The reason this file does not reach over and do the same is
    // not that a frame needs a GPU and a display -- it does not, and this
    // comment said so for a while. It is that `solium_qml_start` assigns
    // the one `QGuiApplication` without a lock, so the tests that bring Qt
    // up share a mutex, and that mutex is in the module where they live.
    // A second, unsynchronised starter in another module is a data race.
}

/// Drawing is unclipped; input is not. A spike reaching over the next
/// window must not eat that window's clicks — the failure mode is a
/// neighbour that has silently stopped responding, with nothing on screen
/// to explain it.
///
/// **It passed the moment it was written, and that is the point.** Every
/// hit-test in this file — `chrome_under`, `decorated_under`,
/// `window_under` and `surface_under` — reaches its pane through
/// [`Solium::pane_outer`], and the two that own a decoration gate on
/// `drawn.rect.contains(location)` before a layer is asked anything at all.
/// So the invariant holds by construction and nothing *states* it: the
/// canvas is a rectangle that exists, is larger, is right there in
/// `decoration.rs`, and is exactly what someone fixing "my glow does not
/// take clicks" would reach for.
///
/// **What it is and is not.** It is the two rectangles' relationship, in
/// one place, with the reason written down; it is not a guard on the
/// hit-tests, and it adds no machine-checked coverage that `decoration.rs`
/// did not already have. Both controls below were run, and between them
/// they are the honest limit of this test: the one it fails is pinned
/// twice over elsewhere, and the regression it is named for is pinned
/// nowhere.
///
/// | control | measured |
/// |---|---|
/// | `canvas` returning `outer` — the state before Task 5 | fails on the first assertion, but so do `decoration::tests::a_canvas_is_the_pane_grown_by_its_bleed` and `no_bleed_means_the_canvas_is_the_pane`, which pin it already |
/// | the frame band **and** `decorated_under` switched to the canvas, through `decoration::spread` | the whole suite still passes, 201 of 201 |
///
/// The regression is instead caught with a real pointer, which is what the
/// task's step 4 is for: two panes side by side under `bleedy`, a press
/// 60px into the first one's bleed and 20px inside the second, and the
/// second takes focus. Run against the canvas-hit-test build above, the
/// *first* window takes it and the second is left unfocused — a neighbour
/// that has silently stopped responding, exactly as described.
///
/// A point 30px to the *left* of the pane: inside a canvas that bled 50, and
/// outside the pane on the only axis that matters.
#[test]
fn a_point_in_the_bleed_is_not_in_the_pane() {
    let outer = Rectangle::<i32, Logical>::new((100, 100).into(), (200, 200).into());
    let bleed = crate::style::Bleed {
        top: 50,
        right: 50,
        bottom: 50,
        left: 50,
    };
    let canvas = crate::decoration::canvas(outer, bleed);
    let in_bleed = smithay::utils::Point::<f64, smithay::utils::Logical>::from((70.0, 120.0));
    assert!(canvas.to_f64().contains(in_bleed));
    assert!(!outer.to_f64().contains(in_bleed));
}

/// An ordinary framed window: 400x300 at (100, 100) under a titlebar and
/// nothing else reserved, which is the shape both of #108's symptoms were
/// reported on.
fn framed() -> (Rectangle<i32, Logical>, Insets) {
    (
        Rectangle::new((100, 100).into(), (400, 300).into()),
        Insets {
            top: TITLEBAR_HEIGHT,
            ..Insets::NONE
        },
    )
}

/// What [`Solium::pane_chrome`] makes of a point of that window, with the
/// pane drawn where the layout put it — no transform, so a screen point
/// and a pane-local point differ only by the pane's corner.
fn chrome_at(point: (f64, f64)) -> Option<Chrome> {
    let (outer, insets) = framed();
    let location = Point::<f64, Logical>::from(point);
    chrome_of(
        on_frame(outer.size, insets, location - outer.loc.to_f64()),
        resize::border_edges(outer, location),
    )
}

/// The whole of [`claim_of`] at a point of that window, with the two links
/// above the chrome supplied by the caller: `surface` is an interactive
/// scripted surface above the windows claiming the point, `mode` is a
/// script grab held — `sol.grab(true)`, which is overview.
fn claim_at(point: (f64, f64), surface: bool, mode: bool) -> Claim {
    claim_of(surface, mode, chrome_at(point))
}

/// **Issue #108, and the assertion the fix is actually for.**
///
/// The cursor and the press must be reading the same answer, because the
/// bug was that they were not. The frame's band and the resize border
/// genuinely overlap — the top `RESIZE_BORDER` pixels of a titlebar are
/// within reach of the window's top edge — and the press had always
/// resolved that in the frame's favour while the pointer resolved it not
/// at all, leaving whatever a CSD client had set for its own shadow's
/// resize affordance. So in that band the pointer drew a resize arrow and
/// a drag moved the window.
///
/// What is pinned here is that no point can be claimed by both: whatever
/// [`claim_of`] answers is *the* answer, and the cursor is a function of it
/// rather than of a second hit test. A test that checked only the
/// edge-to-icon mapping would pass with the second symptom still in place,
/// which is why the sweep below is the body of this test and the named
/// points are only the landmarks.
///
/// **The chrome is one link of three, and the first version of this test
/// swept only that one.** It was green while the pointer still promised a
/// resize over an overview thumbnail and over the bottom edge of a scripted
/// bar, because it never held a script grab and never put a surface over
/// the point — so it exercised exactly the world in which the bug does not
/// appear. The sweep is now run in all four worlds, and the deferring ones
/// are checked against the count of pixels the chrome *would* have claimed,
/// so a chain that quietly stopped deferring could not leave this passing.
#[test]
fn the_pointer_and_the_press_cannot_claim_different_things() {
    let (outer, insets) = framed();

    // The overlap is real, not theoretical -- without this the sweep's
    // "never both" would be vacuously true and would stay true if somebody
    // shrank the titlebar to nothing.
    let contested = Point::<f64, Logical>::from((300.0, 104.0));
    assert!(
        on_frame(outer.size, insets, contested - outer.loc.to_f64()),
        "four pixels below the top edge is inside a {TITLEBAR_HEIGHT}px \
         titlebar"
    );
    assert_ne!(
        resize::border_edges(outer, contested),
        ResizeEdge::None,
        "and inside the top resize border, which is what the two disagreed \
         about"
    );
    // The frame takes it, because the frame is what a press there does.
    assert_eq!(chrome_at((300.0, 104.0)), Some(Chrome::Frame));
    assert_eq!(
        claim_at((300.0, 104.0), false, false).cursor(),
        Some(CursorIcon::Default),
        "the band that moves the window must not draw a resize cursor: \
         that is #108's second symptom"
    );

    // Four pixels *above* the top edge is outside the window, so the frame
    // has no claim on it and the border does. This is where dragging the
    // top edge still works, and it now says so.
    assert_eq!(
        chrome_at((300.0, 96.0)),
        Some(Chrome::Resize(ResizeEdge::Top))
    );

    // The first symptom: the bottom-right corner resizes, and now looks
    // like it. Nothing is reserved along the bottom or the right, so the
    // corner is the client's pixels and the border's claim alone.
    let corner = (497.0, 397.0);
    assert_eq!(
        chrome_at(corner),
        Some(Chrome::Resize(ResizeEdge::BottomRight))
    );
    assert_eq!(
        claim_at(corner, false, false).cursor(),
        Some(CursorIcon::NwseResize)
    );

    // The middle of the client is nobody's chrome, which is what leaves a
    // client free to name its own cursor over its own window.
    assert_eq!(chrome_at((300.0, 250.0)), None);
    assert_eq!(chrome_at((900.0, 900.0)), None);

    // **A mode holds the grab: overview.** `lua/overview.lua` sets it and
    // hit-tests its own thumbnails, and `pointer_button` hands every press
    // to the mode before it looks at any chrome -- so a press on that same
    // corner focuses the window and leaves overview. Offering
    // `NwseResize` there is the pointer describing an action that will not
    // happen, which is the whole of #108, on `super+space`.
    assert_eq!(claim_at(corner, false, true), Claim::Mode);
    assert_eq!(
        claim_at(corner, false, true).cursor(),
        None,
        "in a mode the press is the mode's, so the pointer promises nothing"
    );
    assert_eq!(claim_at((300.0, 104.0), false, true).cursor(), None);

    // **A scripted surface takes the press.** The bottom eight pixels of a
    // `layer = "top"` bar are within reach of a maximised window's top
    // edge, and the tweaks panel is a full-height overlay down the right of
    // one. The press goes to the panel; the border cursor would have said
    // it resized the window.
    assert_eq!(claim_at(corner, true, false), Claim::Surface);
    assert_eq!(
        claim_at(corner, true, false).cursor(),
        None,
        "the press is the surface's, so the pointer leaves the shape to it"
    );
    assert_eq!(claim_at((300.0, 104.0), true, false).cursor(), None);

    // And the order between the two, which is the order `pointer_button`
    // asks them in: a surface above the windows is offered the press before
    // the mode is consulted.
    assert_eq!(claim_of(true, true, None), Claim::Surface);
    assert_eq!(claim_of(false, false, None), Claim::Nothing);
    assert_eq!(claim_of(false, false, None).cursor(), None);

    // And the whole neighbourhood of the window, a pixel at a time, in
    // each of the four worlds the two links above the chrome make.
    let mut chrome_pixels = 0_u32;
    let mut deferred_pixels = 0_u32;
    for (surface, mode) in [(false, false), (true, false), (false, true), (true, true)] {
        for y in 80..=420 {
            for x in 80..=520 {
                let location = Point::<f64, Logical>::from((f64::from(x), f64::from(y)));
                let framed_here = on_frame(outer.size, insets, location - outer.loc.to_f64());
                let edges = resize::border_edges(outer, location);
                let chrome = chrome_of(framed_here, edges);
                if !surface && !mode && chrome.is_some() {
                    chrome_pixels += 1;
                }
                if (surface || mode) && chrome.is_some() {
                    deferred_pixels += 1;
                }
                match claim_of(surface, mode, chrome) {
                    Claim::Surface => {
                        assert!(
                            surface,
                            "nothing was over {location:?} and the pointer \
                             stood aside for it"
                        );
                        assert_eq!(claim_of(surface, mode, chrome).cursor(), None);
                    }
                    Claim::Mode => {
                        assert!(
                            mode && !surface,
                            "no mode holds the grab at {location:?}, or a \
                             surface should have taken it first"
                        );
                        assert_eq!(claim_of(surface, mode, chrome).cursor(), None);
                    }
                    Claim::Chrome(Chrome::Frame) => {
                        assert!(
                            !surface && !mode,
                            "a press at {location:?} would never reach the \
                             frame, and the pointer says it would"
                        );
                        assert!(
                            framed_here,
                            "a press at {location:?} would not hit the \
                             frame, but the pointer says it would"
                        );
                    }
                    Claim::Chrome(Chrome::Resize(edges)) => {
                        assert!(
                            !surface && !mode,
                            "a press at {location:?} would never reach the \
                             resize border, and the pointer offered to drag \
                             it {edges:?}"
                        );
                        assert!(
                            !framed_here,
                            "a press at {location:?} moves the window, and \
                             the pointer offered to resize it {edges:?}"
                        );
                        assert_ne!(edges, ResizeEdge::None);
                        assert_eq!(
                            Chrome::Resize(edges).cursor(),
                            resize::cursor(edges),
                            "the cursor over a border is the border's own, \
                             whatever else is on screen"
                        );
                    }
                    Claim::Nothing => assert!(
                        !surface && !mode && !framed_here && edges == ResizeEdge::None,
                        "the compositor asserts nothing at {location:?} \
                         while claiming to own it"
                    ),
                }
            }
        }
    }

    // The deferring worlds are only worth sweeping if the chrome had
    // something to say at those pixels, which is what made the first
    // version of this test green over a live disagreement. Three of the
    // four worlds defer, so the same pixels are counted three times.
    assert!(chrome_pixels > 0, "the sweep never crossed any chrome");
    assert_eq!(
        deferred_pixels,
        chrome_pixels * 3,
        "every pixel the chrome would have claimed must be one the pointer \
         gave up in each of the three worlds where the press never gets \
         there"
    );
}

/// The frame band is the insets and only the insets.
///
/// Two directions, because getting it wrong either way is a bug with a
/// face: too wide and a strip of the client stops taking clicks, too
/// narrow and the titlebar has a dead line along one edge. The outer bound
/// is asserted separately -- it is what stops every point above a window
/// from counting as its titlebar, which is the mistake a "not the client
/// rect" test makes on its own.
#[test]
fn the_frame_band_is_what_the_insets_reserved() {
    let (outer, insets) = framed();
    let local = |x: f64, y: f64| Point::<f64, Logical>::from((x, y));

    assert!(on_frame(outer.size, insets, local(200.0, 0.0)));
    assert!(on_frame(
        outer.size,
        insets,
        local(200.0, f64::from(TITLEBAR_HEIGHT) - 1.0)
    ));
    assert!(
        !on_frame(outer.size, insets, local(200.0, f64::from(TITLEBAR_HEIGHT))),
        "the first row below the bar is the client's"
    );
    assert!(
        !on_frame(outer.size, insets, local(200.0, -1.0)),
        "a point above the window is not its titlebar"
    );
    assert!(
        !on_frame(outer.size, insets, local(200.0, 500.0)),
        "nor is a point below it, which reserves nothing"
    );
    assert!(
        !on_frame(outer.size, Insets::NONE, local(200.0, 0.0)),
        "a decoration that reserves nothing owns no band, and its clicks \
         belong to the window under it"
    );
}

/// One pane of a stack, as the pure rules see it: where it is drawn, and
/// what its frame reserves.
///
/// No presentation transform, so the drawn rect *is* the outer rect and a
/// screen point differs from a pane-local one only by the pane's corner.
/// That is the situation #111 was reported in — two ordinary overlapping
/// windows on the desktop, neither of them in a mode — and it keeps the
/// arithmetic below readable enough to check by hand. A built decoration is
/// assumed, which both windows in the report had.
#[derive(Clone, Copy)]
struct Stacked {
    outer: Rectangle<i32, Logical>,
    insets: Insets,
    /// Whether the client has arrived. `false` is a pane still loading: it
    /// is drawn and it covers, its frame's buttons work, and it has no
    /// window for a resize border to drag.
    window: bool,
    /// Whether the layout owns this pane. `false` is an X11 menu, tooltip
    /// or dropdown the client placed itself.
    managed: bool,
    /// Whether this pane paints anything. `false` is a pane held at opacity
    /// zero — which on this desktop means one between its close animation
    /// landing and its client acting on the request. It has a rectangle and
    /// it covers nothing.
    shows: bool,
}

impl Stacked {
    /// A framed window at a corner: a titlebar across the top and nothing
    /// else reserved, which is [`framed`]'s shape at an arbitrary place.
    fn window(at: (i32, i32), size: (i32, i32)) -> Self {
        Self {
            outer: Rectangle::new(at.into(), size.into()),
            insets: Insets {
                top: TITLEBAR_HEIGHT,
                ..Insets::NONE
            },
            window: true,
            managed: true,
            shows: true,
        }
    }

    /// The same pane with its application not yet arrived.
    const fn loading(mut self) -> Self {
        self.window = false;
        self
    }

    /// The same pane placed by its own client: a menu, a tooltip, a
    /// dropdown. It draws, so it covers; it is nothing's to resize or move.
    const fn unmanaged(mut self) -> Self {
        self.managed = false;
        self
    }

    /// The same pane held at opacity zero: asked to close and waiting on a
    /// client that has not answered. It is still in the stack, still the
    /// topmost thing at this rectangle, and on screen it is not there.
    const fn invisible(mut self) -> Self {
        self.shows = false;
        self
    }

    /// What [`Solium::pane_chrome`] makes of a point, out of the same two
    /// functions in the same order it uses them.
    ///
    /// [`chrome_offered`] and [`pane_hit_of`] are called here rather than
    /// reimplemented, which is the difference between a fixture and a
    /// second copy of the rule: `covers` is what `pane_chrome` passes —
    /// the *drawn* rect containing the point, which with no presentation
    /// transform is this rect — and both of `chrome_offered`'s gates apply
    /// exactly as they do there. A hand-rolled composition here is how the
    /// covers-before-chrome mistake could come back with every test still
    /// green.
    fn hit(self, point: (f64, f64)) -> PaneHit<Chrome> {
        let location = Point::<f64, Logical>::from(point);
        // Both halves of `Frame::covers`, in the same order and for the
        // same reason `pane_chrome` asks them: the rectangle says where the
        // pane would be drawn, and `shows` says whether it is drawn at all.
        let covers = self.shows && self.outer.to_f64().contains(location);
        let framed = covers
            && on_frame(
                self.outer.size,
                self.insets,
                location - self.outer.loc.to_f64(),
            );
        pane_hit_of(
            chrome_offered(
                self.shows,
                self.managed,
                self.window,
                framed,
                resize::border_edges(self.outer, location),
            ),
            covers,
        )
    }
}

/// The walk as it stood before #111: every pane's frame over the whole
/// stack, and only then every pane's resize border, with a pane that merely
/// *covers* the point stopping nothing.
///
/// Kept rather than deleted so the bug is pinned and not only the fix. A
/// test that asserts the new answer alone stays green against a walk that
/// never occludes anything — which is how this fault survived #108's
/// rewrite of the very same function, and why each test below measures both
/// rules at the same point.
///
/// `stack` is topmost-first, as [`Solium::chrome_under`]'s `rev` makes it.
fn two_pass(stack: &[Stacked], point: (f64, f64)) -> Option<Chrome> {
    let claimed = |frames: bool| {
        stack.iter().find_map(|pane| match pane.hit(point) {
            // Drawn or not made no difference to this walk: it asked each
            // pane for chrome and took the first that answered.
            PaneHit::Chrome(Chrome::Frame) | PaneHit::Halo(Chrome::Frame) if frames => {
                Some(Chrome::Frame)
            }
            PaneHit::Chrome(Chrome::Resize(edges)) | PaneHit::Halo(Chrome::Resize(edges))
                if !frames =>
            {
                Some(Chrome::Resize(edges))
            }
            _ => None,
        })
    };
    claimed(true).or_else(|| claimed(false))
}

/// The walk as `ab11731` first fixed #111: one pass, topmost first, with
/// *any* chrome claim winning outright whether or not the pane claiming it
/// draws anything at that point.
///
/// The second control, kept for the same reason [`two_pass`] is. It got the
/// covered titlebar right — that was the fix — and it got a halo over a
/// lower pane's drawn chrome wrong, because "which window is on top" was
/// asked at a pixel the upper window does not occupy. Every case below
/// measures all three rules at the same point, so what each one gets right
/// and wrong is written down rather than remembered.
fn halo_wins(stack: &[Stacked], point: (f64, f64)) -> Option<Chrome> {
    for pane in stack {
        match pane.hit(point) {
            PaneHit::Chrome(chrome) | PaneHit::Halo(chrome) => return Some(chrome),
            PaneHit::Client => return None,
            PaneHit::Miss => {}
        }
    }
    None
}

/// **Issue #111, as reported: a titlebar took clicks through the window
/// covering it.**
///
/// Two overlapping windows. A press on the *top* one, at a point where the
/// lower one's titlebar happened to lie underneath, focused and raised the
/// lower window. The walk searched only for chrome, so the top window —
/// whose client covers the point and which therefore had no chrome to
/// offer — did not stop the descent, and the lower window's titlebar was
/// found beneath it.
///
/// The last assertion is the control, and it was run red before the fix:
/// with the first assertion pointed at `two_pass` the test fails with
/// `Some(Frame)` against an expected `None`, which is the reported
/// behaviour reproduced in a unit test rather than on hardware.
#[test]
fn a_covered_titlebar_does_not_take_the_click() {
    let lower = Stacked::window((100, 100), (400, 300));
    let upper = Stacked::window((60, 60), (400, 300));
    let stack = [upper, lower];

    // (300, 116) is 56px down into the upper window: past its 32px titlebar
    // and 160px clear of its nearest resize border, so it is that window's
    // client and nothing else. The same point is 16px down into the lower
    // window, inside its titlebar and clear of its top border -- so the two
    // windows genuinely disagree here, which is what makes the walk's
    // answer worth anything.
    let point = (300.0, 116.0);
    assert_eq!(
        upper.hit(point),
        PaneHit::Client,
        "the top window covers this point with its client"
    );
    assert_eq!(
        lower.hit(point),
        PaneHit::Chrome(Chrome::Frame),
        "and the lower window's titlebar is underneath it, which is the \
         whole setup"
    );

    assert_eq!(
        topmost_chrome(stack.iter().map(|pane| pane.hit(point))),
        None,
        "a press on the top window's client is the client's, and the \
         titlebar buried under it may not have it"
    );
    assert_eq!(
        two_pass(&stack, point),
        Some(Chrome::Frame),
        "the walk this replaced hands the point to the covered titlebar, \
         which is the bug"
    );
    assert_eq!(
        halo_wins(&stack, point),
        None,
        "and the one-pass rule that replaced it got this case right -- \
         what it got wrong is the halo, below"
    );
}

/// **A resize border hanging outside its own window loses to anything a
/// lower pane actually draws, and beats bare desktop.**
///
/// A border reaches [`resize::RESIZE_BORDER`] pixels *outside* the window it
/// belongs to, over whatever is behind. Out there the pane draws nothing, so
/// the claim is not backed by a single pixel and "which window is on top"
/// has no answer: an upper window's bottom edge floating four pixels above a
/// lower window's close button is not on top of that button in any sense a
/// user would recognise. It is beside it. The button is what is drawn there
/// and the button takes the press.
///
/// The halo still wins over the desktop, which is the only reason an edge
/// can be grabbed from outside at all — and its *inside* half still wins
/// outright, since there the pane does draw the pixel it is claiming.
///
/// Three rules are measured at every point: the corrected one, the
/// [`two_pass`] walk from before #111, and [`halo_wins`] as #111 was first
/// fixed. The first case is the one that separates them.
#[test]
fn a_halo_loses_to_what_a_lower_pane_draws() {
    let lower = Stacked::window((100, 100), (400, 300));
    // Overlapping the top-right of the lower window, where its close button
    // is. The bottom edge, y = 120, floats inside the lower window's
    // titlebar band -- drawn, and 30px clear of the lower window's own
    // right border, so the only things meeting here are one window's empty
    // margin and another window's buttons.
    let upper = Stacked::window((300, 20), (200, 100));
    let stack = [upper, lower];

    let button = (470.0, 124.0);
    assert_eq!(
        upper.hit(button),
        PaneHit::Halo(Chrome::Resize(ResizeEdge::Bottom)),
        "4px below the upper window's bottom edge is its resize border, and \
         outside everything that window draws"
    );
    assert_eq!(
        lower.hit(button),
        PaneHit::Chrome(Chrome::Frame),
        "and 24px down into the lower window's titlebar, which is drawn"
    );
    assert_eq!(
        topmost_chrome(stack.iter().map(|pane| pane.hit(button))),
        Some(Chrome::Frame),
        "the titlebar is drawn there and the border is not, so the press is \
         the button's"
    );
    assert_eq!(
        two_pass(&stack, button),
        Some(Chrome::Frame),
        "which the walk from before #111 also answered -- it was not wrong \
         here, only wrong about why, having never asked which pane was on \
         top at all"
    );
    assert_eq!(
        halo_wins(&stack, button),
        Some(Chrome::Resize(ResizeEdge::Bottom)),
        "where #111's first fix drew NsResize over a visible close button \
         and started a resize grab on it"
    );

    // The same halo over a lower window's *client*, which is drawn just as
    // surely as its titlebar is. Nothing is offered and the client keeps
    // its own cursor: a window's margin does not reach through the window
    // under it.
    let deeper = Stacked::window((300, 20), (200, 180));
    let body = (470.0, 204.0);
    assert_eq!(
        deeper.hit(body),
        PaneHit::Halo(Chrome::Resize(ResizeEdge::Bottom))
    );
    assert_eq!(lower.hit(body), PaneHit::Client);
    assert_eq!(
        topmost_chrome([deeper, lower].iter().map(|pane| pane.hit(body))),
        None
    );
    assert_eq!(
        two_pass(&[deeper, lower], body),
        Some(Chrome::Resize(ResizeEdge::Bottom)),
        "and here the older walk is wrong too, so neither rule this \
         replaces got the halo right"
    );

    // Over the desktop the halo is the best claim there is, and it must
    // still win: this is what makes an edge grabbable from outside, and a
    // rule that demanded a pane draw what it claims would take every
    // window's outer border away.
    let sky = (400.0, 16.0);
    assert_eq!(
        upper.hit(sky),
        PaneHit::Halo(Chrome::Resize(ResizeEdge::Top)),
        "4px above the upper window's top edge"
    );
    assert_eq!(lower.hit(sky), PaneHit::Miss);
    assert_eq!(
        topmost_chrome(stack.iter().map(|pane| pane.hit(sky))),
        Some(Chrome::Resize(ResizeEdge::Top)),
        "nothing is drawn under the halo, so the halo has it"
    );
    assert_eq!(two_pass(&stack, sky), Some(Chrome::Resize(ResizeEdge::Top)));
    assert_eq!(
        halo_wins(&stack, sky),
        Some(Chrome::Resize(ResizeEdge::Top))
    );

    // And the half of that border that lies *inside* its own window is
    // chrome outright: the pane draws the pixel it is claiming, so it wins
    // over everything below and is never downgraded to a halo. A rule that
    // answered `Client` wherever the drawn rect contained the point would
    // have swallowed it and left every window resizable only from outside.
    let inside = (300.0, 355.0);
    let alone = Stacked::window((60, 60), (400, 300));
    assert!(
        alone
            .outer
            .to_f64()
            .contains(Point::<f64, Logical>::from(inside))
    );
    assert_eq!(
        alone.hit(inside),
        PaneHit::Chrome(Chrome::Resize(ResizeEdge::Bottom))
    );
    assert_eq!(
        topmost_chrome([alone, lower].iter().map(|pane| pane.hit(inside))),
        Some(Chrome::Resize(ResizeEdge::Bottom)),
        "and it beats the lower window it is drawn over, which is the half \
         of #111's fix that was right"
    );
}

/// **A halo over two panes takes the topmost one's, and a lower pane's
/// border does not reach up through a window covering it.**
///
/// The tie-break the corrected rule needs and the covered case it must not
/// lose. Remembering a halo and carrying on is only safe if the *first* one
/// is kept: two stacked windows whose edges both hang over the same strip
/// of desktop are an ordinary sight, and the answer there is still the one
/// on top.
#[test]
fn the_topmost_halo_is_the_one_kept() {
    // Two windows 6px apart with a strip of desktop between them: the
    // upper's bottom border and the lower's top border both hang over it,
    // and they name opposite edges, so which one the walk keeps is
    // legible in the answer.
    let upper = Stacked::window((100, 100), (200, 100));
    let lower = Stacked::window((100, 206), (200, 100));
    let below = (200.0, 202.0);

    assert_eq!(
        upper.hit(below),
        PaneHit::Halo(Chrome::Resize(ResizeEdge::Bottom)),
        "2px below the upper window"
    );
    assert_eq!(
        lower.hit(below),
        PaneHit::Halo(Chrome::Resize(ResizeEdge::Top)),
        "and 4px above the lower one"
    );
    assert_eq!(
        topmost_chrome([upper, lower].iter().map(|pane| pane.hit(below))),
        Some(Chrome::Resize(ResizeEdge::Bottom)),
        "two halos over the same desktop, and the topmost is the one kept"
    );
    assert_eq!(
        topmost_chrome([lower, upper].iter().map(|pane| pane.hit(below))),
        Some(Chrome::Resize(ResizeEdge::Top)),
        "stack them the other way and the answer follows the stacking order"
    );

    // A lower window's border, under a window that covers where it hangs.
    // The border is the lower window's own and the point is still the upper
    // window's client: a halo is a claim on the desktop, not a tunnel.
    let over = Stacked::window((60, 150), (400, 200));
    assert_eq!(over.hit(below), PaneHit::Client);
    assert_eq!(
        topmost_chrome([over, upper].iter().map(|pane| pane.hit(below))),
        None,
        "the covering window's client owns it, and the border reaching up \
         from underneath does not"
    );
}

/// **An unmanaged pane occludes and offers nothing.**
///
/// An X11 menu, tooltip or dropdown is placed by its client and owned by
/// it: `size_window` refuses an override-redirect surface, and nothing in
/// the layout moves one. So a resize border eight pixels outside a Steam or
/// GTK menu is a cursor promising a drag that cannot happen, and a press
/// there starts a `ResizeGrab` that does nothing when it should have
/// dismissed the menu. The single-pass walk made that worse before this
/// gate: the phantom border beat a lower window's real titlebar.
///
/// Covering is untouched, which is the half that was always right — the
/// menu is drawn and a press on it is the menu's.
#[test]
fn an_unmanaged_pane_occludes_but_offers_no_chrome() {
    let lower = Stacked::window((100, 100), (400, 300));
    let menu = Stacked::window((300, 20), (200, 100)).unmanaged();
    let stack = [menu, lower];

    // The same point as the halo case above: 4px below the menu's bottom
    // edge, over the lower window's titlebar.
    let button = (470.0, 124.0);
    assert_eq!(
        menu.hit(button),
        PaneHit::Miss,
        "a menu has no resize border to offer, inside or out"
    );
    assert_eq!(
        topmost_chrome(stack.iter().map(|pane| pane.hit(button))),
        Some(Chrome::Frame),
        "so the titlebar under it is pressable, phantom border or not"
    );

    // And on the menu itself: covered, and the press is the client's.
    let on_menu = (400.0, 60.0);
    assert_eq!(menu.hit(on_menu), PaneHit::Client);
    assert_eq!(
        topmost_chrome(stack.iter().map(|pane| pane.hit(on_menu))),
        None,
        "a press on a menu is the menu's, which is what occluding means"
    );

    // The inside half of the border it would otherwise have offered is the
    // client's too, rather than a resize of something nothing may resize.
    let edge = (400.0, 116.0);
    assert_eq!(
        Stacked::window((300, 20), (200, 100)).hit(edge),
        PaneHit::Chrome(Chrome::Resize(ResizeEdge::Bottom)),
        "a managed window of the same shape would offer this"
    );
    assert_eq!(menu.hit(edge), PaneHit::Client);
}

/// **An invisible pane offers nothing and covers nothing** — the one kind
/// that fails both of [`pane_hit_of`]'s questions at once.
///
/// Issue #127's review finding 1 at the level of the rule rather than of
/// the compositor. A pane between its close animation landing and its
/// client acting on the request is held at opacity zero by a transform
/// written `release: false`, for `CLOSING` plus the whole grace period. It
/// is still in the stack and its rectangle still contains the point; it is
/// simply not on screen.
///
/// **The `Halo` is what makes this two gates and not one.** Suppressing
/// only `covers` would turn the chrome this pane claims into
/// [`PaneHit::Halo`] — the weakest claim, but a claim that survives the
/// walk and wins wherever nothing lower paints. A closed window's titlebar
/// and resize border would go on being pressable over bare desktop, which
/// is a worse bug than the one being fixed because there is nothing on
/// screen to explain it. Both questions are therefore asked, and every
/// point is a `Miss`.
///
/// Contrast [`an_unmanaged_pane_occludes_but_offers_no_chrome`]: a menu
/// offers no chrome *and still covers*, because it is drawn. That is the
/// distinction — offering is about what a press would mean, covering is
/// about pixels, and this pane has no pixels.
#[test]
fn an_invisible_pane_offers_no_chrome_and_covers_nothing() {
    let lower = Stacked::window((100, 100), (400, 300));
    // Directly over the lower window, which is the situation a close
    // leaves behind once the layout has reflowed into the space.
    let closed = Stacked::window((100, 100), (400, 300)).invisible();
    let stack = [closed, lower];

    // On the lower window's titlebar, which the invisible pane's own
    // titlebar sits exactly on top of.
    let titlebar = (300.0, 110.0);
    assert_eq!(
        Stacked::window((100, 100), (400, 300)).hit(titlebar),
        PaneHit::Chrome(Chrome::Frame),
        "a visible pane of the same shape owns this point -- without which \
         the assertions below pass for want of a titlebar rather than for \
         want of a pane"
    );
    assert_eq!(
        closed.hit(titlebar),
        PaneHit::Miss,
        "an invisible pane's titlebar is not pressable"
    );
    assert_eq!(
        topmost_chrome(stack.iter().map(|pane| pane.hit(titlebar))),
        Some(Chrome::Frame),
        "so the press reaches the titlebar that is actually drawn there"
    );

    // The body, which is the click-to-focus case and the one that took the
    // keyboard with it.
    let body = (300.0, 250.0);
    assert_eq!(
        closed.hit(body),
        PaneHit::Miss,
        "and it occludes nothing, so the walk descends rather than \
         stopping at `PaneHit::Client`"
    );

    // The resize border, inside and out. Outside is the `Halo` this gate
    // exists to prevent.
    let inside = (300.0, 396.0);
    let outside = (300.0, 404.0);
    assert_eq!(
        Stacked::window((100, 100), (400, 300)).hit(outside),
        PaneHit::Halo(Chrome::Resize(ResizeEdge::Bottom)),
        "a visible pane of the same shape claims this as a halo, which is \
         the claim that must not survive being invisible"
    );
    assert_eq!(closed.hit(inside), PaneHit::Miss);
    assert_eq!(
        closed.hit(outside),
        PaneHit::Miss,
        "an edge nobody can see is an edge nobody can drag, and a halo \
         would have been honoured over bare desktop"
    );
}

/// A pane whose application has not arrived offers its frame and not its
/// border, and covers either way.
///
/// The `window.is_some()` half of [`chrome_offered`]: a frame around a
/// loading window has working buttons, which is the point of drawing one,
/// and there is nothing yet for a resize to resize. What it must not do is
/// let the window behind it take the press, which is issue #111 again with
/// a different pane on top.
#[test]
fn a_loading_pane_offers_its_frame_and_not_its_border() {
    let lower = Stacked::window((100, 100), (400, 300));
    let loading = Stacked::window((300, 20), (200, 100)).loading();
    let stack = [loading, lower];

    assert_eq!(
        loading.hit((400.0, 30.0)),
        PaneHit::Chrome(Chrome::Frame),
        "its titlebar is drawn and its buttons work"
    );
    assert_eq!(
        loading.hit((400.0, 116.0)),
        PaneHit::Client,
        "and its bottom border is not offered, so the point is simply \
         inside what it draws"
    );
    assert_eq!(
        loading.hit((470.0, 124.0)),
        PaneHit::Miss,
        "nor is the half of that border outside it"
    );
    assert_eq!(
        topmost_chrome(stack.iter().map(|pane| pane.hit((400.0, 80.0)))),
        None,
        "a press on a loading window's body is its own, not the window \
         behind it"
    );
}

/// **The cursor is composed from the same walk, so an occluded border does
/// not promise a resize.**
///
/// #108's finding one altitude up: the pointer's shape is read off
/// [`claim_of`] rather than off any second hit test, so whatever the walk
/// declines to claim is a point the compositor says nothing about. Without
/// this the fix would be half-applied — presses landing correctly while the
/// pointer went on describing the resize that the press no longer performs,
/// which is exactly the symptom #108 was filed for.
#[test]
fn an_occluded_border_promises_no_resize_cursor() {
    let lower = Stacked::window((100, 100), (400, 300));
    let upper = Stacked::window((300, 20), (200, 100));
    let stack = [upper, lower];
    let cursor_at = |point| {
        claim_of(
            false,
            false,
            topmost_chrome(stack.iter().map(|pane| pane.hit(point))),
        )
        .cursor()
    };

    // Over the lower window's close button, under the upper window's halo:
    // the frame's own arrow, and not the resize the halo would have asked
    // for.
    assert_eq!(cursor_at((470.0, 124.0)), Some(CursorIcon::Default));
    assert_eq!(
        claim_of(false, false, halo_wins(&stack, (470.0, 124.0))).cursor(),
        Some(CursorIcon::NsResize),
        "which is the pointer #111's first fix drew over that button"
    );

    // Over the desktop above the upper window, where the halo is the whole
    // claim: the resize cursor, because the press really would resize.
    assert_eq!(cursor_at((400.0, 16.0)), Some(CursorIcon::NsResize));

    // And on the upper window's client, over the lower window's titlebar:
    // nothing at all, so the client's own cursor stands. This is #111's
    // point and the reason `chrome_under` is the only hit test.
    let covered = Stacked::window((60, 60), (400, 300));
    assert_eq!(
        claim_of(
            false,
            false,
            topmost_chrome([covered, lower].iter().map(|pane| pane.hit((300.0, 116.0)))),
        )
        .cursor(),
        None
    );
}

/// A point no window covers still descends, which is the case the fix must
/// not break: [`PaneHit::Miss`] and [`PaneHit::Client`] are both "no chrome
/// here" and only one of them may stop the walk.
#[test]
fn a_point_nothing_covers_still_descends() {
    let lower = Stacked::window((100, 100), (400, 300));
    let upper = Stacked::window((150, 20), (200, 100));
    let stack = [upper, lower];

    // 92px to the right of the upper window -- well past its border -- and
    // 10px down into the lower window's titlebar.
    let past = (450.0, 110.0);
    assert_eq!(upper.hit(past), PaneHit::Miss);
    assert_eq!(
        topmost_chrome(stack.iter().map(|pane| pane.hit(past))),
        Some(Chrome::Frame),
        "a window that is not there covers nothing, and the titlebar \
         beside it is still pressable"
    );
    assert_eq!(
        two_pass(&stack, past),
        Some(Chrome::Frame),
        "which the rule this replaced also got right -- only the covered \
         case differs"
    );

    // Bare desktop: every pane misses and the walk runs out.
    let desktop = (700.0, 700.0);
    assert_eq!(upper.hit(desktop), PaneHit::Miss);
    assert_eq!(lower.hit(desktop), PaneHit::Miss);
    assert_eq!(
        topmost_chrome(stack.iter().map(|pane| pane.hit(desktop))),
        None
    );
    assert_eq!(
        topmost_chrome(std::iter::empty::<PaneHit<Chrome>>()),
        None,
        "and a desktop with no windows on it at all"
    );
}

/// The tests that need a client on the other end of a socket.
///
/// A bare `WlSurface` is not enough for any of them: `Window` only wraps a
/// real `ToplevelSurface`, and Smithay gives no way to fabricate one except
/// a client asking for it over the wire. Anything that is *state a client
/// sets* -- the scale it was told, the parent it named, the modal flag it
/// raised -- can only be reached from this side. `wl-probe` (see its own
/// `Cargo.toml`) exists in this workspace for the identical reason, and its
/// dependencies are what make this affordable here: `wayland-client` and
/// `wayland-protocols`'s `client` feature were already in the lockfile.
///
/// The module was one test's and was named after it. It is two now, and the
/// fixture was always the expensive part of it.
mod real_client {
    use super::*;
    use smithay::output::{Mode, PhysicalProperties, Subpixel};
    use smithay::reexports::wayland_server::Display;
    use std::os::unix::io::{AsFd, OwnedFd};
    use std::os::unix::net::UnixStream;
    use wayland_client::protocol::{
        wl_buffer, wl_callback, wl_compositor, wl_data_device, wl_data_device_manager,
        wl_data_offer, wl_keyboard, wl_output, wl_registry, wl_seat, wl_shm, wl_shm_pool,
        wl_subcompositor, wl_subsurface, wl_surface,
    };
    use wayland_client::{Connection, Dispatch, QueueHandle};
    use wayland_protocols::ext::session_lock::v1::client::{
        ext_session_lock_manager_v1, ext_session_lock_surface_v1, ext_session_lock_v1,
    };
    use wayland_protocols::xdg::activation::v1::client::{
        xdg_activation_token_v1, xdg_activation_v1,
    };
    use wayland_protocols::xdg::dialog::v1::client::{xdg_dialog_v1, xdg_wm_dialog_v1};
    use wayland_protocols::xdg::shell::client::{
        xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base,
    };
    use wayland_protocols_wlr::layer_shell::v1::client::{
        zwlr_layer_shell_v1, zwlr_layer_surface_v1,
    };
    use wayland_protocols_wlr::screencopy::v1::client::{
        zwlr_screencopy_frame_v1, zwlr_screencopy_manager_v1,
    };

    /// The `edge_at` a `ResizeGrab` would record on the frame it produced
    /// `wanted`, for a pane the layout has at that same rectangle.
    ///
    /// These fixtures build a `ResizeRequest` by hand, because what is under
    /// test is what `settle_resize` does with one rather than how a grab
    /// assembles one. It still has to be a payload a grab could *have*
    /// assembled. Spelled `(wanted.loc.x, wanted.loc.y)` it was not: that is
    /// the top-left corner whatever the edges say, so every `Right`,
    /// `Bottom` and `BottomRight` case carried a pair
    /// `crate::input::resize::dragged_edge` cannot produce. Nothing read it
    /// -- every one of these takes the floating path, where no layout is
    /// offered the drag at all -- which is exactly why it would have sat
    /// there until a tiled test was written against it and believed.
    ///
    /// Built by the real function for that reason, rather than by a second
    /// copy of its rules here.
    fn payload_edge(wanted: Rectangle<i32, Logical>, edges: ResizeEdge) -> (f64, f64) {
        // The pointer, for an axis this drag has no hold of. Its centre is
        // as good as anywhere: the value is passed straight through and
        // these fixtures never look at it.
        let pointer: Point<f64, Logical> = (
            f64::from(wanted.loc.x) + f64::from(wanted.size.w) / 2.0,
            f64::from(wanted.loc.y) + f64::from(wanted.size.h) / 2.0,
        )
            .into();
        crate::input::resize::dragged_edge(
            crate::input::resize::LaidOut(wanted),
            edges,
            pointer,
            pointer,
        )
    }

    /// The client side of the fixture. Binds exactly the globals a window
    /// needs and nothing else -- there is no renderer on this end to answer
    /// anything more, and none of what follows needs one.
    #[derive(Debug, Default)]
    struct Client {
        compositor: Option<wl_compositor::WlCompositor>,
        wm_base: Option<xdg_wm_base::XdgWmBase>,
        shm: Option<wl_shm::WlShm>,
        /// For #126: a window whose picture is more than its root surface.
        subcompositor: Option<wl_subcompositor::WlSubcompositor>,
        /// Every `wl_buffer.release` this client has been sent, by buffer.
        /// For #126, whose fade draws from a buffer the client must not be
        /// told it may reuse until the fade is over.
        released: Vec<wayland_client::backend::ObjectId>,
        /// The global #72 added. Bound here because "the client said modal"
        /// is not a thing the server side can say on a client's behalf --
        /// which is the whole reason this test is in this module.
        dialogs: Option<xdg_wm_dialog_v1::XdgWmDialogV1>,
        /// For a window that asks to be brought forward with a token, as
        /// an application launched with one does as it opens -- GTK, Qt
        /// and winit all do. A raw client that never sends it cannot see
        /// what the compositor does with the request (#134's second
        /// review, finding 2), so `keyboard_at_open` sends it.
        activation: Option<xdg_activation_v1::XdgActivationV1>,
        /// For [`bar`]: the one way to make a monitor's work area smaller
        /// than the monitor, which is what tells a maximised window from a
        /// fullscreen one.
        layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
        /// Every `xdg_toplevel.configure` this client has been sent, with
        /// the toplevel it was sent to.
        ///
        /// The *rate* a client is configured at is the subject of #123 and
        /// there is no way to see it from the server side: `size_window`
        /// hands a size to Smithay and Smithay decides whether that is a
        /// change worth a wire message. Counting them here counts what the
        /// client actually receives, which is the only number the issue is
        /// about — and it is also what keeps a test honest, because a
        /// compositor that sends the same size sixty times a second looks
        /// identical from its own side and costs nothing on the wire.
        configures: Vec<(wayland_client::backend::ObjectId, i32, i32)>,

        /// The monitor, the lock manager and the clipboard, for
        /// `lock_focus`: the only tests that need a surface to lock a
        /// screen with, or a data device. Bound by every client and used by
        /// none of the others, which is harmless -- binding a global asks
        /// for nothing until a request is made on it. The seat, which
        /// `lock_focus` needs as well, is with the keyboard below.
        output: Option<wl_output::WlOutput>,
        /// Every monitor, where `output` is the last one bound: a lock
        /// client covers them all.
        outputs: Vec<wl_output::WlOutput>,
        locks: Option<ext_session_lock_manager_v1::ExtSessionLockManagerV1>,
        data_devices: Option<wl_data_device_manager::WlDataDeviceManager>,
        screencopy: Option<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1>,
        /// Every `wl_keyboard.key` this client has been sent, as evdev
        /// codes, presses and releases alike. What the lock tests are
        /// about in the end: a key that reaches an application's client
        /// is a key the application has.
        keys: Vec<u32>,
        /// The same keys with whether each was a press, for the one
        /// question `keys` cannot answer: whether a key the client was
        /// told went down was ever told it came up.
        key_events: Vec<(u32, bool)>,
        /// The keys the last `wl_keyboard.enter` said were already held.
        enter_keys: Vec<u32>,
        /// The lock objects this client was told `locked` on, and the
        /// ones it was told `finished` on.
        locked: Vec<wayland_client::backend::ObjectId>,
        finished: Vec<wayland_client::backend::ObjectId>,
        /// Screen captures this client was offered a buffer for, and ones
        /// it was told failed.
        captures_offered: usize,
        captures_failed: usize,
        /// The surface this client's keyboard is on, as its own
        /// `enter`/`leave` events say -- the client's view, which is the
        /// one that matters, and not the server's.
        keyboard_on: Option<wayland_client::backend::ObjectId>,
        /// The serial of the last `wl_keyboard.enter`, for a menu to grab
        /// with the way a real one does.
        serial: u32,
        /// How many `wl_data_device.selection` events have arrived. One is
        /// sent each time this client is made the clipboard's client, so a
        /// count that moves while the session is locked is an application
        /// behind the lock being handed the clipboard.
        selections: usize,
        /// How many `xdg_popup.popup_done` events have arrived.
        popups_done: usize,
        /// Every `xdg_surface.configure` serial, with the `xdg_surface` it
        /// was sent to. Recorded and not acked, for the reason
        /// [`Self::configures`] gives -- and so a test that needs a popup
        /// to draw can ack one itself, which xdg-shell requires before a
        /// popup may commit anything.
        surface_configures: Vec<(wayland_client::backend::ObjectId, u32)>,
        /// Every `xdg_toplevel.close` this client has been sent, with the
        /// toplevel it was sent to.
        ///
        /// Counted here for the same reason `configures` is: `send_close`
        /// is a call the server makes into Smithay, and from the server's
        /// own side a compositor that asks twice looks exactly like one
        /// that asks once. Issue #127's second fault is precisely a second
        /// request going out, so the only honest place to count is the end
        /// that receives them.
        closes: Vec<wayland_client::backend::ObjectId>,
        /// The seat and its keyboard, bound so that a test can ask where
        /// typing *went* rather than where focus was set.
        ///
        /// The distinction is the whole of #127's second review, finding 1:
        /// a compositor that leaves focus on a window nobody can see is
        /// indistinguishable, from its own side, from one that moved it —
        /// the seat is perfectly happy to hold a surface that is drawn at
        /// opacity zero, and `give_keyboard`'s `true` says only that the
        /// lock's rule allowed it, not that anyone can see the surface. The
        /// end that receives the keystrokes is the only one that can say,
        /// which is why `typing_after_a_close_reaches_the_window_that_is_drawn`
        /// asserts here and not at the seat.
        ///
        /// The keyboard is the one the seat's handler asks for, and the
        /// only one: see that handler.
        seat: Option<wl_seat::WlSeat>,
        keyboard: Option<wl_keyboard::WlKeyboard>,
        /// The surface this client currently has keyboard focus on, as
        /// `wl_keyboard.enter` and `.leave` report it.
        ///
        /// Kept as the protocol id rather than the proxy, because that is
        /// the one number both ends of this fixture agree on: a
        /// `wl_surface` is created by the client and carries the same id in
        /// the server's object map, so a test holding only the server's
        /// `Window` can still say which surface this was.
        entered: Option<u32>,
        /// Every key this client was sent, paired with the surface it was
        /// focused on when it arrived.
        ///
        /// The pairing is the point. `wl_keyboard.key` carries no surface —
        /// it goes wherever the last `enter` put the focus — so "which
        /// window did this character reach" is a question only a client
        /// tracking both events can answer, and it is exactly the question
        /// the user asks when they close a window and keep typing.
        typed: Vec<(Option<u32>, u32)>,
        /// Every activation token this client asked for and was handed,
        /// by `xdg_activation_token_v1.done`, oldest first.
        tokens: Vec<String>,
    }

    /// See [`Client::tokens`].
    impl Dispatch<xdg_activation_token_v1::XdgActivationTokenV1, ()> for Client {
        fn event(
            state: &mut Self,
            _token: &xdg_activation_token_v1::XdgActivationTokenV1,
            event: xdg_activation_token_v1::Event,
            (): &(),
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
        ) {
            if let xdg_activation_token_v1::Event::Done { token } = event {
                state.tokens.push(token);
            }
        }
    }

    impl Dispatch<wl_registry::WlRegistry, ()> for Client {
        fn event(
            state: &mut Self,
            registry: &wl_registry::WlRegistry,
            event: wl_registry::Event,
            (): &(),
            _conn: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            let wl_registry::Event::Global {
                name, interface, ..
            } = event
            else {
                return;
            };
            match interface.as_str() {
                "wl_compositor" => state.compositor = Some(registry.bind(name, 1, qh, ())),
                "xdg_wm_base" => state.wm_base = Some(registry.bind(name, 1, qh, ())),
                "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
                "wl_subcompositor" => {
                    state.subcompositor = Some(registry.bind(name, 1, qh, ()));
                }
                "xdg_wm_dialog_v1" => state.dialogs = Some(registry.bind(name, 1, qh, ())),
                "xdg_activation_v1" => {
                    state.activation = Some(registry.bind(name, 1, qh, ()));
                }
                // Version 5 rather than the newest: everything below needs
                // `enter`, `leave` and `key`, which are version 1, and the
                // lower the bind the fewer ways this fixture can stop
                // matching the compositor's advertised version.
                "wl_seat" => state.seat = Some(registry.bind(name, 5, qh, ())),
                "wl_output" => {
                    let output: wl_output::WlOutput = registry.bind(name, 1, qh, ());
                    state.outputs.push(output.clone());
                    state.output = Some(output);
                }
                "zwlr_screencopy_manager_v1" => {
                    state.screencopy = Some(registry.bind(name, 3, qh, ()));
                }
                "ext_session_lock_manager_v1" => {
                    state.locks = Some(registry.bind(name, 1, qh, ()));
                }
                "wl_data_device_manager" => {
                    state.data_devices = Some(registry.bind(name, 1, qh, ()));
                }
                "zwlr_layer_shell_v1" => {
                    state.layer_shell = Some(registry.bind(name, 1, qh, ()));
                }
                _ => {}
            }
        }
    }

    /// The keyboard is taken as soon as the seat says it has one.
    ///
    /// Here rather than at a call site, because capabilities arrive
    /// asynchronously: a test that asked for the keyboard at the moment it
    /// wanted to type would be asking before the registry round trip that
    /// `connect` ends with had delivered this event.
    ///
    /// **Here and nowhere else**, so that a client has one keyboard. A
    /// second one -- `lock_focus`'s `Side::connect` asked for its own
    /// before the two suites met -- is sent every event again, and a
    /// client with two records every key twice.
    /// `a_mode_active_at_the_lock_does_not_garble_the_password` asserts
    /// the lock client's exact key sequence, and fails on the doubling.
    impl Dispatch<wl_seat::WlSeat, ()> for Client {
        fn event(
            state: &mut Self,
            seat: &wl_seat::WlSeat,
            event: wl_seat::Event,
            (): &(),
            _conn: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            let wl_seat::Event::Capabilities { capabilities } = event else {
                return;
            };
            let has_keyboard = capabilities
                .into_result()
                .is_ok_and(|capabilities| capabilities.contains(wl_seat::Capability::Keyboard));
            if has_keyboard && state.keyboard.is_none() {
                state.keyboard = Some(seat.get_keyboard(qh, ()));
            }
        }
    }

    /// Where the typing went, as both suites ask it: see [`Client::keys`]
    /// and [`Client::keyboard_on`] for `lock_focus`, and [`Client::typed`]
    /// for #127's.
    ///
    /// The keymap fd is dropped rather than read: nothing here interprets
    /// keysyms, and the assertion is about which surface received a key
    /// rather than which character it was.
    impl Dispatch<wl_keyboard::WlKeyboard, ()> for Client {
        fn event(
            state: &mut Self,
            _keyboard: &wl_keyboard::WlKeyboard,
            event: wl_keyboard::Event,
            (): &(),
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
        ) {
            match event {
                wl_keyboard::Event::Enter {
                    serial,
                    surface,
                    keys,
                } => {
                    let id = wayland_client::Proxy::id(&surface);
                    state.entered = Some(id.protocol_id());
                    state.keyboard_on = Some(id);
                    state.serial = serial;
                    state.enter_keys = keys
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|key| u32::from_ne_bytes(*key))
                        .collect();
                }
                wl_keyboard::Event::Leave { .. } => {
                    state.entered = None;
                    state.keyboard_on = None;
                }
                wl_keyboard::Event::Key {
                    key,
                    state: key_state,
                    ..
                } => {
                    state.keys.push(key);
                    state.key_events.push((
                        key,
                        key_state == wayland_client::WEnum::Value(wl_keyboard::KeyState::Pressed),
                    ));
                    state.typed.push((state.entered, key));
                }
                _ => {}
            }
        }
    }

    wayland_client::delegate_noop!(Client: ignore xdg_wm_dialog_v1::XdgWmDialogV1);
    wayland_client::delegate_noop!(Client: ignore xdg_dialog_v1::XdgDialogV1);
    wayland_client::delegate_noop!(Client: ignore xdg_activation_v1::XdgActivationV1);
    wayland_client::delegate_noop!(Client: ignore wl_compositor::WlCompositor);
    wayland_client::delegate_noop!(Client: ignore wl_surface::WlSurface);
    wayland_client::delegate_noop!(Client: ignore wl_shm::WlShm);
    wayland_client::delegate_noop!(Client: ignore wl_shm_pool::WlShmPool);
    wayland_client::delegate_noop!(Client: ignore xdg_wm_base::XdgWmBase);
    wayland_client::delegate_noop!(Client: ignore wl_subcompositor::WlSubcompositor);
    wayland_client::delegate_noop!(Client: ignore wl_subsurface::WlSubsurface);

    /// See [`Client::released`].
    impl Dispatch<wl_buffer::WlBuffer, ()> for Client {
        fn event(
            state: &mut Self,
            buffer: &wl_buffer::WlBuffer,
            event: wl_buffer::Event,
            _data: &(),
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
        ) {
            if matches!(event, wl_buffer::Event::Release) {
                state.released.push(wayland_client::Proxy::id(buffer));
            }
        }
    }

    /// See [`Client::surface_configures`].
    impl Dispatch<xdg_surface::XdgSurface, ()> for Client {
        fn event(
            state: &mut Self,
            surface: &xdg_surface::XdgSurface,
            event: xdg_surface::Event,
            _data: &(),
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
        ) {
            if let xdg_surface::Event::Configure { serial } = event {
                state
                    .surface_configures
                    .push((wayland_client::Proxy::id(surface), serial));
            }
        }
    }
    wayland_client::delegate_noop!(Client: ignore wl_callback::WlCallback);
    wayland_client::delegate_noop!(Client: ignore wl_output::WlOutput);
    wayland_client::delegate_noop!(Client: ignore wl_data_device_manager::WlDataDeviceManager);
    wayland_client::delegate_noop!(Client: ignore wl_data_offer::WlDataOffer);
    wayland_client::delegate_noop!(Client: ignore xdg_positioner::XdgPositioner);
    wayland_client::delegate_noop!(
        Client: ignore ext_session_lock_manager_v1::ExtSessionLockManagerV1
    );
    wayland_client::delegate_noop!(
        Client: ignore zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1
    );

    /// See [`Client::locked`] and [`Client::finished`].
    impl Dispatch<ext_session_lock_v1::ExtSessionLockV1, ()> for Client {
        fn event(
            state: &mut Self,
            lock: &ext_session_lock_v1::ExtSessionLockV1,
            event: ext_session_lock_v1::Event,
            _data: &(),
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
        ) {
            let id = wayland_client::Proxy::id(lock);
            match event {
                ext_session_lock_v1::Event::Locked => state.locked.push(id),
                ext_session_lock_v1::Event::Finished => state.finished.push(id),
                _ => {}
            }
        }
    }

    /// See [`Client::captures_offered`].
    impl Dispatch<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, ()> for Client {
        fn event(
            state: &mut Self,
            _frame: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
            event: zwlr_screencopy_frame_v1::Event,
            _data: &(),
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
        ) {
            match event {
                zwlr_screencopy_frame_v1::Event::Buffer { .. } => state.captures_offered += 1,
                zwlr_screencopy_frame_v1::Event::Failed => state.captures_failed += 1,
                _ => {}
            }
        }
    }

    wayland_client::delegate_noop!(
        Client: ignore ext_session_lock_surface_v1::ExtSessionLockSurfaceV1
    );

    /// See [`Client::selections`].
    impl Dispatch<wl_data_device::WlDataDevice, ()> for Client {
        fn event(
            state: &mut Self,
            _device: &wl_data_device::WlDataDevice,
            event: wl_data_device::Event,
            _data: &(),
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
        ) {
            if let wl_data_device::Event::Selection { .. } = event {
                state.selections += 1;
            }
        }

        // An offer is a new object the server creates, so the client has to
        // be told what to make of one even though no test sets a selection
        // and none should ever arrive.
        wayland_client::event_created_child!(Client, wl_data_device::WlDataDevice, [
            wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, ()),
        ]);
    }

    /// See [`Client::popups_done`].
    impl Dispatch<xdg_popup::XdgPopup, ()> for Client {
        fn event(
            state: &mut Self,
            _popup: &xdg_popup::XdgPopup,
            event: xdg_popup::Event,
            _data: &(),
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
        ) {
            if let xdg_popup::Event::PopupDone = event {
                state.popups_done += 1;
            }
        }
    }
    wayland_client::delegate_noop!(Client: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
    wayland_client::delegate_noop!(Client: ignore zwlr_layer_surface_v1::ZwlrLayerSurfaceV1);

    /// The two events this fixture does not ignore. See
    /// [`Client::configures`] and [`Client::closes`].
    ///
    /// Configures are deliberately not acked:
    /// `ToplevelSurface::send_pending_configure` compares against the last
    /// configure it *sent*, not the last one a client acknowledged, so a
    /// fixture that never acks still sees exactly the deduplication a real
    /// client would.
    ///
    /// A close is deliberately not acted on either, and that is the whole
    /// of what makes this client a stand-in for a slow one. `close` is a
    /// request with no reply in the protocol; a client honours it by
    /// destroying its toplevel, and one that has not done so *yet* is
    /// indistinguishable from one that has decided not to. This fixture
    /// never destroys anything, so it is both — which is what issue #127's
    /// third fault is about.
    impl Dispatch<xdg_toplevel::XdgToplevel, ()> for Client {
        fn event(
            state: &mut Self,
            toplevel: &xdg_toplevel::XdgToplevel,
            event: xdg_toplevel::Event,
            _data: &(),
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
        ) {
            match event {
                xdg_toplevel::Event::Configure { width, height, .. } => {
                    state
                        .configures
                        .push((wayland_client::Proxy::id(toplevel), width, height))
                }
                xdg_toplevel::Event::Close => {
                    state.closes.push(wayland_client::Proxy::id(toplevel));
                }
                _ => {}
            }
        }
    }

    /// An anonymous, already-unlinked file of `size` bytes -- enough for a
    /// client to back a `wl_shm_pool` with. Its contents are never read:
    /// nothing in this fixture renders. Only its *size* matters, because
    /// that is what gives the mapped `Window` a real, non-zero bounding
    /// box instead of the `Rectangle::zero()` an uncommitted surface has,
    /// which overlaps no output at all and so would never appear in
    /// `elements_for_output` for either monitor below.
    fn anon_file(size: i32) -> OwnedFd {
        let path = std::env::temp_dir().join(format!(
            "solium-scale-resend-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .expect("creating a backing file for a test wl_shm_pool");
        std::fs::remove_file(&path).expect("unlinking the test shm file");
        file.set_len(u64::from(size.unsigned_abs()))
            .expect("sizing the test shm file");
        file.into()
    }

    /// A client on the other end of a socketpair, with every global this
    /// compositor has already in its registry.
    ///
    /// The queue comes back rather than a handle to it, because a
    /// `QueueHandle` borrows its queue and the caller has to own one.
    fn connect(
        display: &mut Display<Solium>,
        state: &mut Solium,
    ) -> (Connection, wayland_client::EventQueue<Client>, Client) {
        let (server_side, client_side) = UnixStream::pair().expect("a socketpair");
        display
            .handle()
            .insert_client(server_side, std::sync::Arc::new(ClientState::default()))
            .expect("inserting the test client");
        let conn = Connection::from_socket(client_side).expect("wrapping the client socket");
        let mut event_queue = conn.new_event_queue::<Client>();
        let qh = event_queue.handle();
        let mut client = Client::default();

        conn.display().get_registry(&qh, ());
        conn.flush().expect("flushing get_registry");
        display
            .dispatch_clients(state)
            .expect("dispatching get_registry");
        display
            .flush_clients()
            .expect("flushing the registry snapshot");
        // The one blocking read in this fixture: it is safe because the
        // server has, on the line above, already written the response --
        // every global this compositor has -- onto the socket. Blocking
        // here waits on bytes that are already in the kernel buffer, not
        // on the server, which nothing is driving but this same thread.
        event_queue
            .blocking_dispatch(&mut client)
            .expect("reading the registry snapshot");

        (conn, event_queue, client)
    }

    /// Opens one window through the real protocol: a surface, an
    /// `xdg_toplevel`, and a tiny committed buffer, so `new_toplevel` maps
    /// a `Window` with a real, non-zero bounding box at `(0, 0)` -- where
    /// every window is first mapped; the caller repositions it from
    /// there. Returns the newly-mapped `Window`.
    ///
    /// The client's own `xdg_toplevel` comes back with it, because some of
    /// what a window is only exists on that side: `set_parent` and the modal
    /// flag are both requests, and there is no way to ask for them except as
    /// the client.
    fn open_window(
        display: &mut Display<Solium>,
        state: &mut Solium,
        conn: &Connection,
        client: &Client,
        qh: &QueueHandle<Client>,
    ) -> (Window, xdg_toplevel::XdgToplevel) {
        let (window, toplevel, _surface) = open_surface(display, state, conn, client, qh);
        (window, toplevel)
    }

    /// The same, handing back the `wl_surface` as well.
    ///
    /// For the one thing no other fixture has needed: making the client
    /// *answer* a configure with a size of its own. A window's size is its
    /// surface's committed bounding box, so the only way to change it is to
    /// attach another buffer from this side — which is exactly what a client
    /// refusing a resize does, and refusal is the trap `crate::resizing` is
    /// built around. Dropping a `wayland-client` proxy sends nothing, so the
    /// callers that do not want it are unaffected.
    fn open_surface(
        display: &mut Display<Solium>,
        state: &mut Solium,
        conn: &Connection,
        client: &Client,
        qh: &QueueHandle<Client>,
    ) -> (Window, xdg_toplevel::XdgToplevel, wl_surface::WlSurface) {
        let (window, toplevel, surface, _xdg_surface) = open_xdg(display, state, conn, client, qh);
        (window, toplevel, surface)
    }

    /// The same again, with the `xdg_surface` too: a menu names its parent
    /// by that, and there is no other way to open one.
    fn open_xdg(
        display: &mut Display<Solium>,
        state: &mut Solium,
        conn: &Connection,
        client: &Client,
        qh: &QueueHandle<Client>,
    ) -> (
        Window,
        xdg_toplevel::XdgToplevel,
        wl_surface::WlSurface,
        xdg_surface::XdgSurface,
    ) {
        let compositor = client.compositor.clone().expect("wl_compositor bound");
        let wm_base = client.wm_base.clone().expect("xdg_wm_base bound");

        let before: Vec<Window> = state.space.elements().cloned().collect();

        let surface = compositor.create_surface(qh, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, qh, ());
        let toplevel = xdg_surface.get_toplevel(qh, ());

        const SIDE: i32 = 64;
        commit_buffer(client, qh, &surface, SIDE, SIDE);

        conn.flush().expect("flushing the window-open requests");
        display
            .dispatch_clients(state)
            .expect("dispatching the window-open requests");

        let window = state
            .space
            .elements()
            .find(|window| !before.contains(window))
            .cloned()
            .expect("new_toplevel mapped a window");
        (window, toplevel, surface, xdg_surface)
    }

    /// Attach a buffer of exactly this size and commit it.
    ///
    /// A `Window`'s geometry is its surface's committed bounding box — this
    /// fixture never calls `set_window_geometry` — so this *is* how a client
    /// says what size it has become, and how it says it has become a
    /// different one from the one it was asked for.
    fn commit_buffer(
        client: &Client,
        qh: &QueueHandle<Client>,
        surface: &wl_surface::WlSurface,
        width: i32,
        height: i32,
    ) -> wl_buffer::WlBuffer {
        let shm = client.shm.clone().expect("wl_shm bound");
        let stride = width * 4;
        let bytes = stride * height;
        let fd = anon_file(bytes);
        let pool = shm.create_pool(fd.as_fd(), bytes, qh, ());
        let buffer = pool.create_buffer(0, width, height, stride, wl_shm::Format::Argb8888, qh, ());
        surface.attach(Some(&buffer), 0, 0);
        // Without this the server keeps the *old* buffer's damage and the
        // bounding box does not move: an attach is a promise and a commit is
        // the moment it counts.
        surface.damage(0, 0, width, height);
        surface.commit();
        buffer
    }

    /// One full round trip: the client's requests to the server, the
    /// server's events back to the client.
    ///
    /// **The `sync` is what makes the read safe.** `blocking_dispatch` waits
    /// on the socket and nothing but this thread drives the server, so a
    /// read issued when the server happened to have written nothing would
    /// hang the test binary for ever — and "the server wrote nothing" is
    /// precisely the assertion a throttle test is trying to make. A
    /// `wl_display.sync` guarantees at least the `done` event, so the read
    /// always returns and what it returns is *whatever else* was queued.
    fn pump(
        display: &mut Display<Solium>,
        state: &mut Solium,
        conn: &Connection,
        qh: &QueueHandle<Client>,
        queue: &mut wayland_client::EventQueue<Client>,
        client: &mut Client,
    ) {
        conn.display().sync(qh, ());
        conn.flush().expect("flushing the round trip");
        display
            .dispatch_clients(state)
            .expect("dispatching the round trip");
        display
            .flush_clients()
            .expect("flushing the server's events");
        queue
            .blocking_dispatch(client)
            .expect("reading the server's events");
    }

    /// **Issue #99: a rescale left already-open windows blurry.**
    ///
    /// Two protocols tell a client what scale to draw at. `commit`'s call to
    /// `send_surface_state` resends `wl_surface.preferred_buffer_scale` on
    /// every commit, so an existing client picks up a new output scale the
    /// moment it next draws. `new_fractional_scale` answers the other one,
    /// `wp_fractional_scale_v1`, but only when a client asks -- once, ever,
    /// per surface, and nothing called it again when `scale_outputs` changed
    /// a monitor's scale later. A window opened before a `super+shift+r`
    /// rescale kept the scale it was told at startup, and the compositor
    /// upscaled its buffer to fill the larger area the new scale gave it.
    ///
    /// The scenario it describes: two monitors, a `super+shift+r` rescale of
    /// one of them, and a window already open on each.
    #[test]
    fn changed_output_resends_fractional_scale_and_unchanged_output_does_not() {
        let mut display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());

        // No decoration, and that is load-bearing rather than tidiness.
        //
        // This fixture is the only test in the tree that drives a *real*
        // wayland-client, and a window that never negotiates
        // `xdg_decoration` now has a frame built for it on first show
        // (#103) -- so its two windows would each construct a Qt scene.
        // Doing that in a process already holding a raw libwayland
        // connection of its own aborts the whole test binary: SIGABRT,
        // nothing on stderr even under `--nocapture`, taking every test
        // after it down as well.
        //
        // It surfaced only when #103 and this fixture first met in one
        // build, each having been green on its own branch. `bare()`
        // short-circuits `Decorations::insert` before it reaches Qt, so
        // this asks for the one thing the fixture does not need and
        // cannot survive.
        //
        // Nothing is weakened by it. The subject here is whether a scale
        // change reaches surfaces that are already open; a frame has no
        // part in that, and the two assertions are untouched.
        state
            .decorations
            .set_style(&mut state.panes, Some("none".to_string()));

        // Two monitors side by side, both starting at 1x -- the state
        // they would be in right after `place_outputs` first ran.
        let output_a = Output::new(
            "scale-resend-test-a".to_string(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "solium".to_string(),
                model: "test-a".to_string(),
            },
        );
        output_a.change_current_state(
            Some(Mode {
                size: (1920, 1080).into(),
                refresh: 60_000,
            }),
            None,
            Some(Scale::Fractional(1.0)),
            None,
        );
        state.space.map_output(&output_a, (0, 0));

        let output_b = Output::new(
            "scale-resend-test-b".to_string(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "solium".to_string(),
                model: "test-b".to_string(),
            },
        );
        output_b.change_current_state(
            Some(Mode {
                size: (1920, 1080).into(),
                refresh: 60_000,
            }),
            None,
            Some(Scale::Fractional(1.0)),
            None,
        );
        state.space.map_output(&output_b, (1920, 0));

        // A real client -- see the module doc comment for why.
        let (conn, event_queue, client) = connect(&mut display, &mut state);
        let qh = event_queue.handle();

        let (window_a, _toplevel_a) = open_window(&mut display, &mut state, &conn, &client, &qh);
        let (window_b, _toplevel_b) = open_window(&mut display, &mut state, &conn, &client, &qh);

        // Placed explicitly, one per monitor: `new_toplevel` maps every
        // window at `(0, 0)`, and where the pane system fits it from
        // there depends on a layout this test does not configure.
        state.space.map_element(window_a.clone(), (100, 100), false);
        state
            .space
            .map_element(window_b.clone(), (2100, 100), false);
        // `elements_for_output` reads overlap data that only
        // `Space::refresh` computes -- see its own doc comment. The real
        // backends call it once a frame, for the same reason.
        state.space.refresh();

        let surface_a = window_a
            .wl_surface()
            .expect("window a has a surface")
            .into_owned();
        let surface_b = window_b
            .wl_surface()
            .expect("window b has a surface")
            .into_owned();

        let preferred_scale = |surface: &WlSurface| {
            with_states(surface, |states| {
                with_fractional_scale(states, |fractional| fractional.preferred_scale())
            })
        };

        assert_eq!(
            preferred_scale(&surface_a),
            None,
            "neither window has asked for a fractional scale yet, so \
             neither should have one before the rescale"
        );
        assert_eq!(preferred_scale(&surface_b), None);

        // The rescale: `a`'s monitor is asked for 2x, as if someone had
        // just edited it in and pressed `super+shift+r`. `b`'s monitor is
        // asked for exactly the scale it already has.
        state.arrangement = crate::monitor::Arrangement::new(vec![
            crate::monitor::Placement {
                name: "scale-resend-test-a".to_string(),
                at: None,
                beside: None,
                mode: crate::monitor::Wanted::default(),
                vrr: None,
                transform: None,
                enabled: true,
                primary: false,
                scale: crate::monitor::Scaling::Fixed(2.0),
            },
            crate::monitor::Placement {
                name: "scale-resend-test-b".to_string(),
                at: None,
                beside: None,
                mode: crate::monitor::Wanted::default(),
                vrr: None,
                transform: None,
                enabled: true,
                primary: false,
                scale: crate::monitor::Scaling::Fixed(1.0),
            },
        ]);
        state.scale_outputs();

        assert_eq!(
            preferred_scale(&surface_a),
            Some(2.0),
            "a's monitor actually changed scale (1x to 2x), so a real \
             client's wp_fractional_scale_v1 object -- bound once, at \
             startup, and never asked again -- must be told the new \
             value, or it keeps drawing at the old one forever. This is \
             issue #99."
        );
        assert_eq!(
            preferred_scale(&surface_b),
            None,
            "b's monitor was asked for exactly the scale it already had, \
             so scale_outputs's own early `continue` means this function \
             never runs for it at all -- and separately, b's window was \
             never on the monitor that changed, so it must be untouched \
             even if it had been"
        );
    }

    /// One 1920x1080 monitor at the origin, at 1x.
    ///
    /// Fullscreen fills the monitor a window is on and maximise fills its
    /// work area, so both need one to exist; with no layer surfaces on it,
    /// the work area is the whole monitor.
    fn one_screen(state: &mut Solium) -> Output {
        a_screen(state, "restore-test", (0, 0))
    }

    /// Two 1920x1080 monitors side by side at 1x: `left` at the origin and
    /// [`RIGHT_SCREEN`] beside it, top edges aligned.
    ///
    /// The arrangement nothing tested until #134's third review, and the
    /// one where "a screen away" stops meaning "nowhere": with the shipped
    /// `workspaces.spread` the left monitor's workspace 2 is parked 1.06
    /// screens to the right of it, which is squarely on the right monitor.
    fn side_by_side(state: &mut Solium, left: &str) -> (Output, Output) {
        (
            a_screen(state, left, (0, 0)),
            a_screen(state, RIGHT_SCREEN, (1920, 0)),
        )
    }

    /// The name [`side_by_side`] gives the right-hand monitor.
    const RIGHT_SCREEN: &str = "right-test";

    /// The last size this toplevel was configured with, as the client saw it.
    fn last_configured(
        client: &Client,
        toplevel: &xdg_toplevel::XdgToplevel,
    ) -> Option<(i32, i32)> {
        let id = wayland_client::Proxy::id(toplevel);
        client
            .configures
            .iter()
            .rev()
            .find(|(to, _, _)| *to == id)
            .map(|&(_, width, height)| (width, height))
    }

    /// **Issue #92: a window leaving fullscreen went nowhere.**
    ///
    /// `fullscreen_request` kept the rect to come back to on the window's
    /// `Decoration`, and then dropped that decoration so the fullscreen
    /// window would have no titlebar; the one `unfullscreen_request` built
    /// in its place had no rect in it. With nothing to re-place it, which is
    /// how this test runs -- no layout script at all -- the window stayed
    /// where fullscreen put it, covering the monitor, and was configured
    /// with no size. A layout that re-places windows on the relayout that
    /// follows covers for it, which is why the issue saw it on floating
    /// layouts and not on tiled ones.
    ///
    /// Run with `pane = "none"` for the reason every test in this module is
    /// (see the #99 test), which means this window never has a `Decoration`
    /// to lose. That fails on the old code all the same, and for the second
    /// half of the same defect: a window with no server-side frame had
    /// nowhere to keep the rect in the first place. The half with a real
    /// frame is `decoration.rs`'s
    /// `a_rebuilt_frame_does_not_take_the_way_back_with_it`.
    ///
    /// The request is sent twice, because a client may, and the second one
    /// must not overwrite the way back with the monitor it is already
    /// covering.
    #[test]
    fn a_window_leaving_fullscreen_is_back_where_it_was() {
        let mut display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        state
            .decorations
            .set_style(&mut state.panes, Some("none".to_string()));
        let output = one_screen(&mut state);
        let screen = state
            .space
            .output_geometry(&output)
            .expect("the monitor is mapped");

        let (conn, mut event_queue, mut client) = connect(&mut display, &mut state);
        let qh = event_queue.handle();
        let (window, toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
        state.space.map_element(window.clone(), (400, 300), false);
        state.space.refresh();
        let before = state.real_geometry(&window).expect("the window is mapped");
        assert_eq!(
            before,
            Rectangle::new((400, 300).into(), (64, 64).into()),
            "the fixture's window, where this test put it"
        );
        let id = state.panes.id_of(&window).expect("the window has a pane");

        toplevel.set_fullscreen(None);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut event_queue,
            &mut client,
        );
        toplevel.set_fullscreen(None);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut event_queue,
            &mut client,
        );
        assert_eq!(
            state.panes.get(id).map(Pane::slot),
            Some(screen),
            "fullscreen really did move the window, or coming back would \
             prove nothing"
        );
        assert_eq!(state.space.element_location(&window), Some(screen.loc));

        toplevel.unset_fullscreen();
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut event_queue,
            &mut client,
        );

        assert_eq!(
            state.panes.get(id).map(Pane::slot),
            Some(before),
            "a window leaving fullscreen goes back to the rect it had before \
             -- not the monitor it was covering, which is where it stayed \
             when there was no rect kept to go back to"
        );
        assert_eq!(
            state.space.element_location(&window),
            Some(before.loc),
            "and it is drawn there, not only recorded there"
        );
        assert_eq!(
            last_configured(&client, &toplevel),
            Some((before.size.w, before.size.h)),
            "and the client is told the size it had, rather than being \
             configured with no size and left to guess"
        );
    }

    /// **A window with no frame toggles back from maximised.**
    ///
    /// The rect a maximise goes back to lived on the window's `Decoration`,
    /// and a window with no server-side frame -- one that draws its own, or
    /// any window under `pane = "none"` -- has none, so on stage this
    /// toggle kept nothing for such a window and the second call maximised
    /// it again. Found with #92, and fixed by the same move.
    ///
    /// **Latent, not something a user could hit.** `toggle_maximize` has
    /// one caller, `frame_action`, reached only from a button on a
    /// `Styled` frame; there is no `maximize_request` handler, binding or
    /// script path. A window with no frame had no button to press. It is
    /// tested anyway because `pane = "none"` is the only way this module
    /// can drive `toggle_maximize` at all, and the maximise-then-fullscreen
    /// tests below build on it working. The frameless half of #92 that was
    /// seen is fullscreen's, in `a_window_leaving_fullscreen_is_back_where_it_was`.
    #[test]
    fn a_window_with_no_frame_toggles_back_from_maximised() {
        let mut display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        state
            .decorations
            .set_style(&mut state.panes, Some("none".to_string()));
        let output = one_screen(&mut state);
        let screen = state
            .space
            .output_geometry(&output)
            .expect("the monitor is mapped");

        let (conn, mut event_queue, mut client) = connect(&mut display, &mut state);
        let qh = event_queue.handle();
        let (window, toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
        state.space.map_element(window.clone(), (400, 300), false);
        state.space.refresh();
        let before = state.real_geometry(&window).expect("the window is mapped");
        let surface = window.toplevel().cloned().expect("an xdg toplevel");
        // The server's `xdg_toplevel`, which this module's `use` of the
        // client's shadows.
        let maximized = || {
            surface.with_pending_state(|pending| {
                pending.states.contains(
                    smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Maximized,
                )
            })
        };

        state.toggle_maximize(&window);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut event_queue,
            &mut client,
        );
        assert!(maximized(), "the first press maximises");
        assert_eq!(
            state.space.element_location(&window),
            Some(screen.loc),
            "and moves the window to the work area, which is the whole \
             monitor here, or coming back would prove nothing"
        );
        assert_eq!(
            last_configured(&client, &toplevel),
            Some((screen.size.w, screen.size.h))
        );

        state.toggle_maximize(&window);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut event_queue,
            &mut client,
        );
        assert!(
            !maximized(),
            "the second press restores, rather than maximising again"
        );
        assert_eq!(
            state.space.element_location(&window),
            Some(before.loc),
            "back to exactly where it was"
        );
        assert_eq!(
            last_configured(&client, &toplevel),
            Some((before.size.w, before.size.h)),
            "at exactly the size it was"
        );
    }

    /// A 1920x1080 monitor at 1x, mapped at `at`.
    ///
    /// Named, because two of them in one test must not share a name:
    /// `place_outputs` goes by names, and the unplug tests below run it.
    fn a_screen(state: &mut Solium, name: &str, at: (i32, i32)) -> Output {
        let output = Output::new(
            name.to_string(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "solium".to_string(),
                model: name.to_string(),
            },
        );
        output.change_current_state(
            Some(Mode {
                size: (1920, 1080).into(),
                refresh: 60_000,
            }),
            None,
            Some(Scale::Fractional(1.0)),
            None,
        );
        state.space.map_output(&output, at);
        output
    }

    /// A bar across the top of the primary monitor, `height` pixels tall,
    /// holding them as its exclusive zone.
    ///
    /// Committed once and with no buffer, which is enough: the zone is
    /// double-buffered state that the initial commit applies, and
    /// `configure_layer` arranges the monitor on that commit. The caller
    /// checks the work area it leaves rather than trusting this.
    fn bar(
        display: &mut Display<Solium>,
        state: &mut Solium,
        conn: &Connection,
        client: &Client,
        qh: &QueueHandle<Client>,
        height: i32,
    ) -> (
        wl_surface::WlSurface,
        zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    ) {
        let compositor = client.compositor.clone().expect("wl_compositor bound");
        let shell = client
            .layer_shell
            .clone()
            .expect("zwlr_layer_shell_v1 bound");
        let surface = compositor.create_surface(qh, ());
        let layer = shell.get_layer_surface(
            &surface,
            None,
            zwlr_layer_shell_v1::Layer::Top,
            "restore-test-bar".to_string(),
            qh,
            (),
        );
        layer.set_anchor(
            zwlr_layer_surface_v1::Anchor::Top
                | zwlr_layer_surface_v1::Anchor::Left
                | zwlr_layer_surface_v1::Anchor::Right,
        );
        layer.set_size(0, height.unsigned_abs());
        layer.set_exclusive_zone(height);
        surface.commit();
        conn.flush().expect("flushing the bar");
        display
            .dispatch_clients(state)
            .expect("dispatching the bar");
        (surface, layer)
    }

    /// Whether the server has this window in `wanted`, as the next
    /// configure it is sent will say.
    fn in_state(
        window: &Window,
        wanted: smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State,
    ) -> bool {
        window.toplevel().is_some_and(|surface| {
            surface.with_pending_state(|pending| pending.states.contains(wanted))
        })
    }

    /// Every size this toplevel has been configured with, oldest first.
    fn sizes_sent(client: &Client, toplevel: &xdg_toplevel::XdgToplevel) -> Vec<(i32, i32)> {
        let id = wayland_client::Proxy::id(toplevel);
        client
            .configures
            .iter()
            .filter(|(to, _, _)| *to == id)
            .map(|&(_, width, height)| (width, height))
            .collect()
    }

    /// **#92 review, finding 1: a maximised window leaving fullscreen is
    /// maximised again.**
    ///
    /// Maximise and fullscreen keep their way back in the one slot on the
    /// pane, and a window maximised and then sent fullscreen keeps the rect
    /// from before the maximise in it. Leaving fullscreen took that rect:
    /// the window was placed and sized un-maximised while its xdg state
    /// still said `Maximized`, and the slot was empty, so the next toggle
    /// maximised it again rather than restoring it. A maximised browser
    /// sent fullscreen for a video came back small on Esc, still told it
    /// was maximised.
    ///
    /// The bar is what makes the placement assertion mean anything. Without
    /// one the work area is the whole monitor, which is also where
    /// fullscreen put the window, so "back at the work area" and "left where
    /// fullscreen put it" would be the same rectangle.
    ///
    /// `pane = "none"`, as everywhere in this module, so there is no frame's
    /// share to take out of the work area. `toggle_maximize` and leaving
    /// fullscreen both take it from `frame_insets`, and this does not test
    /// that they agree about a frame that is really there.
    #[test]
    fn a_maximised_window_leaving_fullscreen_is_maximised_again() {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;

        let mut display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        state
            .decorations
            .set_style(&mut state.panes, Some("none".to_string()));
        let output = one_screen(&mut state);
        let screen = state
            .space
            .output_geometry(&output)
            .expect("the monitor is mapped");

        let (conn, mut event_queue, mut client) = connect(&mut display, &mut state);
        let qh = event_queue.handle();
        let _bar = bar(&mut display, &mut state, &conn, &client, &qh, 30);
        let work = state.work_area_on(&output).expect("the monitor is mapped");
        assert_eq!(
            work,
            Rectangle::new((0, 30).into(), (1920, 1050).into()),
            "the bar holds the top of the screen, or the work area is the \
             monitor and nothing below can tell maximised from fullscreen"
        );

        let (window, toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
        state.space.map_element(window.clone(), (400, 300), false);
        state.space.refresh();
        let before = state.real_geometry(&window).expect("the window is mapped");
        let id = state.panes.id_of(&window).expect("the window has a pane");

        state.toggle_maximize(&window);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut event_queue,
            &mut client,
        );
        assert!(in_state(&window, State::Maximized));
        assert_eq!(state.space.element_location(&window), Some(work.loc));

        toplevel.set_fullscreen(None);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut event_queue,
            &mut client,
        );
        assert_eq!(
            state.space.element_location(&window),
            Some(screen.loc),
            "fullscreen covers the bar, so leaving it has somewhere to come \
             back from"
        );

        client.configures.clear();
        toplevel.unset_fullscreen();
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut event_queue,
            &mut client,
        );

        assert!(!in_state(&window, State::Fullscreen));
        assert!(
            in_state(&window, State::Maximized),
            "it was maximised when it went fullscreen and nothing has \
             un-maximised it since"
        );
        assert_eq!(
            state.space.element_location(&window),
            Some(work.loc),
            "so it is placed maximised, below the bar -- not at the rect \
             from before the maximise, which is where leaving fullscreen \
             used to put it while its state still said maximised"
        );
        assert_eq!(state.panes.get(id).map(Pane::slot), Some(work));
        assert_eq!(
            sizes_sent(&client, &toplevel),
            vec![(work.size.w, work.size.h)],
            "and it is told the work area's size, once"
        );
        assert_eq!(
            state.panes.get(id).and_then(Pane::restore),
            Some(before),
            "the rect from before the maximise is still kept, for the \
             un-maximise"
        );

        state.toggle_maximize(&window);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut event_queue,
            &mut client,
        );
        assert!(
            !in_state(&window, State::Maximized),
            "the next toggle restores, rather than maximising again"
        );
        assert_eq!(
            state.space.element_location(&window),
            Some(before.loc),
            "to where it was before the maximise"
        );
        assert_eq!(
            last_configured(&client, &toplevel),
            Some((before.size.w, before.size.h))
        );
    }

    /// **#92 review, finding 1: leaving fullscreen needs a window that is
    /// fullscreen.**
    ///
    /// A client may send `unset_fullscreen` whenever it likes. One that
    /// sent it while merely maximised had the maximise's way back spent on
    /// it, because the two share one slot: the window jumped back to its
    /// pre-maximise rect still marked maximised, and the next toggle
    /// maximised it again.
    #[test]
    fn a_window_that_is_not_fullscreen_has_nothing_to_leave() {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;

        let mut display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        state
            .decorations
            .set_style(&mut state.panes, Some("none".to_string()));
        let output = one_screen(&mut state);
        let screen = state
            .space
            .output_geometry(&output)
            .expect("the monitor is mapped");

        let (conn, mut event_queue, mut client) = connect(&mut display, &mut state);
        let qh = event_queue.handle();
        let (window, toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
        state.space.map_element(window.clone(), (400, 300), false);
        state.space.refresh();
        let before = state.real_geometry(&window).expect("the window is mapped");
        let id = state.panes.id_of(&window).expect("the window has a pane");

        state.toggle_maximize(&window);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut event_queue,
            &mut client,
        );
        assert!(in_state(&window, State::Maximized));

        client.configures.clear();
        toplevel.unset_fullscreen();
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut event_queue,
            &mut client,
        );
        assert_eq!(
            state.panes.get(id).and_then(Pane::restore),
            Some(before),
            "a window that was never fullscreen keeps its maximise's way back"
        );
        assert_eq!(
            state.space.element_location(&window),
            Some(screen.loc),
            "and stays maximised where it was"
        );
        assert_eq!(
            sizes_sent(&client, &toplevel),
            Vec::<(i32, i32)>::new(),
            "and is told nothing, because nothing changed"
        );

        state.toggle_maximize(&window);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut event_queue,
            &mut client,
        );
        assert!(!in_state(&window, State::Maximized));
        assert_eq!(state.space.element_location(&window), Some(before.loc));
    }

    /// **#92 review, finding 3: leaving fullscreen is one configure.**
    ///
    /// It was two: the size cleared -- 0x0, "pick your own" -- and then the
    /// size to go back to. A client that acts on every configure it reads,
    /// a terminal reflowing its grid, resized twice, once to a size of its
    /// own choosing and once to the real one. On stage the second was never
    /// sent, because there was never a rect to send.
    #[test]
    fn a_window_leaving_fullscreen_is_told_its_size_once() {
        let mut display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        state
            .decorations
            .set_style(&mut state.panes, Some("none".to_string()));
        let _output = one_screen(&mut state);

        let (conn, mut event_queue, mut client) = connect(&mut display, &mut state);
        let qh = event_queue.handle();
        let (window, toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
        state.space.map_element(window.clone(), (400, 300), false);
        state.space.refresh();
        let before = state.real_geometry(&window).expect("the window is mapped");

        toplevel.set_fullscreen(None);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut event_queue,
            &mut client,
        );
        client.configures.clear();
        toplevel.unset_fullscreen();
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut event_queue,
            &mut client,
        );
        assert_eq!(
            sizes_sent(&client, &toplevel),
            vec![(before.size.w, before.size.h)],
            "one configure, with the size it had before"
        );
    }

    /// **#92 review, finding 4: a window leaving fullscreen on a monitor
    /// that has gone comes back on one that is here.**
    ///
    /// The rect a window goes back to is stored, and a stored rect can be
    /// on a monitor that has since been unplugged or gone to sleep.
    /// `rescue_offscreen` brings the fullscreen window itself onto a
    /// remaining screen, but not the rect it goes back to, and it runs
    /// only when the monitors change: leaving fullscreen put the window on
    /// no screen at all, and it stayed there until the next hotplug.
    #[test]
    fn a_window_leaving_fullscreen_after_its_monitor_went_is_on_a_screen() {
        let mut display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        state
            .decorations
            .set_style(&mut state.panes, Some("none".to_string()));
        let left = a_screen(&mut state, "restore-left", (0, 0));
        let right = a_screen(&mut state, "restore-right", (1920, 0));
        let remaining = state
            .space
            .output_geometry(&left)
            .expect("the left monitor is mapped");

        let (conn, mut event_queue, mut client) = connect(&mut display, &mut state);
        let qh = event_queue.handle();
        let (window, toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
        state.space.map_element(window.clone(), (2320, 300), false);
        state.space.refresh();
        let before = state.real_geometry(&window).expect("the window is mapped");

        toplevel.set_fullscreen(None);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut event_queue,
            &mut client,
        );
        assert_eq!(
            state.space.element_location(&window),
            Some((1920, 0).into()),
            "fullscreen on the monitor the window is on, the right one"
        );

        // Unplugged, the way both backends do it.
        state.space.unmap_output(&right);
        state.settle_monitors();
        let rescued = state.real_geometry(&window).expect("the window is mapped");
        assert!(
            remaining.overlaps(rescued),
            "rescue_offscreen brought the fullscreen window onto the screen \
             that is left, or leaving fullscreen would start from nowhere: \
             {rescued:?}"
        );

        toplevel.unset_fullscreen();
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut event_queue,
            &mut client,
        );
        let back = state.real_geometry(&window).expect("the window is mapped");
        assert!(
            state.on_any_output(back),
            "the window that left fullscreen is on a screen: {back:?}, not \
             at the {before:?} it had on the monitor that went"
        );
        assert_eq!(
            back,
            Rectangle::new((1920 - 64, 300).into(), (64, 64).into()),
            "moved the way rescue_offscreen moves a stranded window: onto \
             the nearest screen, its size kept, clamped at the edge it was \
             beyond"
        );
        assert_eq!(last_configured(&client, &toplevel), Some((64, 64)));
    }

    /// **#92 review, finding 4, for a maximise.**
    ///
    /// The same stored rect and the same hazard. Leaving fullscreen now
    /// puts a maximised window back at its work area and leaves the rect
    /// from before the maximise for the un-maximise, so a window maximised
    /// on a monitor that then goes needs the un-maximise to check too.
    #[test]
    fn a_window_maximised_on_a_monitor_that_went_restores_onto_a_screen() {
        let mut display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        state
            .decorations
            .set_style(&mut state.panes, Some("none".to_string()));
        let left = a_screen(&mut state, "restore-left", (0, 0));
        let right = a_screen(&mut state, "restore-right", (1920, 0));
        let remaining = state
            .space
            .output_geometry(&left)
            .expect("the left monitor is mapped");

        let (conn, mut event_queue, mut client) = connect(&mut display, &mut state);
        let qh = event_queue.handle();
        let (window, toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
        state.space.map_element(window.clone(), (2320, 300), false);
        state.space.refresh();

        state.toggle_maximize(&window);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut event_queue,
            &mut client,
        );
        assert_eq!(
            state.space.element_location(&window),
            Some((1920, 0).into()),
            "maximised on the monitor the window is on, the right one"
        );

        state.space.unmap_output(&right);
        state.settle_monitors();
        let rescued = state.real_geometry(&window).expect("the window is mapped");
        assert!(
            remaining.overlaps(rescued),
            "rescue_offscreen brought the maximised window onto the screen \
             that is left: {rescued:?}"
        );

        state.toggle_maximize(&window);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut event_queue,
            &mut client,
        );
        let back = state.real_geometry(&window).expect("the window is mapped");
        assert_eq!(
            back,
            Rectangle::new((1920 - 64, 300).into(), (64, 64).into()),
            "restored onto the screen that is left, not to 2320,300 on the \
             one that went"
        );
        assert_eq!(last_configured(&client, &toplevel), Some((64, 64)));
    }

    /// **#92 review, finding 5: a window fullscreen before it has drawn
    /// keeps no way back.**
    ///
    /// A player started with `--fs` asks before its first commit.
    /// `new_toplevel` has mapped it at 0,0 by then, with no buffer and so no
    /// size, and that 0x0 rect was kept as the way back: leaving fullscreen
    /// configured 0x0 -- which on the wire is "pick your own", the same as
    /// no rect at all -- and set the pane's slot to a rect of no size.
    ///
    /// And with no rect kept, a second request once the window has drawn
    /// must still not keep one: the window is fullscreen by then, and the
    /// rect it has is the monitor.
    #[test]
    fn a_window_fullscreen_before_it_has_drawn_keeps_no_way_back() {
        let mut display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        state
            .decorations
            .set_style(&mut state.panes, Some("none".to_string()));
        let output = one_screen(&mut state);
        let screen = state
            .space
            .output_geometry(&output)
            .expect("the monitor is mapped");

        let (conn, mut event_queue, mut client) = connect(&mut display, &mut state);
        let qh = event_queue.handle();
        let compositor = client.compositor.clone().expect("wl_compositor bound");
        let wm_base = client.wm_base.clone().expect("xdg_wm_base bound");
        let surface = compositor.create_surface(&qh, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg_surface.get_toplevel(&qh, ());
        toplevel.set_fullscreen(None);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut event_queue,
            &mut client,
        );
        let window = state
            .space
            .elements()
            .next()
            .cloned()
            .expect("new_toplevel maps a window before it has drawn anything");
        let id = state.panes.id_of(&window).expect("the window has a pane");
        assert_eq!(
            state.panes.get(id).and_then(Pane::restore),
            None,
            "a window with no size has no rect worth going back to"
        );

        commit_buffer(&client, &qh, &surface, screen.size.w, screen.size.h);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut event_queue,
            &mut client,
        );
        toplevel.set_fullscreen(None);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut event_queue,
            &mut client,
        );
        assert_eq!(
            state.panes.get(id).and_then(Pane::restore),
            None,
            "asking again while fullscreen does not keep the monitor as the \
             way back"
        );

        client.configures.clear();
        toplevel.unset_fullscreen();
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut event_queue,
            &mut client,
        );
        assert_eq!(
            sizes_sent(&client, &toplevel),
            vec![(0, 0)],
            "with nowhere kept to go back to, the client picks its own size"
        );
        assert!(
            state
                .panes
                .get(id)
                .is_some_and(|pane| !pane.slot().is_empty()),
            "and the pane is not given a rect of no size"
        );
    }

    /// **A modal cannot be buried under the window it is waiting on.**
    ///
    /// The regression floating them introduced. Tiled, a dialog took a slot
    /// of its own and overlapped nothing, so there was nowhere for it to be
    /// lost; floated, it sits *on* its parent, and nothing in the stack said
    /// which of the two belongs on top. `Space::map_element` takes an
    /// element out of the stack and pushes it back at the top whatever
    /// `activate` says, so every raise buried the prompt: one click on the
    /// strip of document left showing, or one pointer crossing with
    /// `focus_follows_mouse`, and "Discard changes?" was behind the window
    /// refusing to accept keystrokes until it is answered.
    ///
    /// The third window is what stops this passing for the wrong reason. If
    /// `focus_window` had quietly stopped restacking at all, the dialog
    /// would still be above its parent and the test would be green over a
    /// broken raise -- so the parent is also asserted to have come above the
    /// window it was under.
    #[test]
    fn a_modal_stays_above_the_window_it_is_waiting_on() {
        let mut display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        // See the #99 test: a Qt scene in a process holding a libwayland
        // connection of its own aborts the whole test binary.
        state
            .decorations
            .set_style(&mut state.panes, Some("none".to_string()));

        let (conn, event_queue, client) = connect(&mut display, &mut state);
        let qh = event_queue.handle();

        let (parent, parent_toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
        let (dialog, dialog_toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
        let (other, _other_toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);

        // The two requests that make this a modal dialog, in the order a
        // toolkit sends them and from the only side that can send them.
        let dialogs = client.dialogs.clone().expect("xdg_wm_dialog_v1 bound");
        dialog_toplevel.set_parent(Some(&parent_toplevel));
        let object = dialogs.get_xdg_dialog(&dialog_toplevel, &qh, ());
        object.set_modal();
        conn.flush().expect("flushing set_parent and set_modal");
        display
            .dispatch_clients(&mut state)
            .expect("dispatching set_parent and set_modal");

        // The fixture is doing what it claims before anything is asserted
        // about stacking: the compositor agrees this is a modal, and agrees
        // whose.
        assert!(
            state.is_modal(&dialog),
            "the client called set_modal and the compositor did not read it \
             back, so nothing below is about a modal dialog at all"
        );
        let parent_pane = state.panes.id_of(&parent).expect("the parent has a pane");
        assert_eq!(
            state.parent_of(&dialog),
            Parentage::Window(parent_pane.get()),
            "the client called set_parent and the compositor did not read it \
             back"
        );

        // `elements` is back to front, so a higher index is nearer the top.
        let depth = |state: &Solium, window: &Window| {
            state
                .space
                .elements()
                .position(|element| element == window)
                .expect("a mapped window is in the space")
        };
        assert!(
            depth(&state, &dialog) > depth(&state, &parent),
            "the dialog opened after its parent, so it starts above it -- and \
             a test that starts in the state it is checking for proves nothing"
        );

        // One click on the parent. This is the whole of the bug.
        state.focus_window(&parent, SERIAL_COUNTER.next_serial());

        assert!(
            depth(&state, &parent) > depth(&state, &other),
            "focusing the parent did not raise it above the window that was \
             over it, so this run says nothing about what a raise does to its \
             dialog"
        );
        assert!(
            depth(&state, &dialog) > depth(&state, &parent),
            "the parent was raised over the dialog that is waiting on it: the \
             prompt is now behind the window it is blocking, where it cannot \
             be read or dismissed"
        );
    }

    /// **This is issue #113, through the compositor rather than beside it.**
    ///
    /// A top-left drag, on the first frame, with a real client that has
    /// committed one buffer and will not commit another. That last part is
    /// the whole test: the client answers *nothing*, so what is asserted is
    /// what the compositor draws while it is waiting — which is exactly the
    /// window the shake lives in.
    ///
    /// **A test that checked the final rectangle would pass on `stage`.**
    /// The client does commit eventually and the window does end up the
    /// right size; the bug is entirely in the frames before that. So this
    /// asserts the rectangle with no answer in hand, and it asserts the
    /// *pinned* edge rather than the dragged one — `stage` gets the dragged
    /// corner right, because it applies the origin immediately. What it
    /// gets wrong is the opposite corner, which it moves by the whole of
    /// the drag's delta and then snaps back.
    ///
    /// Confirmed failing against `stage`'s rule, not merely assumed to: the
    /// rectangle `stage` would produce is computed here from the same
    /// inputs and asserted to be wrong. `stage`'s `pane_geometry` answers
    /// `real_geometry`, which is the space's location paired with the
    /// *client's* size — `on_stage` below, spelled out.
    #[test]
    fn the_edge_being_dragged_is_the_edge_that_moves_before_any_client_answers() {
        let mut display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        // See the #99 test: a Qt scene in a process holding a libwayland
        // connection of its own aborts the whole test binary.
        state
            .decorations
            .set_style(&mut state.panes, Some("none".to_string()));

        let (conn, event_queue, client) = connect(&mut display, &mut state);
        let qh = event_queue.handle();
        let (window, _toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);

        // Away from the origin, so a drag that moved the window to where it
        // already was could not pass by accident.
        state.map_stacked(window.clone(), (400, 300), false);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&window)
            .expect("a client in the space has a pane");
        let before = state
            .pane_outer_of(pane)
            .expect("a mapped pane has a rectangle");
        let painted = window.geometry().size;
        assert!(
            painted.w > 20 && painted.h > 12,
            "the drag below has to leave a window with a size"
        );

        // The drag: the top-left corner, 20 right and 12 down. Smaller by
        // that much, moved by that much, with the bottom-right corner
        // standing still — that is what dragging a top-left corner means.
        let wanted = Rectangle::new(
            (before.loc.x + 20, before.loc.y + 12).into(),
            (before.size.w - 20, before.size.h - 12).into(),
        );
        state.pending_resize = Some(ResizeRequest {
            window: window.clone(),
            wanted,
            edge_at: payload_edge(wanted, ResizeEdge::TopLeft),
            edges: ResizeEdge::TopLeft,
        });
        state.settle_resize();

        // The client has been asked and has said nothing. Asserted rather
        // than assumed, because a client that *had* answered would make
        // every assertion below true for the wrong reason.
        assert_eq!(
            window.geometry().size,
            painted,
            "the fixture's client never commits a second buffer; if it had, \
             this test would be checking the easy case"
        );

        let after = state
            .pane_outer_of(pane)
            .expect("a mapped pane has a rectangle");
        assert_eq!(
            after, wanted,
            "the pane takes the dragged rectangle whole — origin and size \
             in the same frame — or the two halves land at different times \
             and that gap is the shake"
        );
        assert_eq!(
            (after.loc.x + after.size.w, after.loc.y + after.size.h),
            (before.loc.x + before.size.w, before.loc.y + before.size.h),
            "the bottom-right corner is not being dragged and must not move"
        );

        // And `stage`'s rule, computed from the same two inputs: the new
        // origin paired with the client's size, which is what
        // `real_geometry` answers and what `pane_geometry` used to return.
        let on_stage = Rectangle::new(wanted.loc, painted);
        assert_ne!(
            (
                on_stage.loc.x + on_stage.size.w,
                on_stage.loc.y + on_stage.size.h
            ),
            (before.loc.x + before.size.w, before.loc.y + before.size.h),
            "the control: pairing the new origin with the client's old size \
             walks the corner nobody is dragging across the desktop, and \
             snaps it back when the client finally commits. That is #113, \
             and it is what this test fails on against stage"
        );
        assert_eq!(
            on_stage.loc.x + on_stage.size.w,
            before.loc.x + before.size.w + 20,
            "by the drag's whole delta, every frame"
        );

        // The space agrees about *position* throughout. Only the size is
        // held back, because only the size needs the client's consent —
        // holding the position back too would put the space and the pane
        // into the standing disagreement #84 is about, for no gain.
        assert_eq!(
            state
                .space
                .element_location(&window)
                .expect("a mapped window has a location"),
            wanted.loc,
            "the slot is authoritative for the size, not for where the \
             window is; nothing here may add a fourth opinion about that"
        );
    }

    /// **The round trip this whole fix rests on, with a frame's insets in
    /// it, on all eight edges.**
    ///
    /// `hold_resize` writes the pane's slot as `inner(wanted)` and every
    /// reader grows it back with `pane_outer`, so `pane_outer(inner(wanted))
    /// == wanted` is what makes the dragged rectangle survive the trip. It
    /// held for zero insets whichever spelling was used, which is why a
    /// window with no frame could not catch this: `frame_insets` asks
    /// `is_decorated` — `Styled` and nothing else — while every reader goes
    /// through `insets_of`, which reserves a titlebar for `Frame::Pending`
    /// so a window does not change shape when its frame arrives.
    ///
    /// `Frame::Pending` is not a moment, it is where a pane whose
    /// decoration *failed to build* stays for good — see
    /// `decoration::Decorations::insert`, which logs and leaves it there. So
    /// the two spellings disagreeing meant that for those windows the slot
    /// grew by a titlebar on every frame of a drag: the top edge landing a
    /// titlebar above the rectangle under the pointer, and the client asked
    /// for a size a titlebar too tall.
    ///
    /// Set by hand rather than by failing a build, because building a frame
    /// needs Qt and a Qt scene in a process holding a libwayland connection
    /// aborts the test binary (see the #99 test). The state is the same
    /// state; how a pane got into it is `decoration.rs`'s business.
    #[test]
    fn a_framed_window_is_dragged_to_the_rectangle_the_pointer_asks_for() {
        let mut display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        state
            .decorations
            .set_style(&mut state.panes, Some("none".to_string()));

        let (conn, event_queue, client) = connect(&mut display, &mut state);
        let qh = event_queue.handle();
        let (window, _toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
        state.map_stacked(window.clone(), (400, 300), false);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&window)
            .expect("a client in the space has a pane");

        // A pane reserving room for a frame that is never coming.
        state
            .panes
            .get_mut(pane)
            .expect("the pane is here")
            .set_frame(crate::pane::Frame::Pending);
        assert!(
            !state.is_decorated(&window),
            "`Pending` is not decorated, which is exactly why the two \
             spellings of the insets disagree about it"
        );
        assert_eq!(
            state.insets_of(pane).top,
            TITLEBAR_HEIGHT,
            "and it reserves a titlebar all the same, or this test is about \
             two things that agree"
        );

        for edges in [
            ResizeEdge::Top,
            ResizeEdge::Bottom,
            ResizeEdge::Left,
            ResizeEdge::Right,
            ResizeEdge::TopLeft,
            ResizeEdge::TopRight,
            ResizeEdge::BottomLeft,
            ResizeEdge::BottomRight,
        ] {
            // A fresh gesture each time, which also reconciles the previous
            // one rather than leaving it hanging.
            let before = state
                .begin_resize(&window)
                .expect("a mapped pane has a rectangle");

            // Twenty pixels out of whichever edges this drag holds, with
            // the opposite ones standing still. `resized` is where that
            // arithmetic lives and it is tested beside itself; what is
            // being checked here is only that the rectangle survives the
            // compositor.
            let (left, top) = (
                crate::input::resize::pulls_left(edges),
                crate::input::resize::pulls_top(edges),
            );
            let right = matches!(
                edges,
                ResizeEdge::Right | ResizeEdge::TopRight | ResizeEdge::BottomRight
            );
            let bottom = matches!(
                edges,
                ResizeEdge::Bottom | ResizeEdge::BottomLeft | ResizeEdge::BottomRight
            );
            let wanted = Rectangle::new(
                (
                    before.loc.x - i32::from(left) * 20,
                    before.loc.y - i32::from(top) * 20,
                )
                    .into(),
                (
                    before.size.w + i32::from(left || right) * 20,
                    before.size.h + i32::from(top || bottom) * 20,
                )
                    .into(),
            );
            assert_ne!(wanted, before, "every edge has to actually move one");
            state.pending_resize = Some(ResizeRequest {
                window: window.clone(),
                wanted,
                edge_at: payload_edge(wanted, edges),
                edges,
            });
            state.settle_resize();

            assert_eq!(
                state
                    .pane_outer_of(pane)
                    .expect("a mapped pane has a rectangle"),
                wanted,
                "the drag asked for this rectangle and {edges:?} did not get \
                 it: the slot is written with one spelling of the frame's \
                 insets and read back with another"
            );
            // And the client is asked for what is left inside the frame,
            // not for the whole of it.
            assert_eq!(
                state
                    .panes
                    .get(pane)
                    .expect("the pane is here")
                    .slot()
                    .size
                    .h,
                wanted.size.h - TITLEBAR_HEIGHT,
                "a titlebar's worth of the dragged rectangle belongs to the \
                 frame, so the client must not be sized the whole of it"
            );
        }
    }

    /// **A whole gesture inside one dispatch batch, which is an ordinary
    /// quick nudge of a border.**
    ///
    /// The press, the motion and the release all land in one calloop
    /// dispatch — sixteen milliseconds is plenty — and `settle_resize` runs
    /// at the frame *after* all three. So the hold is born after the
    /// gesture it belongs to has already ended: `ResizeGrab::motion` only
    /// records `pending_resize`, and `hold_resize` is what turns that into a
    /// `Hold`.
    ///
    /// A hold that cannot observe its own release never sets `released`,
    /// and `Hold::settle` answers `Waiting` unconditionally without one. The
    /// hold is then **permanent**: `holding_resize` stays true for ever,
    /// `pane_geometry` keeps answering the slot, and every later size the
    /// client chooses for itself is stretched into a rectangle from a drag
    /// that finished minutes ago. A window soft until something drags it
    /// again.
    ///
    /// So what this waits for is the deadline, which is the *only* thing
    /// that can end this gesture: the fixture's client answers nothing at
    /// all. Slept rather than faked because `present::Clock` reads the
    /// monotonic clock through — see its own documentation for why it has no
    /// settable "now" to lie to.
    #[test]
    fn a_gesture_that_ends_before_its_first_frame_still_lets_go() {
        let mut display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        // See the #99 test: a Qt scene in a process holding a libwayland
        // connection of its own aborts the whole test binary.
        state
            .decorations
            .set_style(&mut state.panes, Some("none".to_string()));

        let (conn, event_queue, client) = connect(&mut display, &mut state);
        let qh = event_queue.handle();
        let (window, _toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);

        state.map_stacked(window.clone(), (400, 300), false);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&window)
            .expect("a client in the space has a pane");
        let before = state
            .pane_outer_of(pane)
            .expect("a mapped pane has a rectangle");

        // The press.
        state.begin_resize(&window);
        // The motion. `ResizeGrab::motion` records and does not apply, so
        // this is the whole of what a motion does before a frame.
        let wanted = Rectangle::new(
            (before.loc.x + 20, before.loc.y + 12).into(),
            (before.size.w - 20, before.size.h - 12).into(),
        );
        state.pending_resize = Some(ResizeRequest {
            window: window.clone(),
            wanted,
            edge_at: payload_edge(wanted, ResizeEdge::TopLeft),
            edges: ResizeEdge::TopLeft,
        });
        // And the release, still with no frame in between: this is
        // `ResizeGrab::unset`, which is where the button coming up lands.
        state.release_resize(&window);

        // *Now* the frame. This is where the hold is born, and it is born
        // into a gesture that is already over.
        state.settle_resize();
        assert!(
            state.holding_resize(pane),
            "the frame after the release is where this hold is born; if no \
             hold is created at all then this test is about nothing"
        );
        assert_eq!(
            window.geometry().size,
            before.size,
            "the fixture's client never commits a second buffer, which is \
             what leaves the deadline as the only thing that can end this"
        );

        // Past the deadline, and one more frame to notice it.
        std::thread::sleep(crate::resizing::PATIENCE + std::time::Duration::from_millis(100));
        state.settle_resize();
        assert!(
            !state.holding_resize(pane),
            "the hold outlived its own gesture's deadline, so it will outlive \
             everything: `pane_geometry` answers the slot for as long as this \
             is true and the client's own size is stretched into it for ever"
        );
        assert_eq!(
            state.pane_geometry(state.panes.get(pane).expect("the pane is still here")),
            state
                .real_geometry(&window)
                .expect("a mapped window has a rectangle"),
            "once the hold is gone the slot and the space agree again, which \
             is what makes the stretch exactly 1"
        );
    }

    /// **The same defect by the other route: a hold dropped mid-gesture and
    /// re-created after the release.**
    ///
    /// `settle_resize` drops the hold on any frame a layout claims the
    /// drag — `trigger_resize` answering true — and creates a fresh one on
    /// any frame it does not. A script whose answer changes between two
    /// frames therefore destroys a hold and builds another, and if the
    /// second one is built on the frame *after* the button came up it is
    /// built into a gesture that is already over. Identical outcome to the
    /// quick-nudge case and a completely different way in, which is why the
    /// release is recorded on the compositor rather than guarded at each
    /// place a hold is made.
    ///
    /// The drop is driven directly here rather than through a Lua layout:
    /// `drop_resize_hold_for` *is* the line `settle_resize` runs when a
    /// layout claims the drag, and standing up a script that changes its
    /// mind between frames would test mlua rather than this.
    #[test]
    fn a_hold_rebuilt_after_the_release_is_rebuilt_already_released() {
        let mut display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        state
            .decorations
            .set_style(&mut state.panes, Some("none".to_string()));

        let (conn, event_queue, client) = connect(&mut display, &mut state);
        let qh = event_queue.handle();
        let (window, _toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
        state.map_stacked(window.clone(), (400, 300), false);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&window)
            .expect("a client in the space has a pane");
        let before = state
            .begin_resize(&window)
            .expect("a mapped pane has a rectangle");

        let dragged = |state: &mut Solium, by: i32| {
            let wanted = Rectangle::new(
                (before.loc.x + by, before.loc.y + by).into(),
                (before.size.w - by, before.size.h - by).into(),
            );
            state.pending_resize = Some(ResizeRequest {
                window: window.clone(),
                wanted,
                edge_at: payload_edge(wanted, ResizeEdge::TopLeft),
                edges: ResizeEdge::TopLeft,
            });
        };

        // A frame of ordinary drag, so a hold exists to be dropped.
        dragged(&mut state, 8);
        state.settle_resize();
        assert!(state.holding_resize(pane), "the drag is live");

        // The frame a layout claims it.
        state.drop_resize_hold_for(&window);
        assert!(!state.holding_resize(pane));

        // The button comes up with no hold to tell, and the frame after it
        // carries the last motion — which is where a hold is made again.
        state.release_resize(&window);
        dragged(&mut state, 12);
        state.settle_resize();
        assert!(
            state.holding_resize(pane),
            "the post-release frame is where the second hold is born; \
             without one there is nothing here to go wrong"
        );

        std::thread::sleep(crate::resizing::PATIENCE + std::time::Duration::from_millis(100));
        state.settle_resize();
        assert!(
            !state.holding_resize(pane),
            "the second hold never heard about the release that preceded it, \
             so nothing can ever end it"
        );
    }

    /// One frame of a drag that a layout claims, without standing up a
    /// layout.
    ///
    /// These three statements in this order *are* what `settle_resize` runs
    /// on the claimed branch: it arms the gesture, asks the scripts, and the
    /// scripts' own `apply` is what reaches `move_pane` — the whole layout
    /// sweep happens inside `trigger_resize`, before the caller learns
    /// whether the drag was claimed. Driving them directly is the same
    /// choice `a_hold_rebuilt_after_the_release_is_rebuilt_already_released`
    /// makes about `drop_resize_hold_for`, and for the same reason: a Lua
    /// layout here would be testing mlua and `tiling.lua`'s arithmetic
    /// rather than what the compositor does with the rectangle it is given.
    ///
    /// `duration: ZERO` is not a simplification. `tiling.lua` passes
    /// `{ duration = 0 }` for every frame of a seam drag, and the zero is
    /// load-bearing: a transform that finishes instantly is retired by
    /// `Solium::settle` after the very frame it was written on, which is why
    /// the layout's rectangle used to survive exactly one frame.
    fn tiled_frame(
        state: &mut Solium,
        request: &ResizeRequest,
        pane: crate::pane::PaneId,
        outer: Rectangle<i32, Logical>,
        now: Duration,
    ) {
        state.arm_resize_gesture(request);
        let was = state
            .pane_outer_of(pane)
            .expect("a mapped pane has a rectangle");
        state.move_pane(
            pane,
            outer,
            was,
            AnimationSpec {
                duration: Duration::ZERO,
                ..AnimationSpec::default()
            },
            now,
            Standing::Tile,
        );
        state.resize_gesture = None;
        // The claimed branch's own line: the bridge is watching every pane
        // the sweep moved, so a floating hold on this window would be a
        // second authority over one of them.
        state.drop_resize_hold_for(&request.window);
        // What `settle_resize` does after the layout has run. No-op while
        // the pointer is down, and included so the loop under test is the
        // loop that ships rather than a shorter one.
        paused_frame(state, now);
    }

    /// A frame of a live drag that carried **no** motion.
    ///
    /// The whole of what `settle_resize` does when `pending_resize` is
    /// empty, which is every frame the pointer is not moving on — including
    /// the ones at the end of a gesture, because people stop moving before
    /// they let go. The flush is the half `move_pane` cannot do: it is only
    /// reached from a frame that carried a motion, so the offers made in
    /// the last `TELL_EVERY` of travel have nowhere else to come from.
    fn paused_frame(state: &mut Solium, now: Duration) {
        state.flush_resize(now);
        state.settle_resize_hold(now);
        state.settle_resize_bridge(now);
    }

    /// Everything a tiled-drag test needs: a compositor, a client, a mapped
    /// window at a known place, and a clean configure log.
    ///
    /// Returned as a tuple rather than a struct because the borrow checker
    /// wants the queue and the client separately at every call site.
    macro_rules! tiled_fixture {
        ($display:ident, $state:ident, $conn:ident, $queue:ident, $client:ident, $qh:ident) => {
            let mut $display = Display::<Solium>::new().expect("creating a test wayland display");
            let mut $state = Solium::new($display.handle());
            // No decoration, for the reason `scale_resend` gives at length:
            // building a Qt scene inside a process that already holds a raw
            // libwayland connection aborts the whole test binary.
            $state
                .decorations
                .set_style(&mut $state.panes, Some("none".to_string()));
            let ($conn, mut $queue, mut $client) = connect(&mut $display, &mut $state);
            let $qh = $queue.handle();
        };
    }

    /// **#124 review, finding 2: a client that rounds must not move the
    /// seam on the first frame.**
    ///
    /// **The cross-boundary test, and the only one there is.** Every other
    /// test of this gesture lives on one side of the call or the other:
    /// `input::resize::dragged_edge_tests` is arithmetic on rectangles the
    /// test itself made up, and `solium_layout::tree::dragged_edge_tests`
    /// feeds the tree a number the same tree produced. Neither can see the
    /// link that actually broke — `Solium`'s rectangle for a pane against
    /// the tree's — because neither crosses it. This one does: a real
    /// `Tiling` is laid out, placed through `Solium::place` exactly as
    /// `tiling.apply` places it, disagreed with by a real client over the
    /// real protocol, and then the number the compositor would hand a
    /// layout on the first frame of a drag is fed back into that same tree.
    ///
    /// What #124 shipped read the edge off `Solium::pane_outer`, which goes
    /// through `pane_geometry` and answers `real_geometry` for a mapped
    /// client with no hold live: the space's location paired with the size
    /// the *client* committed. So the first frame handed the layout the
    /// client's edge and `drag_seam` obediently moved the seam there,
    /// shifting the whole column by the client's rounding residue with the
    /// pointer still on the pixel it pressed.
    ///
    /// **Eight pixels, which is what makes it the nasty kind.** The client
    /// below answers a cell short, which is what every terminal does to
    /// every configure it is ever sent; `crate::resizing` puts the threshold
    /// for calling such an answer a refusal rather than a rounding at
    /// `max(asked / 20, CELL)`, so this is deliberately *under* it — the
    /// compositor is meant to accept it and does. It is at or above the
    /// half-gap that was the whole of #120, and it is silent.
    ///
    /// **A right edge, because a left edge cannot show it.** `real.loc` is
    /// compositor-set and `real.size` is the client's, so `pane_outer`'s
    /// left and top edges are exact and its right and bottom carry the whole
    /// of the disagreement. A version of this test on `Edge::Left` passes
    /// against the defect.
    ///
    /// **Off-centre first**, for the reason
    /// `solium_layout::tree::dragged_edge_tests::a_window_handed_its_own_edge_does_not_move`
    /// gives at length: a fixture at `split: 0.5` is where a skew is least
    /// visible, and a test that cannot fail is not evidence. The seam is
    /// dragged somewhere lopsided and re-placed before the client is ever
    /// asked to disagree.
    ///
    /// Confirmed failing against `ec1da24` rather than assumed: the value
    /// that commit would have sent is computed here from the same live
    /// compositor state and fed into a clone of the same tree, and the
    /// second half of this test pins that it moves the window. Both halves
    /// are needed — the first alone would pass on a tree that ignored
    /// `drag_seam` entirely.
    #[test]
    fn a_client_that_rounds_its_size_does_not_move_the_seam() {
        use solium_layout::tree::{Edge, Tiling};

        tiled_fixture!(display, state, conn, queue, client, qh);
        let (left, _left_toplevel, left_surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        let (right, _right_toplevel, _right_surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&left)
            .expect("a client in the space has a pane");
        let (left_id, right_id) = (state.window_id(&left), state.window_id(&right));

        let (area, settings) = (tiled_area(), tiled_settings());
        let mut tiling = Tiling::new();
        tiling.insert(left_id, None, None, area, settings);
        tiling.insert(right_id, Some(left_id), None, area, settings);
        sweep(&mut state, &tiling);

        // Lopsided, so a skew that is self-cancelling at the midpoint of
        // the seam's box cannot hide in this fixture.
        tiling.drag_seam(left_id, Edge::Right, (340.0, 300.0), area, settings);
        sweep(&mut state, &tiling);

        let slot = leaf_of(&tiling, left_id);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a rect from a layout is screen-sized, and `Solium::place` \
                      rounds these same numbers the same way"
        )]
        let asked = at(slot.x as i32, slot.y as i32, slot.w as i32, slot.h as i32);

        // First the obedient client, which is the round trip this whole
        // test rests on: what `tree:layout` returned, through `sol.place`,
        // is what the pane's rectangle becomes -- so `pane_outer` and the
        // layout's own answer agree whenever a client does as it is told.
        // The defect below is entirely about the case where one does not.
        commit_buffer(&client, &qh, &left_surface, asked.size.w, asked.size.h);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        state.sync_panes();
        assert_eq!(
            state.pane_outer_of(pane),
            Some(asked),
            "a client at the size it was asked for puts `pane_outer` on \
             the layout's own rectangle"
        );

        // And now the same client answering a cell short. This is the one
        // thing no rectangle arithmetic can fake, and the whole reason this
        // test needs a real client.
        let short = asked.size.w - 8;
        commit_buffer(&client, &qh, &left_surface, short, asked.size.h);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        state.sync_panes();
        assert_eq!(
            state.pane_outer_of(pane).map(|outer| outer.size.w),
            Some(short),
            "the client committed a size of its own and the compositor \
             took it -- if it had not, this test would be checking nothing"
        );

        // The drag begins. `begin_resize` is what both grab sites call
        // first, and its answer is `began` -- the client's rectangle.
        let began = state.begin_resize(&left).expect("a mapped pane");
        let laid_out = state
            .pane_laid_out(&left)
            .expect("a pane a layout has placed");

        // The first frame: the pointer has not moved.
        let grab: Point<f64, Logical> = (f64::from(began.loc.x + began.size.w) - 3.0, 300.0).into();
        let sent = crate::input::resize::dragged_edge(laid_out, ResizeEdge::Right, grab, grab);

        let mut unmoved = tiling.clone();
        unmoved.drag_seam(left_id, Edge::Right, sent, area, settings);
        let after = leaf_of(&unmoved, left_id);
        assert!(
            (after.x - slot.x).abs() < 0.5
                && (after.y - slot.y).abs() < 0.5
                && (after.w - slot.w).abs() < 0.5
                && (after.h - slot.h).abs() < 0.5,
            "a drag that has not moved must not move the seam. The layout \
             is handed {sent:?}; its own edge is at {:?}. {slot:?} became \
             {after:?}",
            (slot.x + slot.w, slot.y + slot.h)
        );

        // And what `ec1da24` sent, from the same state, into the same
        // tree. `began` is the rectangle that commit derived its edge from,
        // and it is a different rectangle from `laid_out` by exactly the
        // eight pixels the client kept -- asserted, because if they were
        // ever equal the half below would be testing the same thing twice.
        assert_ne!(
            began, laid_out.0,
            "the client's rectangle and the layout's have to differ here, \
             or the defect this test is about cannot arise"
        );
        let shipped = crate::input::resize::dragged_edge(
            crate::input::resize::LaidOut(began),
            ResizeEdge::Right,
            grab,
            grab,
        );
        let mut moved = tiling.clone();
        moved.drag_seam(left_id, Edge::Right, shipped, area, settings);
        let jumped = leaf_of(&moved, left_id);
        assert!(
            (jumped.w - slot.w).abs() > 4.0,
            "reading the edge off the client's rectangle moved the seam by \
             the client's rounding on frame one, which is the defect: \
             {slot:?} became {jumped:?}"
        );
    }

    /// Everything a #133 test needs after the fixture: one window in a
    /// one-leaf tree, swept through `Solium::place` as `tiling.apply`
    /// would, with every finished transform retired so that what is drawn
    /// is what the pane's own geometry says -- the state a desktop is in
    /// between gestures. Returns the pane and the tile it was given.
    fn tiled_alone(
        state: &mut Solium,
        window: &Window,
    ) -> (crate::pane::PaneId, Rectangle<i32, Logical>) {
        use solium_layout::tree::Tiling;

        state.sync_panes();
        let pane = state
            .panes
            .id_of(window)
            .expect("a client in the space has a pane");
        let mut tiling = Tiling::new();
        tiling.insert(
            state.window_id(window),
            None,
            None,
            tiled_area(),
            tiled_settings(),
        );
        sweep(state, &tiling);
        let tile = state
            .panes
            .get(pane)
            .and_then(Pane::placed)
            .expect("a pane the layout has just placed is in a tile");
        state.settle(state.clock.now());
        (pane, tile)
    }

    /// A frame's worth of bookkeeping after a client has committed: the
    /// space into the panes, and finished transforms retired.
    fn a_frame(state: &mut Solium) {
        state.sync_panes();
        state.settle(state.clock.now());
    }

    /// **#133: a tiled client that commits more than its tile is held
    /// inside it**, and the hit test stops where the tile does.
    ///
    /// Two windows side by side, and the left one's client answers with a
    /// buffer 120 pixels wider than its tile and 8 shorter -- a browser at
    /// its minimum width, with a terminal's cell-grid residue on the other
    /// axis. On stage `pane_geometry` answered the committed size, so the
    /// pane, its frame canvas and its hit test all reached 120 pixels into
    /// the right-hand window, and a press there went to the left one.
    ///
    /// The left window is raised first, so that it is the one a hit test
    /// meets first. Without that the right window would win the overlap by
    /// being on top, and the last two assertions would pass on stage.
    #[test]
    fn a_tiled_client_that_commits_more_than_its_tile_is_held_inside_it() {
        use solium_layout::tree::Tiling;

        tiled_fixture!(display, state, conn, queue, client, qh);
        let (left, _left_toplevel, left_surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        let (right, _right_toplevel, right_surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&left)
            .expect("a client in the space has a pane");
        let beside_pane = state
            .panes
            .id_of(&right)
            .expect("a client in the space has a pane");
        let (left_id, right_id) = (state.window_id(&left), state.window_id(&right));

        let (area, settings) = (tiled_area(), tiled_settings());
        let mut tiling = Tiling::new();
        tiling.insert(left_id, None, None, area, settings);
        tiling.insert(right_id, Some(left_id), None, area, settings);
        sweep(&mut state, &tiling);
        let placed = |state: &Solium, pane| {
            state
                .panes
                .get(pane)
                .and_then(Pane::placed)
                .expect("a pane the layout has placed is in a tile")
        };
        let (tile, beside) = (placed(&state, pane), placed(&state, beside_pane));
        assert!(
            beside.loc.x > tile.loc.x + tile.size.w,
            "the fixture is two tiles side by side, left then right: {tile:?}, {beside:?}"
        );

        // The right-hand client does as it is told, so its tile is all
        // its own; the left one does not.
        commit_buffer(&client, &qh, &right_surface, beside.size.w, beside.size.h);
        let wide = (tile.size.w + 120, tile.size.h - 8);
        commit_buffer(&client, &qh, &left_surface, wide.0, wide.1);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        state.space.raise_element(&left, false);
        a_frame(&mut state);
        assert_eq!(
            left.geometry().size,
            Size::from(wide),
            "the client really did commit more than its tile, or there is \
             nothing here to hold"
        );

        assert_eq!(
            state.pane_outer_of(pane),
            Some(Rectangle::new(tile.loc, (tile.size.w, wide.1).into())),
            "cut to the tile across, where the client is too wide, and left \
             at the client's own height, where it is short of the tile"
        );
        assert_eq!(
            state.pane_laid_out(&left).map(|laid_out| laid_out.0),
            Some(tile),
            "a tiled edge drag still starts from the layout's rectangle (#124)"
        );
        let drawn = state.drawn(pane, state.pane_outer_of(pane).expect("the pane is here"));
        assert!(
            (drawn.rect.size.w - f64::from(tile.size.w)).abs() < 0.5,
            "and drawn at the tile's width, which is what the frame canvas \
             and the titlebar are sized from: {drawn:?}"
        );

        // Inside the right-hand tile, and inside the left client's
        // committed width.
        let point: Point<f64, Logical> =
            (f64::from(beside.loc.x + 20), f64::from(beside.loc.y + 100)).into();
        assert!(
            point.x < f64::from(tile.loc.x + wide.0),
            "the point has to be one the oversized client would reach"
        );
        assert_eq!(
            state.window_under(point).map(|(window, _)| window),
            Some(right.clone()),
            "a press in the right-hand tile is the right-hand window's, \
             whatever the left one committed"
        );
        let right_surface = right
            .toplevel()
            .map(|toplevel| toplevel.wl_surface().clone());
        assert_eq!(
            state.surface_under(point).map(|(surface, _)| surface),
            right_surface,
            "and so is the pointer's motion there"
        );
    }

    /// **Maximised is not tiled**: the work area a maximise configures is
    /// not cut down to the tile the window left.
    ///
    /// `Pane::placed` is the tile a client is held inside, and on stage
    /// `toggle_maximize` never touched it -- harmless while nothing read
    /// it but #124's edge drag. With the cap it is a maximised window
    /// drawn in the corner its tile used to occupy.
    #[test]
    fn a_maximised_window_is_not_held_in_its_old_tile() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let output = one_screen(&mut state);
        let screen = state
            .space
            .output_geometry(&output)
            .expect("the monitor is mapped");
        let (window, toplevel, surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        let (pane, tile) = tiled_alone(&mut state, &window);
        assert!(
            tile.size.w < screen.size.w && tile.size.h < screen.size.h,
            "the tile has to be smaller than the monitor, or a cap to it \
             would cut nothing: {tile:?}"
        );

        state.toggle_maximize(&window);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(
            last_configured(&client, &toplevel),
            Some((screen.size.w, screen.size.h)),
            "the client was asked for the work area"
        );
        commit_buffer(&client, &qh, &surface, screen.size.w, screen.size.h);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        a_frame(&mut state);

        assert_eq!(
            state.panes.get(pane).and_then(Pane::placed),
            None,
            "a maximised window is in no tile"
        );
        assert_eq!(
            state.pane_outer_of(pane),
            Some(screen),
            "and it is the size it was maximised to, not the tile it left"
        );
    }

    /// **And the way back puts it in the tile again**, so a tiled edge drag
    /// started from it begins at the layout's rectangle (#124).
    ///
    /// The client answers a cell short on the way back, the way a terminal
    /// does, so that the layout's rectangle and the pane's own differ: a
    /// window that came back in no tile would have `pane_laid_out` fall
    /// back to the client's rectangle, and a drag begun from it would move
    /// the seam by the client's rounding on its first frame.
    #[test]
    fn a_restored_window_goes_back_into_its_tile() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let output = one_screen(&mut state);
        let screen = state
            .space
            .output_geometry(&output)
            .expect("the monitor is mapped");
        let (window, _toplevel, surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        let (pane, tile) = tiled_alone(&mut state, &window);

        state.toggle_maximize(&window);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        commit_buffer(&client, &qh, &surface, screen.size.w, screen.size.h);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        a_frame(&mut state);

        state.toggle_maximize(&window);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        let short = tile.size.w - 8;
        commit_buffer(&client, &qh, &surface, short, tile.size.h);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        a_frame(&mut state);

        assert_eq!(
            state.panes.get(pane).and_then(Pane::placed),
            Some(tile),
            "the un-maximise put the window back in the tile it left"
        );
        assert_eq!(
            state.pane_outer_of(pane).map(|outer| outer.size.w),
            Some(short),
            "the client kept a cell short of its tile, so the two \
             rectangles below can be told apart"
        );
        assert_eq!(
            state.pane_laid_out(&window).map(|laid_out| laid_out.0),
            Some(tile),
            "and an edge drag starts from the layout's rectangle, not the \
             client's"
        );
    }

    /// **Fullscreen is not tiled either**, and leaving it goes back into
    /// the tile.
    ///
    /// The same field and the same stage behaviour as the maximise above:
    /// `fullscreen_request` kept a way back and moved the window, and left
    /// `Pane::placed` saying it was still in its tile -- which with the cap
    /// is a video cut down to the corner of the monitor it was tiled in.
    #[test]
    fn a_fullscreen_window_is_not_held_in_its_old_tile() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let output = one_screen(&mut state);
        let screen = state
            .space
            .output_geometry(&output)
            .expect("the monitor is mapped");
        let (window, toplevel, surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        let (pane, tile) = tiled_alone(&mut state, &window);

        toplevel.set_fullscreen(None);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(
            last_configured(&client, &toplevel),
            Some((screen.size.w, screen.size.h)),
            "the client was asked for the whole monitor"
        );
        commit_buffer(&client, &qh, &surface, screen.size.w, screen.size.h);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        a_frame(&mut state);

        assert_eq!(state.panes.get(pane).and_then(Pane::placed), None);
        assert_eq!(
            state.pane_outer_of(pane),
            Some(screen),
            "a fullscreen window covers its monitor, not the tile it left"
        );

        toplevel.unset_fullscreen();
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(
            state.panes.get(pane).and_then(Pane::placed),
            Some(tile),
            "and leaving fullscreen puts it back in that tile"
        );
    }

    /// **A window a layout lets go of is not held in the tile it had.**
    ///
    /// `sol.unplace` is what `modes.use` sends for every window when the
    /// layout in charge changes: the mode is the script's, so this is the
    /// only way the compositor hears that a window is floating now. Without
    /// it, a window left in its tile by a switch to floating is cut back to
    /// that tile the moment its client commits more -- which a floating
    /// window does whenever it is resized or grows itself.
    #[test]
    fn a_window_let_go_by_its_layout_is_not_held_in_its_old_tile() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, _toplevel, surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        let (pane, tile) = tiled_alone(&mut state, &window);

        state.apply(Outcome {
            commands: vec![Command::Unplace {
                id: state.window_id(&window),
            }],
            ..Outcome::default()
        });
        let grown = (tile.size.w + 200, tile.size.h + 50);
        commit_buffer(&client, &qh, &surface, grown.0, grown.1);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        a_frame(&mut state);

        assert_eq!(state.panes.get(pane).and_then(Pane::placed), None);
        assert_eq!(
            state.pane_outer_of(pane).map(|outer| outer.size),
            Some(Size::from(grown)),
            "a floating window is the size its client committed"
        );
    }

    /// **A dialog a layout centres is placed and not tiled**, through
    /// `sol.place` with `tile = false`.
    ///
    /// A dialog is floating whichever layout is running -- `dialogs.lua`
    /// centres it over its parent at the size it had -- and one that grows
    /// after it was centred, as a file chooser settling on its size does,
    /// must be drawn at the size it grew to. A layout's ordinary placement
    /// beside it is still a tile, which the last assertion holds the
    /// fixture to.
    #[test]
    fn a_dialog_a_layout_centres_is_not_held_in_its_rect() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, _toplevel, surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&window)
            .expect("a client in the space has a pane");
        let id = state.window_id(&window);
        let rect = Rect {
            x: 300.0,
            y: 200.0,
            w: 400.0,
            h: 300.0,
        };
        let instant = AnimationSpec {
            duration: Duration::ZERO,
            ..AnimationSpec::default()
        };
        state.apply(Outcome {
            commands: vec![Command::Place {
                id,
                rect,
                animation: instant,
                tile: false,
                inside: None,
                cramped: false,
            }],
            ..Outcome::default()
        });
        commit_buffer(&client, &qh, &surface, 520, 380);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        a_frame(&mut state);

        assert_eq!(state.panes.get(pane).and_then(Pane::placed), None);
        assert_eq!(
            state.pane_outer_of(pane),
            Some(at(300, 200, 520, 380)),
            "placed where the layout said, at the size the client grew to"
        );

        state.apply(Outcome {
            commands: vec![Command::Place {
                id,
                rect,
                animation: instant,
                tile: true,
                inside: None,
                cramped: false,
            }],
            ..Outcome::default()
        });
        a_frame(&mut state);
        assert_eq!(
            state.pane_outer_of(pane),
            Some(at(300, 200, 400, 300)),
            "the same rect as a tile holds the same client inside it"
        );
    }

    /// **A pane under a resize hold is bridged, not cut to its tile.**
    ///
    /// While an edge is being dragged the dragged rectangle is the truth
    /// and the client's last buffer is stretched or held into it by
    /// `resizing::factor` (#113, #123). The tile the drag is moving is
    /// `Pane::placed` all the while, so a cut to it would cut the bridge:
    /// `Solium::tile_of` answers `None` under a hold, and the committed
    /// size is shown whole. Here the client is larger than the rectangle
    /// the drag has reached, which is exactly the case a cut would change.
    #[test]
    fn a_held_pane_is_bridged_and_not_cut_to_its_tile() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, _toplevel, surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.map_stacked(window.clone(), (400, 300), false);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&window)
            .expect("a client in the space has a pane");
        commit_buffer(&client, &qh, &surface, 400, 300);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        state.sync_panes();

        let outer = at(400, 300, 200, 150);
        let request = ResizeRequest {
            window: window.clone(),
            wanted: outer,
            edge_at: (600.0, 450.0),
            edges: ResizeEdge::BottomRight,
        };
        state.begin_resize(&window);
        tiled_frame(&mut state, &request, pane, outer, Duration::ZERO);
        assert!(state.holding_resize(pane), "the drag is holding the pane");
        assert_eq!(
            state.panes.get(pane).and_then(Pane::placed),
            Some(outer),
            "and the layout has it in the dragged tile, which a cut would use"
        );

        let held = state.panes.get(pane).expect("the pane is still here");
        assert_eq!(
            state.shown_size(held, window.geometry().size),
            Size::from((400, 300)),
            "the whole of the client's last buffer is shown, to be bridged \
             into the dragged rectangle rather than cut to it"
        );
        assert_eq!(
            state.pane_geometry(held),
            outer,
            "and the pane is the dragged rectangle, as #113 made it"
        );
    }

    /// **A floating window brought back onto a screen is still floating.**
    ///
    /// `rescue_offscreen` reaches `move_pane`, which is where a layout's
    /// tile is written -- so on the way to #133 a window lost off a
    /// departed monitor came back *tiled* at the rectangle it was rescued
    /// to, and was cut to it from then on. A rescue is not a layout's
    /// opinion, and keeps a pane's standing as it found it.
    #[test]
    fn a_floating_window_brought_back_onto_a_screen_is_not_given_a_tile() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let _output = one_screen(&mut state);
        let (window, _toplevel, surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.map_stacked(window.clone(), (-5000, 300), false);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&window)
            .expect("a client in the space has a pane");

        state.rescue_offscreen();
        assert!(
            state
                .pane_outer_of(pane)
                .is_some_and(|outer| state.on_any_output(outer)),
            "the rescue brought the window back, or it proves nothing"
        );
        commit_buffer(&client, &qh, &surface, 700, 500);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        a_frame(&mut state);

        assert_eq!(state.panes.get(pane).and_then(Pane::placed), None);
        assert_eq!(
            state.pane_outer_of(pane).map(|outer| outer.size),
            Some(Size::from((700, 500))),
            "and it is the size its client committed"
        );
    }

    /// `window` placed again, through `Solium::place` as `tiling.apply`
    /// places it, at `rect` and gliding there for `duration`.
    fn glide(
        state: &mut Solium,
        window: &Window,
        rect: Rectangle<i32, Logical>,
        duration: Duration,
        easing: present::Curve,
        start: Duration,
    ) {
        let id = state.window_id(window);
        state.place(
            id,
            to_rect(rect),
            AnimationSpec { duration, easing },
            start,
            Standing::Tile,
        );
    }

    /// A window alone in a tile whose client filled it, which is every
    /// tiled window a moment before its neighbour opens.
    fn filled_tile(
        display: &mut Display<Solium>,
        state: &mut Solium,
        conn: &Connection,
        qh: &QueueHandle<Client>,
        queue: &mut wayland_client::EventQueue<Client>,
        client: &mut Client,
    ) -> (
        Window,
        wl_surface::WlSurface,
        crate::pane::PaneId,
        Rectangle<i32, Logical>,
    ) {
        let (window, _toplevel, surface) = open_surface(display, state, conn, client, qh);
        let (pane, tile) = tiled_alone(state, &window);
        commit_buffer(client, qh, &surface, tile.size.w, tile.size.h);
        pump(display, state, conn, qh, queue, client);
        a_frame(state);
        assert_eq!(
            window.geometry().size,
            tile.size,
            "the client filled its tile, so the frame before the sweep is \
             the whole buffer at 1:1"
        );
        (window, surface, pane, tile)
    }

    /// **#133 review, finding 1: a glide that narrows a tiled window is
    /// drawn 1:1, cut to the rectangle it has reached, from its first
    /// frame.**
    ///
    /// The case that made the finding: a window alone in its tile, a
    /// second one opening, and the layout halving the first with a 240ms
    /// glide while its client still has the whole width committed. The
    /// cap takes the pane to its new tile at once, so the glide's first
    /// frame is drawn at the old tile while `pane_outer` is the new one --
    /// and what was shipped read the pair as the new tile zoomed 2x, so
    /// the left half of the buffer was stretched over the whole of the old
    /// tile on the frame after one that had drawn all of it 1:1.
    ///
    /// Driven through the real placement and the real transform, and read
    /// through `render::place_client`, which is what `elements` draws the
    /// surfaces with and what `offscreen::capture_client` sizes a masked
    /// client from. Three frames: the first, one a little way in, and one
    /// most of the way.
    #[test]
    fn a_glide_that_narrows_a_tiled_window_is_drawn_1_to_1_on_its_first_frame() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, _surface, pane, tile) = filled_tile(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );

        let half = Rectangle::new(tile.loc, (tile.size.w / 2, tile.size.h).into());
        let start = state.clock.now();
        glide(
            &mut state,
            &window,
            half,
            Duration::from_millis(240),
            present::Curve::OutCubic,
            start,
        );
        let outer = state.pane_outer_of(pane).expect("the pane is here");
        assert_eq!(
            outer, half,
            "the pane is held in its new tile at once, which is what the \
             glide's frames are measured against"
        );
        let committed = window.geometry().size;
        assert_eq!(committed, tile.size, "and the client has not answered");

        let placed_at = |state: &Solium, at: Duration| {
            let held = state.panes.get(pane).expect("the pane is here");
            let frame = state.drawn_at(held, outer, start + at);
            (
                frame,
                crate::render::place_client(state, held, &frame, outer.size, committed),
            )
        };

        let (first, drawn) = placed_at(&state, Duration::ZERO);
        assert_eq!(
            first.rect,
            tile.to_f64(),
            "the glide starts at the old tile"
        );
        assert_eq!(
            drawn.fit.factor,
            smithay::utils::Scale::from((1.0, 1.0)),
            "its first frame is the whole buffer at its own size, as the frame \
             before the sweep was -- not the new tile zoomed"
        );
        assert_eq!(drawn.fit.crop, None, "and nothing of it is cut yet");

        for at in [Duration::from_millis(30), Duration::from_millis(200)] {
            let (frame, drawn) = placed_at(&state, at);
            assert!(
                frame.rect.size.w < f64::from(tile.size.w) - 1.0
                    && frame.rect.size.w > f64::from(half.size.w) + 1.0,
                "at {at:?} the frame is part of the way, or it proves nothing: {frame:?}"
            );
            assert_eq!(
                drawn.fit.factor,
                smithay::utils::Scale::from((1.0, 1.0)),
                "at {at:?} the buffer is still drawn at its own size"
            );
            assert_eq!(
                drawn.fit.crop,
                Some(drawn.client),
                "at {at:?} it is cut to the rectangle the glide has reached"
            );
            #[expect(clippy::cast_possible_truncation, reason = "a screen-sized rect")]
            let reached = drawn.client.size.w.round() as i32;
            assert_eq!(
                drawn.fit.shown,
                Size::from((reached, committed.h)),
                "at {at:?} a masked client is captured at the same cut, not at \
                 the new tile's share"
            );
        }
    }

    /// **And a press on a gliding window lands on the pixel under it.**
    ///
    /// `surface_under` used to map a point through `to_window_space`,
    /// which reads the drawn rectangle as a scale of the pane's own. For
    /// the glide above that is the new tile zoomed, so a press 600 pixels
    /// into the drawn window reached the client 600 * 488 / 732 = 400
    /// pixels in -- the zoom's picture, which is no longer what is drawn.
    /// It goes through `render::place_client` now, and the glide here is
    /// ten seconds long and linear so that the few milliseconds this test
    /// takes to run cannot move it anywhere that matters.
    #[test]
    fn a_press_on_a_gliding_window_lands_on_the_pixel_under_it() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, _surface, pane, tile) = filled_tile(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );

        let half = Rectangle::new(tile.loc, (tile.size.w / 2, tile.size.h).into());
        let start = state.clock.now();
        glide(
            &mut state,
            &window,
            half,
            Duration::from_secs(10),
            present::Curve::Linear,
            start,
        );
        state.clock.advance(Duration::from_secs(5));

        let into = 600;
        let point: Point<f64, Logical> =
            (f64::from(tile.loc.x + into), f64::from(tile.loc.y + 100)).into();
        let outer = state.pane_outer_of(pane).expect("the pane is here");
        let frame = state.drawn(pane, outer);
        assert!(
            frame.covers(point) && point.x > f64::from(half.loc.x + half.size.w),
            "the point is on the gliding window and past its new tile: \
             {frame:?}, {point:?}"
        );

        let (surface, origin) = state
            .surface_under(point)
            .expect("the gliding window is under the point");
        assert_eq!(
            Some(surface),
            window
                .toplevel()
                .map(|toplevel| toplevel.wl_surface().clone())
        );
        let within = point - origin;
        assert!(
            (within.x - f64::from(into)).abs() < 0.5 && (within.y - 100.0).abs() < 0.5,
            "the client is told the pixel the picture has under the pointer, \
             {into},100 at 1:1, and was told {within:?}"
        );
    }

    /// **#133 review, finding 5: a drag on a maximised tiled window starts
    /// from its tile.**
    ///
    /// A maximise takes the window out of `Pane::placed`, so that the
    /// work area is not cut to the corner it was tiled in -- but it stays
    /// a leaf of its tree, and nothing on the drag path asks about
    /// maximised. `pane_laid_out` fell back to `pane_outer`, the whole
    /// monitor, and the seam was handed the screen's far edge on the first
    /// frame of a drag that had not moved. On stage `placed` still held
    /// the tile. This is `a_client_that_rounds_its_size_does_not_move_the_seam`'s
    /// drag, begun on a maximised window.
    #[test]
    fn a_drag_on_a_maximised_tiled_window_starts_from_its_tile() {
        use solium_layout::tree::{Edge, Tiling};

        tiled_fixture!(display, state, conn, queue, client, qh);
        let output = one_screen(&mut state);
        let screen = state
            .space
            .output_geometry(&output)
            .expect("the monitor is mapped");
        let (left, _left_toplevel, left_surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        let (right, _right_toplevel, _right_surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.sync_panes();
        let (left_id, right_id) = (state.window_id(&left), state.window_id(&right));
        let (area, settings) = (tiled_area(), tiled_settings());
        let mut tiling = Tiling::new();
        tiling.insert(left_id, None, None, area, settings);
        tiling.insert(right_id, Some(left_id), None, area, settings);
        sweep(&mut state, &tiling);
        let slot = leaf_of(&tiling, left_id);

        state.toggle_maximize(&left);
        commit_buffer(&client, &qh, &left_surface, screen.size.w, screen.size.h);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        a_frame(&mut state);
        let pane = state.panes.id_of(&left).expect("the pane is here");
        assert_eq!(
            state.pane_outer_of(pane),
            Some(screen),
            "the window is maximised, and not held in its tile"
        );

        let began = state.begin_resize(&left).expect("a mapped pane");
        let laid_out = state.pane_laid_out(&left).expect("a mapped pane");
        let grab: Point<f64, Logical> = (f64::from(began.loc.x + began.size.w) - 3.0, 300.0).into();
        let sent = crate::input::resize::dragged_edge(laid_out, ResizeEdge::Right, grab, grab);
        let mut unmoved = tiling.clone();
        unmoved.drag_seam(left_id, Edge::Right, sent, area, settings);
        let after = leaf_of(&unmoved, left_id);
        assert!(
            (after.x - slot.x).abs() < 0.5 && (after.w - slot.w).abs() < 0.5,
            "a drag that has not moved left the seam where it was: {slot:?} \
             became {after:?}, from an edge of {sent:?}"
        );
    }

    /// **#133 review, finding 9: a tiled window brought back onto a screen
    /// is still tiled**, at the rectangle it was brought back to -- the
    /// half of `Standing::Kept` that `a_floating_window_brought_back_onto_a_screen_is_not_given_a_tile`
    /// cannot see. A client that then commits more than that rectangle is
    /// held inside it.
    #[test]
    fn a_tiled_window_brought_back_onto_a_screen_is_still_tiled() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let _output = one_screen(&mut state);
        let (window, _toplevel, surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        let (pane, tile) = tiled_alone(&mut state, &window);
        state.map_stacked(window.clone(), (-5000, 300), false);
        state.sync_panes();

        state.rescue_offscreen();
        let rescued = state.pane_outer_of(pane).expect("the pane is here");
        assert!(
            state.on_any_output(rescued),
            "the rescue brought the window back, or it proves nothing"
        );
        assert_eq!(
            state.panes.get(pane).and_then(Pane::placed),
            Some(rescued),
            "still in a tile, at the rectangle it was brought back to"
        );

        commit_buffer(&client, &qh, &surface, tile.size.w + 200, tile.size.h + 50);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        a_frame(&mut state);
        assert_eq!(
            state.pane_outer_of(pane),
            Some(rescued),
            "and a client that grows is held inside it"
        );
    }

    /// **And a maximised one brings the tile it left along with it.**
    ///
    /// A window maximised on a monitor that goes away is rescued onto the
    /// one that is left; the tile it is waiting to go back into was on the
    /// departed monitor too, and `pane_laid_out` answers it for a drag on
    /// the maximised window (see
    /// `a_drag_on_a_maximised_tiled_window_starts_from_its_tile`). Left
    /// behind, that drag would start from a rectangle on no screen.
    #[test]
    fn a_maximised_window_brought_back_onto_a_screen_brings_its_tile() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let _left = a_screen(&mut state, "rescue-left", (0, 0));
        let right = a_screen(&mut state, "rescue-right", (1920, 0));
        let (window, _toplevel, _surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.sync_panes();
        let pane = state.panes.id_of(&window).expect("the pane is here");
        let tile = at(2000, 100, 800, 600);
        let now = state.clock.now();
        glide(
            &mut state,
            &window,
            tile,
            Duration::ZERO,
            present::Curve::Linear,
            now,
        );
        state.toggle_maximize(&window);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        a_frame(&mut state);
        assert_eq!(
            state.panes.get(pane).and_then(Pane::left_tile),
            Some(tile),
            "maximised out of the tile on the right-hand monitor"
        );

        state.space.unmap_output(&right);
        state.settle_monitors();
        let rescued = state.pane_outer_of(pane).expect("the pane is here");
        assert!(
            state.on_any_output(rescued),
            "the maximised window was brought back: {rescued:?}"
        );

        let laid_out = state.pane_laid_out(&window).map(|laid_out| laid_out.0);
        assert_eq!(
            laid_out,
            Some(at(1920 - 800, 100, 800, 600)),
            "the tile it will go back into was brought back the way a window \
             is: onto the screen that is left, its size kept, clamped at the \
             edge it was beyond"
        );
    }

    /// **#133 review, finding 10: a tiled client is held inside its tile
    /// with its frame taken off.**
    ///
    /// Every other #133 test runs undecorated, where the client's share of
    /// a tile *is* the tile, so `Solium::tile_of` forgetting the insets
    /// passed all of them. A pane reserving a titlebar -- `Frame::Pending`,
    /// which needs no Qt scene -- has a share `TITLEBAR_HEIGHT` shorter
    /// than its tile, and a client committing the tile's full height has to
    /// be held to that share: taken as the tile, it reaches a titlebar's
    /// height into the tile below. Asserted of the pane, of the part of
    /// the buffer a masked client is captured at, and of the hit test.
    #[test]
    fn a_titled_tiled_client_is_held_inside_its_tile_frame_and_all() {
        use solium_layout::tree::Tiling;

        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, _toplevel, surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.sync_panes();
        let pane = state.panes.id_of(&window).expect("the pane is here");
        state
            .panes
            .get_mut(pane)
            .expect("the pane is here")
            .set_frame(crate::pane::Frame::Pending);
        assert_eq!(
            state.insets_of(pane).top,
            TITLEBAR_HEIGHT,
            "a titlebar's worth reserved, or this is the undecorated case again"
        );
        let mut tiling = Tiling::new();
        tiling.insert(
            state.window_id(&window),
            None,
            None,
            tiled_area(),
            tiled_settings(),
        );
        sweep(&mut state, &tiling);
        let tile = state
            .panes
            .get(pane)
            .and_then(Pane::placed)
            .expect("the pane is in a tile");
        a_frame(&mut state);

        commit_buffer(&client, &qh, &surface, tile.size.w + 120, tile.size.h);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        a_frame(&mut state);

        assert_eq!(
            state.pane_outer_of(pane),
            Some(tile),
            "the frame and the client together are exactly the tile"
        );
        let held = state.panes.get(pane).expect("the pane is here");
        let frame = state.drawn(pane, tile);
        let drawn =
            crate::render::place_client(&state, held, &frame, tile.size, window.geometry().size);
        assert_eq!(
            drawn.fit.shown,
            Size::from((tile.size.w, tile.size.h - TITLEBAR_HEIGHT)),
            "a masked client is captured at the tile's share, under its titlebar"
        );
        let below: Point<f64, Logical> = (
            f64::from(tile.loc.x + 40),
            f64::from(tile.loc.y + tile.size.h + 4),
        )
            .into();
        assert_eq!(
            state.surface_under(below).map(|(surface, _)| surface),
            None,
            "and a point just past the tile's bottom edge is not the window's"
        );
    }

    /// **#133 review, findings 3 and 6: a warped window is captured at the
    /// rectangle its warp is drawn over.**
    ///
    /// `warp::mesh` spreads the whole of the capture over the frame's
    /// rect, and that rect comes from `pane_outer_of`, which the cap holds
    /// to the tile. The capture was sized from `outer_geometry` -- the
    /// committed size -- so an oversized tiled client was squashed into
    /// its tile for the length of a genie or a tilt, and its frame was told
    /// the uncapped width while warped and the tile's once it landed.
    /// `render::flat` is what both `offscreen::capture` and
    /// `flat_window_elements` read, the texture size and the frame's
    /// `Drawing.outer` alike.
    #[test]
    fn a_warped_window_is_captured_at_the_rect_its_warp_is_drawn_over() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, _toplevel, surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        let (pane, tile) = tiled_alone(&mut state, &window);
        commit_buffer(&client, &qh, &surface, tile.size.w * 2, tile.size.h);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        a_frame(&mut state);
        assert_eq!(
            window.geometry().size.w,
            tile.size.w * 2,
            "the client really is twice as wide as its tile"
        );

        let flat = crate::render::flat(&state, &window).expect("the window is mapped");
        assert_eq!(
            Some(flat.outer),
            state.pane_outer_of(pane).map(|outer| outer.size),
            "captured at the rect the mesh is built over, so nothing is squashed"
        );
        assert_eq!(flat.outer, tile.size, "which is the tile");
        assert_eq!(flat.insets, state.insets_of(pane));
    }

    /// Open a popup on `parent` at `anchor` in its window's coordinates,
    /// `size` big, and give it a buffer -- which means acking its first
    /// configure, as xdg-shell requires before a popup commits anything.
    #[expect(clippy::too_many_arguments, reason = "a fixture's plumbing")]
    fn drawn_popup(
        display: &mut Display<Solium>,
        state: &mut Solium,
        conn: &Connection,
        qh: &QueueHandle<Client>,
        queue: &mut wayland_client::EventQueue<Client>,
        client: &mut Client,
        parent: &xdg_surface::XdgSurface,
        anchor: (i32, i32),
        size: (i32, i32),
    ) -> wl_surface::WlSurface {
        let compositor = client.compositor.clone().expect("wl_compositor bound");
        let wm_base = client.wm_base.clone().expect("xdg_wm_base bound");
        let surface = compositor.create_surface(qh, ());
        let xdg = wm_base.get_xdg_surface(&surface, qh, ());
        let positioner = wm_base.create_positioner(qh, ());
        positioner.set_size(size.0, size.1);
        positioner.set_anchor_rect(anchor.0, anchor.1, 1, 1);
        positioner.set_anchor(xdg_positioner::Anchor::TopLeft);
        positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
        let _popup = xdg.get_popup(Some(parent), &positioner, qh, ());
        surface.commit();
        pump(display, state, conn, qh, queue, client);

        let id = wayland_client::Proxy::id(&xdg);
        let serial = client
            .surface_configures
            .iter()
            .rev()
            .find(|(to, _)| *to == id)
            .map(|&(_, serial)| serial)
            .expect("the popup was configured");
        xdg.ack_configure(serial);
        commit_buffer(client, qh, &surface, size.0, size.1);
        pump(display, state, conn, qh, queue, client);
        surface
    }

    /// **#133 review, findings 2 and 8: a toplevel is drawn without its
    /// popups.**
    ///
    /// Every path that draws a client draws its popups itself --
    /// `elements` uncut above the sandwich, `flat_window_elements` into the
    /// warp's capture -- and smithay's `Window::render_elements` draws them
    /// *again*, ahead of the toplevel's own tree. On the ordinary path that
    /// second copy went through the toplevel's fit and was cut to the
    /// tile, one layer under the uncut one, so a translucent pixel of a
    /// menu crossing the tile's edge was blended twice on one side of it
    /// and once on the other. `render::toplevel_elements` is what all
    /// three paths build the toplevel from, and it is generic over the
    /// renderer so it runs here on smithay's `DummyRenderer`.
    ///
    /// The first assertion is the control: smithay's own call does have
    /// the popup in it, so the fixture has a popup that draws.
    #[test]
    fn a_toplevel_is_drawn_without_its_popups() {
        use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
        use smithay::backend::renderer::element::{Element as _, Id};
        use smithay::backend::renderer::test::DummyRenderer;

        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, _toplevel, _surface, xdg_surface) =
            open_xdg(&mut display, &mut state, &conn, &client, &qh);
        let _popup = drawn_popup(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
            &xdg_surface,
            (10, 10),
            (40, 30),
        );
        let toplevel = window
            .toplevel()
            .map(|toplevel| toplevel.wl_surface().clone())
            .expect("an xdg toplevel");
        let popup = smithay::desktop::PopupManager::popups_for_surface(&toplevel)
            .next()
            .map(|(popup, _)| popup.wl_surface().clone())
            .expect("the popup is tracked on its parent");
        let (own_id, popup_id) = (
            Id::from_wayland_resource(&toplevel),
            Id::from_wayland_resource(&popup),
        );

        let mut renderer = DummyRenderer;
        let scale = smithay::utils::Scale::from(1.0);
        let whole: Vec<WaylandSurfaceRenderElement<DummyRenderer>> =
            smithay::backend::renderer::element::AsRenderElements::render_elements(
                &window,
                &mut renderer,
                (0, 0).into(),
                scale,
                1.0,
            );
        assert!(
            whole.iter().any(|element| element.id() == &popup_id),
            "smithay's whole-window call draws the popup, so there is a popup \
             here to leave out"
        );

        let own =
            crate::render::toplevel_elements(&mut renderer, &window, (0, 0).into(), scale, 1.0);
        assert!(
            own.iter().any(|element| element.id() == &own_id),
            "the toplevel itself is drawn"
        );
        assert!(
            own.iter().all(|element| element.id() != &popup_id),
            "and its popup is not, because every caller draws that itself"
        );
    }

    /// **#133 review, finding 8: a menu past its parent's tile takes the
    /// press.**
    ///
    /// A popup is drawn uncut, reaching past its parent's tile, and since
    /// #133 the parent's hit-test rectangle stops at the tile. So a menu
    /// opened near the right edge of an oversized client drew items over
    /// the neighbouring tile that could not be pointed at: `surface_under`
    /// gated every pane on its own frame, the point fell through to the
    /// neighbour, and a press there dismisses the menu's grab instead of
    /// choosing the item. The window with the menu is raised, as a click
    /// that opened one leaves it.
    #[test]
    fn a_menu_past_its_parents_tile_takes_the_press() {
        use solium_layout::tree::Tiling;

        tiled_fixture!(display, state, conn, queue, client, qh);
        let (left, _left_toplevel, left_surface, left_xdg) =
            open_xdg(&mut display, &mut state, &conn, &client, &qh);
        let (right, _right_toplevel, right_surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.sync_panes();
        let (left_id, right_id) = (state.window_id(&left), state.window_id(&right));
        let (area, settings) = (tiled_area(), tiled_settings());
        let mut tiling = Tiling::new();
        tiling.insert(left_id, None, None, area, settings);
        tiling.insert(right_id, Some(left_id), None, area, settings);
        sweep(&mut state, &tiling);
        let pane = state.panes.id_of(&left).expect("the pane is here");
        let tile = state
            .panes
            .get(pane)
            .and_then(Pane::placed)
            .expect("the pane is in a tile");
        let beside = state
            .panes
            .id_of(&right)
            .and_then(|pane| state.panes.get(pane))
            .and_then(Pane::placed)
            .expect("the neighbour is in a tile");
        commit_buffer(&client, &qh, &right_surface, beside.size.w, beside.size.h);
        commit_buffer(&client, &qh, &left_surface, tile.size.w + 200, tile.size.h);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        let _popup = drawn_popup(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
            &left_xdg,
            (tile.size.w - 20, 50),
            (120, 80),
        );
        state.space.raise_element(&left, false);
        a_frame(&mut state);

        let point: Point<f64, Logical> = (
            f64::from(tile.loc.x + tile.size.w + 40),
            f64::from(tile.loc.y + 90),
        )
            .into();
        assert!(
            beside.to_f64().contains(point),
            "the point is over the neighbour's tile, where the menu reaches"
        );
        let menu = left.toplevel().and_then(|toplevel| {
            smithay::desktop::PopupManager::popups_for_surface(toplevel.wl_surface())
                .next()
                .map(|(popup, _)| popup.wl_surface().clone())
        });
        assert!(menu.is_some(), "the menu is tracked on its parent");
        assert_eq!(
            state.surface_under(point).map(|(surface, _)| surface),
            menu,
            "the menu has the pixel it drew, not the window under it"
        );
    }

    /// **A script's thumbnail and a script's open are pictures of the
    /// whole window**, for a tiled one as for any other.
    ///
    /// `Solium::apply` gives the frames `sol.present` and
    /// `sol.present_from` build a zoom of their rectangle over the
    /// window's own. That zoom is what tells `render::fit` the rectangle is
    /// the window made smaller, so a tiled window that fits its tile is
    /// scaled into an overview slot or an open's first frame whole, as it
    /// was before #133 -- where a frame at zoom 1.0 that small would read
    /// as a tile that narrow, and the window would be cut to its top-left
    /// corner instead.
    #[test]
    fn a_scripts_thumbnail_and_open_are_pictures_of_the_whole_window() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, _surface, pane, tile) = filled_tile(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        let id = state.window_id(&window);
        let instant = AnimationSpec {
            duration: Duration::ZERO,
            ..AnimationSpec::default()
        };
        let committed = window.geometry().size;
        let thumbnail = Rect {
            x: f64::from(tile.loc.x) + 40.0,
            y: f64::from(tile.loc.y) + 30.0,
            w: f64::from(tile.size.w) / 4.0,
            h: f64::from(tile.size.h) / 4.0,
        };
        let placed_now = |state: &Solium| {
            let held = state.panes.get(pane).expect("the pane is here");
            let outer = state.pane_outer(held);
            let frame = state.drawn(pane, outer);
            (
                frame,
                crate::render::place_client(state, held, &frame, outer.size, committed),
            )
        };

        state.apply(Outcome {
            commands: vec![Command::Present {
                id,
                rect: Some(thumbnail),
                opacity: None,
                matrix: None,
                deform: None,
                z: 0.0,
                pivot: (0.5, 0.5),
                animation: instant,
            }],
            ..Outcome::default()
        });
        let (frame, drawn) = placed_now(&state);
        assert_eq!(frame.zoom, (0.25, 0.25), "a quarter of the window");
        assert_eq!(
            (drawn.fit.factor, drawn.fit.crop),
            (smithay::utils::Scale::from((0.25, 0.25)), None),
            "the whole window at a quarter, with nothing cut off it"
        );

        state.apply(Outcome {
            commands: vec![Command::Clear {
                id,
                animation: instant,
            }],
            ..Outcome::default()
        });
        a_frame(&mut state);
        let shrunk = Rect {
            x: f64::from(tile.loc.x) + f64::from(tile.size.w) * 0.06,
            y: f64::from(tile.loc.y) + f64::from(tile.size.h) * 0.06,
            w: f64::from(tile.size.w) * 0.88,
            h: f64::from(tile.size.h) * 0.88,
        };
        let start = state.clock.now();
        state.apply(Outcome {
            commands: vec![Command::PresentFrom {
                id,
                rect: shrunk,
                opacity: Some(0.0),
                animation: AnimationSpec {
                    duration: Duration::from_secs(10),
                    easing: present::Curve::Linear,
                },
            }],
            ..Outcome::default()
        });
        let held = state.panes.get(pane).expect("the pane is here");
        let outer = state.pane_outer(held);
        let first = state.drawn_at(held, outer, start);
        let opened = crate::render::place_client(&state, held, &first, outer.size, committed);
        assert!(
            (first.zoom.0 - 0.88).abs() < 1e-9 && (first.zoom.1 - 0.88).abs() < 1e-9,
            "an open's first frame is the window at 0.88: {:?}",
            first.zoom
        );
        assert!(
            (opened.fit.factor.x - 0.88).abs() < 1e-9 && opened.fit.crop.is_none(),
            "the whole window at 0.88, with nothing cut off it: {:?}",
            opened.fit
        );
    }

    /// **A press on a window lands where its client is drawn, frame and
    /// all.**
    ///
    /// `elements` places the client under the frame's share from
    /// `insets_of`, which answers for a `Frame::Pending` pane -- one still
    /// reserving the titlebar a frame is on its way to fill. The hit test
    /// took its inset from `frame_insets`, which answers nothing for that
    /// pane because it is not decorated yet, so a press there reached the
    /// client a titlebar's height above the pixel under it. Both read the
    /// one `render::place_client` now.
    #[test]
    fn a_press_on_a_window_reserving_a_titlebar_lands_on_the_pixel_under_it() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, _toplevel, surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.map_stacked(window.clone(), (400, 300), false);
        state.sync_panes();
        let pane = state.panes.id_of(&window).expect("the pane is here");
        state
            .panes
            .get_mut(pane)
            .expect("the pane is here")
            .set_frame(crate::pane::Frame::Pending);
        commit_buffer(&client, &qh, &surface, 400, 300);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        // Past the open animation its first commit started, which begins
        // at opacity zero -- and a pane nobody can see takes no press.
        state.clock.advance(Duration::from_secs(1));
        a_frame(&mut state);
        let outer = state.pane_outer_of(pane).expect("the pane is here");
        assert_eq!(
            outer,
            at(400, 300 - TITLEBAR_HEIGHT, 400, 300 + TITLEBAR_HEIGHT),
            "the client at 400,300 with a titlebar's worth reserved above it"
        );

        let point: Point<f64, Logical> = (450.0, 310.0).into();
        let (_, origin) = state
            .surface_under(point)
            .expect("the client is under the point");
        let within = point - origin;
        assert!(
            (within.x - 50.0).abs() < 0.5 && (within.y - 10.0).abs() < 0.5,
            "ten pixels into the client, as drawn, and was told {within:?}"
        );
    }

    /// **A press on a window whose drag is still being answered lands on
    /// the pixel its picture has there.**
    ///
    /// Under a resize hold the client's last buffer is stretched into the
    /// rectangle the drag has reached, and the hold outlives the gesture
    /// by up to `resizing::PATIENCE`. The hit test mapped the point 1:1
    /// against the dragged rectangle, so a press on the stretched picture
    /// reached the client somewhere else -- here, a buffer 400 wide drawn
    /// 200 wide, and a press 150 pixels in, which is pixel 300 of the
    /// picture and was delivered as pixel 150. It inverts the same fit the
    /// picture is drawn with now.
    #[test]
    fn a_press_on_a_held_window_lands_on_the_pixel_its_picture_has() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, _toplevel, surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.map_stacked(window.clone(), (400, 300), false);
        state.sync_panes();
        let pane = state.panes.id_of(&window).expect("the pane is here");
        commit_buffer(&client, &qh, &surface, 400, 300);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        state.sync_panes();

        let outer = at(400, 300, 200, 150);
        let request = ResizeRequest {
            window: window.clone(),
            wanted: outer,
            edge_at: (600.0, 450.0),
            edges: ResizeEdge::BottomRight,
        };
        state.begin_resize(&window);
        tiled_frame(&mut state, &request, pane, outer, Duration::ZERO);
        assert!(state.holding_resize(pane), "the drag is holding the pane");
        assert_eq!(
            state.resize_fill(pane),
            Some(crate::resizing::Fill::Stretch),
            "and stretching the last buffer into the dragged rectangle"
        );

        let point: Point<f64, Logical> = (550.0, 400.0).into();
        let (_, origin) = state
            .surface_under(point)
            .expect("the window is under the point");
        let within = point - origin;
        assert!(
            (within.x - 300.0).abs() < 0.5 && (within.y - 200.0).abs() < 0.5,
            "the picture has buffer pixel 300,200 there, and the client was \
             told {within:?}"
        );
    }

    /// The work area every tiled-tree fixture in this module lays out over.
    fn tiled_area() -> solium_layout::Rect {
        solium_layout::Rect::new(0.0, 0.0, 1000.0, 600.0)
    }

    /// The shipped gap, because at `gap: 0` the two sides of a seam are one
    /// line and half the arithmetic under test disappears.
    fn tiled_settings() -> solium_layout::Settings {
        solium_layout::Settings {
            gap: 12.0,
            split: 0.5,
            ..solium_layout::Settings::default()
        }
    }

    /// Where a tree has put one window. `rect_at` in the layout crate's own
    /// suite, which is not public.
    fn leaf_of(tiling: &solium_layout::tree::Tiling, id: u64) -> solium_layout::Rect {
        tiling
            .layout(tiled_area(), tiled_settings())
            .into_iter()
            .find(|(other, _)| *other == id)
            .expect("the window is in the tree")
            .1
    }

    /// One layout sweep, which is what `tiling.apply` is in Lua: every leaf
    /// placed, every frame, whether it moved or not.
    ///
    /// `sol.place` is `Solium::place`, and a script's rect is the pane's
    /// *outer* rectangle — `place` subtracts the insets itself.
    fn sweep(state: &mut Solium, tiling: &solium_layout::tree::Tiling) {
        for (id, rect) in tiling.layout(tiled_area(), tiled_settings()) {
            state.place(
                id,
                Rect {
                    x: rect.x,
                    y: rect.y,
                    w: rect.w,
                    h: rect.h,
                },
                AnimationSpec {
                    duration: Duration::ZERO,
                    ..AnimationSpec::default()
                },
                Duration::ZERO,
                Standing::Tile,
            );
        }
    }

    /// **A tiled drag measures from the grab, not from the last frame.**
    ///
    /// `ResizeGrab` freezes two rectangles at the press and this pins why
    /// the second of them has to be one of them. `laid_out` is where the
    /// layout had this pane when the button went down, and the layout moves
    /// the pane on *every frame of the drag* — that is what a seam moving
    /// means. So a version that asked `Solium::pane_laid_out` afresh each
    /// frame and added the gesture's total travel to whatever came back
    /// would add the previous frame's travel a second time, and the frame
    /// before that a third. Not drift: the whole of the motion, compounding,
    /// for as long as the button is held.
    ///
    /// It is an easy thing to write, because "ask the layout where the pane
    /// is now" reads like the honest version. The first half below is the
    /// fixture earning its keep — the compositor really is moving this pane
    /// under the drag — and the second is the runaway, computed by doing
    /// exactly that to a second copy of the same tree.
    ///
    /// A *position* built from a frozen base and a total delta, which is
    /// what `ResizeRequest` promises and what keeps `drag_seam` idempotent.
    /// Both halves of that matter and neither implies the other: the base
    /// is frozen so the drag does not read its own output back, and the
    /// delta is total rather than per-frame so it does not accumulate
    /// rounding. See `crate::input::resize::dragged_edge`.
    #[test]
    fn a_tiled_drag_measures_from_where_the_grab_began_and_not_from_the_last_frame() {
        use solium_layout::tree::{Edge, Tiling};

        tiled_fixture!(display, state, conn, queue, client, qh);
        let (left, _left_toplevel, _left_surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        let (right, _right_toplevel, _right_surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.sync_panes();
        let (left_id, right_id) = (state.window_id(&left), state.window_id(&right));

        let (area, settings) = (tiled_area(), tiled_settings());
        let mut tiling = Tiling::new();
        tiling.insert(left_id, None, None, area, settings);
        tiling.insert(right_id, Some(left_id), None, area, settings);
        sweep(&mut state, &tiling);

        let began = leaf_of(&tiling, left_id);
        let frozen = state
            .pane_laid_out(&left)
            .expect("a pane a layout has placed");
        let from: Point<f64, Logical> = (f64::from(frozen.0.loc.x + frozen.0.size.w), 300.0).into();

        // Three frames of one gesture. The numbers are the pointer's total
        // travel from the press, which is what a grab has -- never the step
        // since the last frame.
        let mut the_layout_moved_it = false;
        for total in [10.0, 40.0, 90.0] {
            let now: Point<f64, Logical> = (from.x + total, from.y).into();
            let sent = crate::input::resize::dragged_edge(frozen, ResizeEdge::Right, from, now);
            tiling.drag_seam(left_id, Edge::Right, sent, area, settings);
            sweep(&mut state, &tiling);
            // A whole frame, configures and all. The client is never made
            // to answer here -- what this test is about happens whether it
            // does or not -- but a sweep that never reached the wire would
            // be a different code path from the one a drag takes.
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            state.sync_panes();
            the_layout_moved_it |= state.pane_laid_out(&left) != Some(frozen);
        }
        assert!(
            the_layout_moved_it,
            "the layout has to actually move this pane under the drag, or \
             freezing its rectangle costs nothing and this test pins nothing"
        );

        let ended = leaf_of(&tiling, left_id);
        assert!(
            (ended.w - (began.w + 90.0)).abs() < 0.5,
            "the pointer travelled 90 from the press, so the edge is 90 \
             from where it was: {began:?} became {ended:?}"
        );

        // And the same three frames against a base re-read each time, which
        // is the mistake this is here to keep out.
        let mut runaway = Tiling::new();
        runaway.insert(left_id, None, None, area, settings);
        runaway.insert(right_id, Some(left_id), None, area, settings);
        for total in [10.0, 40.0, 90.0] {
            let live = leaf_of(&runaway, left_id);
            runaway.drag_seam(
                left_id,
                Edge::Right,
                (live.x + live.w + total, 300.0),
                area,
                settings,
            );
        }
        let flew = leaf_of(&runaway, left_id);
        assert!(
            flew.w - began.w > 130.0,
            "re-reading the layout's rectangle each frame adds every \
             earlier frame's travel again -- 140 rather than 90 here, and \
             unbounded on a real gesture: {began:?} became {flew:?}"
        );
    }

    /// **Issue #123, the rate half: a tiled drag configured its client once
    /// per frame.**
    ///
    /// `crate::resizing::TELL_EVERY` is consulted by `Hold::dragged` and by
    /// nothing else, and until now a tiled pane had no hold — so `move_pane`
    /// sent a configure on every frame of every drag, which is sixty a
    /// second. `resizing.rs` says in its own words why that fails: a client
    /// that cannot render at that rate does not try, it falls behind, and
    /// the window stutters against the pointer rather than following it.
    ///
    /// **Every frame asks for a different width**, deliberately. Smithay
    /// deduplicates a configure that repeats the size it last sent, so a
    /// test that dragged a window to the same place twice would be green
    /// against a compositor with no throttle at all.
    #[test]
    fn a_tiled_drag_configures_on_the_throttle_and_not_once_per_frame() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, _toplevel, _surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.map_stacked(window.clone(), (400, 300), false);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&window)
            .expect("a client in the space has a pane");

        // Opening a window configures it, and that configure is not this
        // test's subject.
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        client.configures.clear();

        let request = ResizeRequest {
            window: window.clone(),
            wanted: at(400, 300, 300, 200),
            edge_at: (700.0, 500.0),
            edges: ResizeEdge::Right,
        };
        state.begin_resize(&window);

        // Six frames at sixteen milliseconds, which is what a drag at sixty
        // hertz is and is entirely inside one hundred-millisecond interval.
        for (millis, width) in [
            (0, 300),
            (16, 302),
            (32, 304),
            (48, 306),
            (64, 308),
            (80, 310),
        ] {
            tiled_frame(
                &mut state,
                &request,
                pane,
                at(400, 300, width, 200),
                Duration::from_millis(millis),
            );
        }
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(
            client.configures.len(),
            1,
            "six frames of a claimed drag inside one `TELL_EVERY` must be \
             one configure, not six: the first offer goes out on the frame \
             it is decided and the rest wait. Got {:?}",
            client.configures
        );

        // And past the interval the client hears again, because the throttle
        // is a rate and not a gate: a drag that went on for a second with a
        // single configure in it would end a long way from the pointer.
        tiled_frame(
            &mut state,
            &request,
            pane,
            at(400, 300, 340, 200),
            Duration::from_millis(120),
        );
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(
            client.configures.len(),
            2,
            "the interval has passed, so the next frame's rectangle is sent"
        );
        assert_eq!(
            client.configures.last().map(|&(_, w, h)| (w, h)),
            Some((340, 200)),
            "and what is sent is the rectangle of the frame that sent it, \
             not a stale one the throttle had been sitting on"
        );
    }

    /// **Issue #123, the flicker half: a tiled pane drew the size its client
    /// last committed.**
    ///
    /// `pane_geometry` lets a pane's slot outrank its client only while
    /// `holding_resize` is true, which was false for every tiled drag, so
    /// `pane_outer` and its twenty-odd callers read `real_geometry` — the
    /// size the client last *agreed to*, which during a drag is frames
    /// behind the layout.
    ///
    /// Two assertions and they are not the same one twice. The first is the
    /// inversion. The second is `sync_panes`, which copies the space into
    /// every pane's slot once a frame and so used to undo the layout's
    /// rectangle between the frame that drew it and the next one: that
    /// alternation — the layout's rectangle on a frame carrying a motion,
    /// the client's on a frame without one — is what the user sees as
    /// stutter.
    #[test]
    fn a_tiled_pane_draws_the_layouts_rectangle_while_its_client_lags() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, _toplevel, _surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.map_stacked(window.clone(), (400, 300), false);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&window)
            .expect("a client in the space has a pane");
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );

        let request = ResizeRequest {
            window: window.clone(),
            wanted: at(400, 300, 300, 200),
            edge_at: (700.0, 500.0),
            edges: ResizeEdge::Right,
        };
        state.begin_resize(&window);
        let outer = at(400, 300, 300, 200);
        tiled_frame(&mut state, &request, pane, outer, Duration::ZERO);

        assert_eq!(
            window.geometry().size,
            Size::from((64, 64)),
            "the fixture's client never answers a configure, which is what \
             makes it a stand-in for one that is merely slow -- if it had \
             answered there would be no disagreement to test"
        );
        let geometry = |state: &Solium| {
            state.pane_geometry(state.panes.get(pane).expect("the pane is still here"))
        };
        assert_eq!(
            geometry(&state),
            outer,
            "the layout's rectangle is what the pane is, immediately. \
             Falling back to the client's committed size draws a window \
             the size it was before the drag started at the position the \
             drag has reached"
        );

        state.sync_panes();
        assert_eq!(
            geometry(&state),
            outer,
            "and it survives the frame. `sync_panes` writes the space into \
             every pane's slot, and the space's size is whatever the client \
             last committed, so without `held_slot` the layout's authority \
             lasts exactly until the next frame's reconciliation"
        );
    }

    /// **The trap `crate::resizing` is built around, on the tiled path: a
    /// client that refuses must not be stretched for ever.**
    ///
    /// A client with a minimum size — Firefox has one, a terminal rounds to
    /// its cell grid — answers a configure with a size of its own. Nothing
    /// on the tiled path noticed: `resize_fill` answered `None`, `factor`
    /// took the free-to-grow arm, and the buffer was scaled without limit
    /// towards a rectangle its client had already walked away from. That is
    /// the permanently-soft window this module's documentation says cost a
    /// bug once already, and #115 — reading the client's minimum — is still
    /// unread, so this is the only thing standing between a refusal and a
    /// blur.
    ///
    /// Both halves are asserted, because they happen at different times: the
    /// stretch stops *during* the gesture, and the pane lands on the
    /// client's own size when the gesture ends.
    #[test]
    fn a_tiled_client_that_refuses_a_size_stops_the_stretch_and_ends_the_bridge() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, _toplevel, surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.map_stacked(window.clone(), (400, 300), false);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&window)
            .expect("a client in the space has a pane");
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );

        let request = ResizeRequest {
            window: window.clone(),
            wanted: at(400, 300, 300, 200),
            edge_at: (700.0, 500.0),
            edges: ResizeEdge::Right,
        };
        state.begin_resize(&window);
        let outer = at(400, 300, 300, 200);
        tiled_frame(&mut state, &request, pane, outer, Duration::ZERO);
        assert_eq!(
            state.resize_fill(pane),
            Some(crate::resizing::Fill::Stretch),
            "nothing has been refused yet, so the configured fill stands"
        );

        // The answer, and it is not the one that was asked for: 120x90
        // where 300x200 was offered. A real client saying "this is my
        // minimum" says it exactly this way.
        commit_buffer(&client, &qh, &surface, 120, 90);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(
            window.geometry().size,
            Size::from((120, 90)),
            "the client committed a size of its own, which is the whole \
             scenario"
        );

        tiled_frame(&mut state, &request, pane, outer, Duration::from_millis(16));
        assert_eq!(
            state.resize_fill(pane),
            Some(crate::resizing::Fill::Hold),
            "a refusal overrides the configured fill. A stretch towards a \
             size the client has walked away from never returns to 1, so \
             the window stays soft until something else resizes it"
        );

        state.release_resize(&window);
        state.settle_resize();
        assert!(
            !state.holding_resize(pane),
            "any answer ends the bridge, and a refusal is an answer -- \
             waiting out `PATIENCE` for a size that has already been \
             declined only adds a quarter second of squashed window"
        );
        assert_eq!(
            state
                .panes
                .get(pane)
                .expect("the pane is still here")
                .slot()
                .size,
            Size::from((120, 90)),
            "and the pane lands on the size the client chose rather than \
             keeping a tile its client will never fill"
        );
    }

    /// **A window merely pushed aside by someone else's drag still needs its
    /// configure.**
    ///
    /// `move_pane` runs for every pane a layout touches, not just the one
    /// under the pointer: `tiling.apply` emits a placement for every leaf on
    /// every visible monitor, and a seam moving means at least two of them
    /// have genuinely changed. So the throttle has to be per pane. Retarget
    /// the single hold at "the pane the layout moved" instead — the obvious
    /// smaller fix — and the interval opened by the dragged window swallows
    /// the neighbour's one and only configure, which is a window that never
    /// hears its new size at all.
    ///
    /// Both counts are asserted from one drag, because they are the two
    /// halves of the same rule: throttled is not silenced, and silent is not
    /// throttled.
    #[test]
    fn a_pane_pushed_aside_by_another_panes_drag_is_configured_at_once() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (dragged, dragged_toplevel, _dragged_surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        let (beside, beside_toplevel, _beside_surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.map_stacked(dragged.clone(), (400, 300), false);
        state.map_stacked(beside.clone(), (700, 300), false);
        state.sync_panes();
        let dragged_pane = state
            .panes
            .id_of(&dragged)
            .expect("a client in the space has a pane");
        let beside_pane = state
            .panes
            .id_of(&beside)
            .expect("a client in the space has a pane");
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        client.configures.clear();

        let request = ResizeRequest {
            window: dragged.clone(),
            wanted: at(400, 300, 300, 200),
            edge_at: (700.0, 500.0),
            edges: ResizeEdge::Right,
        };
        state.begin_resize(&dragged);

        // The seam has not reached the neighbour yet, so the layout moves
        // one pane.
        tiled_frame(
            &mut state,
            &request,
            dragged_pane,
            at(400, 300, 300, 200),
            Duration::ZERO,
        );
        // Fifty milliseconds later — half an interval — the seam moves both.
        let later = Duration::from_millis(50);
        tiled_frame(
            &mut state,
            &request,
            dragged_pane,
            at(400, 300, 320, 200),
            later,
        );
        tiled_frame(
            &mut state,
            &request,
            beside_pane,
            at(720, 300, 180, 200),
            later,
        );

        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        let counted = |toplevel: &xdg_toplevel::XdgToplevel| {
            let id = wayland_client::Proxy::id(toplevel);
            client
                .configures
                .iter()
                .filter(|(sent_to, ..)| sent_to == &id)
                .count()
        };
        assert_eq!(
            counted(&dragged_toplevel),
            1,
            "the dragged pane moved on both frames and both were inside one \
             interval, so its client hears once"
        );
        assert_eq!(
            counted(&beside_toplevel),
            1,
            "and the pane beside it moved for the first time on the second \
             frame, so it hears at once -- its interval starts when it \
             moves, not when somebody else did"
        );
    }

    /// **Only a live gesture arms a bridge.**
    ///
    /// The keyboard `nudge` reaches `move_pane` through `Scripts::key` and
    /// never through `settle_resize`, and so do a config reload, a monitor
    /// change, a decoration restyle, a workspace switch and
    /// `rescue_offscreen`. A hold armed by any of them could never be let
    /// go of: `release_resize` has exactly one caller and it is the pointer
    /// grab, so `Hold::settle` would answer `Waiting` for ever and the
    /// pane's slot and the space would be held apart for the life of the
    /// window — the never-resolved disagreement `PATIENCE` exists to
    /// prevent.
    ///
    /// The second assertion is the one that keeps the first honest: a fix
    /// that arms nothing by arming nothing is not a fix. The client still
    /// has to be told.
    #[test]
    fn a_keyboard_nudge_arms_no_bridge_and_still_reaches_its_client() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, _toplevel, _surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.map_stacked(window.clone(), (400, 300), false);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&window)
            .expect("a client in the space has a pane");
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        client.configures.clear();

        // No `arm_resize_gesture`, because nothing in a key dispatch calls
        // it. This is the whole of what a nudge is by the time it arrives.
        let was = state
            .pane_outer_of(pane)
            .expect("a mapped pane has a rectangle");
        state.move_pane(
            pane,
            at(400, 300, 300, 200),
            was,
            AnimationSpec::default(),
            Duration::ZERO,
            Standing::Tile,
        );

        assert!(
            !state.holding_resize(pane),
            "a keypress is not a gesture and has nothing that could ever \
             end a hold"
        );
        assert!(
            state.resize_bridge.is_none(),
            "and no bridge was opened for it either"
        );
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(
            client.configures.len(),
            1,
            "the window still has to hear its new size: arming nothing is \
             only correct if it costs nothing"
        );
    }

    /// **A layout re-placing a pane exactly where it already is says
    /// nothing to its client.**
    ///
    /// `tree:layout` emits every leaf whether or not it moved, once per
    /// visible monitor, and a drag runs it once a frame — so in a dwindle
    /// tree the eight panes that did not move were configured sixty times a
    /// second each for the length of every gesture. Smithay hides half of
    /// that: `send_pending_configure` drops a configure that repeats the
    /// size it last sent, so on the wire the xdg clients saw nothing. The
    /// X11 arm has no such check and sent a real `ConfigureWindow` every
    /// time, for every X11 window in the layout.
    ///
    /// **Asserted against `offers_size` rather than against the wire**, and
    /// that is forced rather than chosen: the fixture speaks xdg, which is
    /// exactly the protocol whose own deduplication makes the defect
    /// invisible from the client's side. A test that counted configures
    /// here would pass against the unfixed compositor and prove nothing.
    #[test]
    fn a_layout_replacing_a_pane_where_it_already_is_tells_its_client_nothing() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, _toplevel, _surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.map_stacked(window.clone(), (400, 300), false);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&window)
            .expect("a client in the space has a pane");
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );

        // Where the client actually is: the fixture's buffer is 64x64 and
        // it was mapped at (400, 300).
        let settled = at(400, 300, 64, 64);
        assert_eq!(
            state.real_geometry(&window),
            Some(settled),
            "the client and the space agree before the sweep, which is the \
             state every unchanged leaf is in on every frame of a drag"
        );
        assert!(
            !state.offers_size(pane, &window, settled, Duration::ZERO),
            "a leaf the layout re-placed where it already was has nothing \
             to be told"
        );
        // A position it does not have is a real change, even at the same
        // size: for an X11 window `size_window` is the only thing that
        // carries a position at all, so deduplicating on size alone would
        // leave one told to stay where it no longer is.
        assert!(
            state.offers_size(pane, &window, at(500, 300, 64, 64), Duration::ZERO),
            "a move with no resize still has to reach the client"
        );
    }

    /// **The throttle had no trailing edge, so the end of every drag was
    /// never sent.**
    ///
    /// `Hold::dragged` is reached from `move_pane` and from `hold_resize`,
    /// and `settle_resize` reaches either only on a frame whose
    /// `pending_resize` carried a motion. So the offers a drag makes inside
    /// its last `TELL_EVERY` are written into the pane's slot, drawn from
    /// the pane's slot, and never put on the wire: the client sits at a size
    /// up to a tenth of a second stale while the pane is drawn where the
    /// pointer is, and the bridge between the two is the visible gap.
    ///
    /// **A paused pointer is the ordinary end of a drag**, not an edge
    /// case — people stop moving before they let go — and the gap is held
    /// for as long as the pause lasts. Before #123 the tiled path
    /// configured on every frame and had no tail at all, which makes this
    /// the regression of exactly the symptom that was reported.
    ///
    /// The pause is driven with `paused_frame`, which is the whole of what
    /// `settle_resize` runs on a frame with no motion. Against the code
    /// before this fix that call is `settle_resize_bridge` alone, and
    /// nothing in it sends anything.
    #[test]
    fn a_drag_that_pauses_still_tells_its_client_where_it_stopped() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, _toplevel, _surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.map_stacked(window.clone(), (400, 300), false);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&window)
            .expect("a client in the space has a pane");
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        client.configures.clear();

        let request = ResizeRequest {
            window: window.clone(),
            wanted: at(400, 300, 300, 200),
            edge_at: (700.0, 500.0),
            edges: ResizeEdge::Right,
        };
        state.begin_resize(&window);

        // Three frames of a drag well inside one interval. The first is the
        // pane joining the bridge, which goes out at once; the other two are
        // throttled, and the third is where the pointer stops.
        for (millis, width) in [(0, 300), (16, 340), (32, 380)] {
            tiled_frame(
                &mut state,
                &request,
                pane,
                at(400, 300, width, 200),
                Duration::from_millis(millis),
            );
        }
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(
            client.configures.last().map(|&(_, w, _)| w),
            Some(300),
            "the control: while the interval is running the client is still \
             at the first offer, and the 80 pixels since are the gap"
        );

        // The pointer has stopped. No motion reaches `settle_resize`, so
        // nothing calls `move_pane` again — these are the frames a paused
        // drag is made of.
        for millis in [48, 64, 80, 96, 112, 128] {
            paused_frame(&mut state, Duration::from_millis(millis));
        }
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(
            client.configures.last().map(|&(_, w, h)| (w, h)),
            Some((380, 200)),
            "once the interval passes the pane's rectangle has to reach its \
             client whether or not the pointer moved again. Without a \
             trailing flush the client stays 80 pixels behind a pane that is \
             drawn where the pointer stopped, for as long as the pause \
             lasts. Got {:?}",
            client.configures
        );
        assert_eq!(
            client.configures.len(),
            2,
            "and once, not once a frame: the flush is the throttle's \
             trailing edge and not a way around it. Got {:?}",
            client.configures
        );
    }

    /// **A pane given a different client mid-gesture was configured through
    /// the old one.**
    ///
    /// `settle_resize_hold`, `settle_resize_bridge` and `release_bridge`
    /// all check `panes.get(pane).and_then(Pane::client)` against the window
    /// their hold remembers before touching anything. `flush_resize` did
    /// not, and it is the one that runs *first* on every frame — so a pane
    /// whose content was replaced while its hold was still alive sent the
    /// new client's slot to the old client, and read the old client's
    /// committed size back into the throttle's bookkeeping while it was
    /// there. `Pane::adopt` is how a pane's content is replaced without the
    /// pane changing: same id, same slot, different window, which is
    /// exactly the state the guard is about.
    ///
    /// **The control is
    /// [`a_drag_that_pauses_still_tells_its_client_where_it_stopped`],
    /// which is this test without the adoption**: the same fixture, the
    /// same two frames inside one interval, the same paused frame past it,
    /// and there the configure does arrive. A test that only asserts
    /// silence passes against a flush that was never going to fire, so the
    /// slot is asserted here as well — the pane is still holding the offer
    /// the throttle swallowed, which is the thing that would have been sent
    /// to the wrong client.
    #[test]
    fn a_pane_given_a_different_client_is_not_flushed_through_the_old_one() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, toplevel, _surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.map_stacked(window.clone(), (400, 300), false);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&window)
            .expect("a client in the space has a pane");
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );

        let request = ResizeRequest {
            window: window.clone(),
            wanted: at(400, 300, 300, 200),
            edge_at: (700.0, 500.0),
            edges: ResizeEdge::Right,
        };
        state.begin_resize(&window);
        // Two frames inside one interval: the first joins the bridge and
        // goes out at once, the second is throttled and is what a trailing
        // flush exists to send.
        for (millis, width) in [(0, 300), (16, 340)] {
            tiled_frame(
                &mut state,
                &request,
                pane,
                at(400, 300, width, 200),
                Duration::from_millis(millis),
            );
        }

        // The pane's content is replaced. Its id and its slot are the same
        // ones the hold is holding; the window inside it is not.
        let (other, _other_toplevel, _other_surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state
            .panes
            .get_mut(pane)
            .expect("the pane is still here")
            .adopt(other);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        client.configures.clear();

        // Past the interval, which is the frame the flush would fire on.
        paused_frame(&mut state, Duration::from_millis(120));
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        let id = wayland_client::Proxy::id(&toplevel);
        assert!(
            !client.configures.iter().any(|(sent_to, ..)| sent_to == &id),
            "the window this hold remembers no longer owns the pane, so the \
             slot being flushed is not its rectangle to be told about. Got \
             {:?}",
            client.configures
        );
        assert_eq!(
            state
                .panes
                .get(pane)
                .expect("the pane is still here")
                .slot()
                .size,
            Size::from((340, 200)),
            "the control: the slot still carries the offer the throttle \
             swallowed, so there was something for the flush to send"
        );
    }

    /// **The same tail on the floating path**, which `settle_resize_hold`
    /// looks like it covers and does not.
    ///
    /// `Hold::settle` decides whether a hold is over; it has never sent a
    /// configure and does not know how. So a paused floating drag sat on
    /// its last offer exactly as a tiled one did — the only thing that ever
    /// sent unconditionally was the release. Older than #123 rather than a
    /// regression of it, and fixed by the same flush, so it is asserted
    /// here rather than left to be rediscovered.
    #[test]
    fn a_floating_drag_that_pauses_also_tells_its_client() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, _toplevel, _surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.map_stacked(window.clone(), (400, 300), false);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&window)
            .expect("a client in the space has a pane");
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        client.configures.clear();
        state.begin_resize(&window);

        let dragged = |state: &mut Solium, width: i32, millis: u64| {
            let request = ResizeRequest {
                window: window.clone(),
                wanted: at(400, 300, width, 200),
                edge_at: (f64::from(400 + width), 500.0),
                edges: ResizeEdge::Right,
            };
            state.hold_resize(&request, Duration::from_millis(millis));
        };
        dragged(&mut state, 300, 0);
        dragged(&mut state, 340, 16);
        dragged(&mut state, 380, 32);
        for millis in [48, 64, 80, 96, 112, 128] {
            paused_frame(&mut state, Duration::from_millis(millis));
        }
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert!(
            state.holding_resize(pane),
            "the gesture is still live, which is what makes the pause a \
             pause rather than an ending"
        );
        assert_eq!(
            client.configures.last().map(|&(_, w, h)| (w, h)),
            Some((380, 200)),
            "a floating drag's tail is the same tail. Got {:?}",
            client.configures
        );
    }

    /// **A pane the layout only *moved* was never told, for the whole of
    /// the gesture.**
    ///
    /// `offers_size`'s case (2) documents why the whole rectangle is
    /// compared and not just the size: `size_window` is the only thing that
    /// carries a position to an X11 client, because `map_stacked` moves the
    /// window in the space and says nothing to anybody. Case (1) compared
    /// sizes — `Hold::dragged` took one — so a bridged pane whose slot
    /// translates answered "nothing to say" and went on answering it until
    /// the button came up. `scrolling.lua`'s `widen` shifts every column
    /// sideways at an unchanged width, so that is the whole of an X11
    /// window's drag in the scrolling layout: stale geometry, and pointer
    /// coordinates routed to where the window used to be.
    ///
    /// **Asserted against `offers_size` rather than against the wire**, and
    /// forced rather than chosen, for the reason
    /// `a_layout_replacing_a_pane_where_it_already_is_tells_its_client_nothing`
    /// gives at length: the fixture speaks xdg, and smithay deduplicates a
    /// configure that repeats the last size it sent — which is every
    /// configure this test is about. Counting them would prove nothing.
    ///
    /// The existing neighbour test cannot see this: it moves the pane it is
    /// about *and* resizes it, and it moves it for the first time, so it
    /// never reaches case (1) at all.
    #[test]
    fn a_bridged_pane_that_only_moves_is_still_told_where_it_went() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, _toplevel, _surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.map_stacked(window.clone(), (400, 300), false);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&window)
            .expect("a client in the space has a pane");
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );

        let request = ResizeRequest {
            window: window.clone(),
            wanted: at(400, 300, 300, 200),
            edge_at: (700.0, 500.0),
            edges: ResizeEdge::Right,
        };
        state.begin_resize(&window);
        // The pane joins the bridge here, so what follows exercises case (1).
        tiled_frame(
            &mut state,
            &request,
            pane,
            at(400, 300, 300, 200),
            Duration::ZERO,
        );
        assert!(
            state.holding_resize(pane),
            "the pane has to be bridged, or this is testing case (2)"
        );

        // A whole interval later, so the throttle is not what is being
        // measured: the column slides 40 pixels sideways at exactly the
        // width it already had.
        assert!(
            state.offers_size(
                pane,
                &window,
                at(440, 300, 300, 200),
                crate::resizing::PATIENCE,
            ),
            "a bridged pane that translates has moved, and for an X11 client \
             `size_window` is the only thing that will ever say so"
        );
        // And the dedup it must not have cost: the same rectangle twice is
        // still nothing to say.
        assert!(
            !state.offers_size(
                pane,
                &window,
                at(440, 300, 300, 200),
                crate::resizing::PATIENCE * 2,
            ),
            "comparing the whole rectangle must not turn into telling the \
             client on every frame"
        );
    }

    /// **A terminal rounds; it does not refuse. `declined` could not tell
    /// the difference, so kitty never got the fill the user configured.**
    ///
    /// `Hold::note` records a decline for any answer that is not exactly
    /// the ask, and `fill` forced `Fill::Hold` for the rest of the gesture
    /// on the strength of it. A terminal answers with a whole number of
    /// character cells and so is a few pixels out on its very first answer
    /// and every one after it — which makes kitty, the ordinary tiled
    /// client, `declined` from the first frame of every seam drag.
    /// `Fill::Hold` deliberately leaves an uncovered strip while a pane
    /// grows, so what the user saw in place of their `stretch` was a band
    /// of background: a different wrong-looking frame rather than none, on
    /// the drag whose symptom is "frames with the wrong size".
    ///
    /// Both directions, because a fix that stops calling anything a refusal
    /// would ship the permanent blur this module was built to prevent.
    #[test]
    fn a_cell_grid_rounding_is_not_the_refusal_that_takes_the_stretch_away() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, _toplevel, surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.map_stacked(window.clone(), (400, 300), false);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&window)
            .expect("a client in the space has a pane");
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );

        let request = ResizeRequest {
            window: window.clone(),
            wanted: at(400, 300, 300, 200),
            edge_at: (700.0, 500.0),
            edges: ResizeEdge::Right,
        };
        state.begin_resize(&window);
        let outer = at(400, 300, 300, 200);
        tiled_frame(&mut state, &request, pane, outer, Duration::ZERO);

        // What kitty answers 300x200 with: the nearest whole number of
        // cells, which at an ordinary font is a handful of pixels short on
        // each axis.
        commit_buffer(&client, &qh, &surface, 294, 190);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(
            window.geometry().size,
            Size::from((294, 190)),
            "the client answered with a size of its own, which is what a \
             cell grid always does"
        );
        tiled_frame(&mut state, &request, pane, outer, Duration::from_millis(16));
        assert_eq!(
            state.resize_fill(pane),
            Some(crate::resizing::Fill::Stretch),
            "six pixels on a three-hundred pixel pane is a rounding, and \
             the user's configured fill stands. Calling it a refusal costs \
             the stretch on every terminal drag there will ever be"
        );

        // And the same client, refusing for real: a minimum width, which is
        // nothing like six pixels out.
        commit_buffer(&client, &qh, &surface, 120, 90);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        tiled_frame(&mut state, &request, pane, outer, Duration::from_millis(32));
        assert_eq!(
            state.resize_fill(pane),
            Some(crate::resizing::Fill::Hold),
            "a client that has walked away from the ask still loses the \
             stretch: it will never bring the factor back to 1, and a \
             window that never un-stretches is the trap this module is \
             built around"
        );
    }

    /// **The drag's throttle was applied to every caller of `move_pane`.**
    ///
    /// A bridge outlives its gesture by up to `PATIENCE`, and `move_pane`
    /// is reached by a config reload, a `modes.use` from a keybinding, a
    /// workspace switch and `rescue_offscreen` as well as by a layout
    /// sweep. Case (1) looked the pane up in the bridge without asking
    /// whose sweep this was, so one of those landing inside a live bridge
    /// had its one and only configure swallowed by an interval somebody
    /// else opened — and nothing would resend it, because a keypress has no
    /// next frame. `move_pane` went on writing the slot regardless, so the
    /// pane was then drawn, stretched, at a rectangle its client had never
    /// been told about for the rest of the gesture.
    ///
    /// `resize_gesture` is set for the length of `trigger_resize` and by
    /// nothing else, which is exactly the question "is the sweep reaching
    /// this pane the live drag's own".
    #[test]
    fn a_reload_inside_a_live_bridge_is_not_throttled_by_the_drag() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, _toplevel, _surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.map_stacked(window.clone(), (400, 300), false);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&window)
            .expect("a client in the space has a pane");
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        client.configures.clear();

        let request = ResizeRequest {
            window: window.clone(),
            wanted: at(400, 300, 300, 200),
            edge_at: (700.0, 500.0),
            edges: ResizeEdge::Right,
        };
        state.begin_resize(&window);
        tiled_frame(
            &mut state,
            &request,
            pane,
            at(400, 300, 300, 200),
            Duration::ZERO,
        );

        // Sixteen milliseconds later — deep inside the interval the drag
        // just opened — something that is not the drag places this pane.
        // No `arm_resize_gesture`, because a reload does not run one: this
        // is the whole of what a `move_pane` from a key dispatch is.
        let was = state
            .pane_outer_of(pane)
            .expect("a mapped pane has a rectangle");
        state.move_pane(
            pane,
            at(400, 300, 260, 180),
            was,
            AnimationSpec::default(),
            Duration::from_millis(16),
            Standing::Tile,
        );
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(
            client.configures.last().map(|&(_, w, h)| (w, h)),
            Some((260, 180)),
            "a reload's one configure must not be swallowed by a drag's \
             interval: there is no second frame to resend it, and the pane \
             has already taken the rectangle. Got {:?}",
            client.configures
        );
        assert!(
            state.holding_resize(pane),
            "and the bridge is still the bridge -- the reload was sent \
             through the hold, not around it, so `asked` still names what \
             the client last heard"
        );
    }

    /// **A pending change must not wait out the drag's interval.**
    ///
    /// `size_window` is a pending size *and* a `send_pending_configure`, so
    /// a throttled frame skips the flush as well. A maximise, a fullscreen
    /// or a decoration mode agreed by somebody else is then blocked behind
    /// an interval for exactly the windows a drag is touching — which is
    /// the one place case (1) short-circuited before the check that exists
    /// to catch it.
    #[test]
    fn a_pending_change_is_flushed_even_while_the_drag_is_throttled() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, _toplevel, _surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.map_stacked(window.clone(), (400, 300), false);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&window)
            .expect("a client in the space has a pane");
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        client.configures.clear();

        let request = ResizeRequest {
            window: window.clone(),
            wanted: at(400, 300, 300, 200),
            edge_at: (700.0, 500.0),
            edges: ResizeEdge::Right,
        };
        state.begin_resize(&window);
        tiled_frame(
            &mut state,
            &request,
            pane,
            at(400, 300, 300, 200),
            Duration::ZERO,
        );
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        let before = client.configures.len();

        // Somebody else agrees a state change and is waiting on the
        // configure that carries it.
        window
            .toplevel()
            .expect("the fixture's window is an xdg toplevel")
            .with_pending_state(|state| {
                state.states.set(
                    smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Maximized,
                );
            });
        // And a throttled frame of the drag arrives before the interval is
        // up. This is the frame that used to swallow it.
        tiled_frame(
            &mut state,
            &request,
            pane,
            at(400, 300, 310, 200),
            Duration::from_millis(16),
        );
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert!(
            client.configures.len() > before,
            "the throttle governs how often a drag asks for a size, not \
             whether a window blocked on somebody else's change ever hears \
             about it. Got {:?}",
            client.configures
        );
    }

    /// **A press that pauses let the previous gesture's deadline expire.**
    ///
    /// `begin_resize` reconciles the floating hold and clears
    /// `resize_ended`, and `arm_resize_gesture` rearms the bridge — but
    /// that runs on the first *motion*, and `settle_resize_bridge` runs on
    /// every frame. So pressing on a border, holding still for a quarter of
    /// a second and then dragging let the previous drag's `PATIENCE` run
    /// out with the button already down: every pane that drag had moved was
    /// adopted off its tile, which is precisely the snap `Hold::rearm`
    /// exists to prevent, arriving between the press and the first pixel of
    /// motion.
    #[test]
    fn a_press_stops_the_previous_drags_deadline_before_it_can_expire() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, _toplevel, _surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.map_stacked(window.clone(), (400, 300), false);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&window)
            .expect("a client in the space has a pane");
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );

        let request = ResizeRequest {
            window: window.clone(),
            wanted: at(400, 300, 300, 200),
            edge_at: (700.0, 500.0),
            edges: ResizeEdge::Right,
        };
        state.begin_resize(&window);
        let outer = at(400, 300, 300, 200);
        tiled_frame(&mut state, &request, pane, outer, Duration::ZERO);
        // The first gesture ends. Its holds are now waiting out `PATIENCE`
        // for an answer the fixture's client never gives.
        let ended = Duration::from_millis(100);
        state.release_bridge(&window, ended);

        // The second gesture presses, and the hand stays still. Every frame
        // of that pause runs `settle_resize_bridge`.
        state.begin_resize(&window);
        paused_frame(&mut state, ended + crate::resizing::PATIENCE);
        paused_frame(
            &mut state,
            ended + crate::resizing::PATIENCE + Duration::from_millis(50),
        );

        assert!(
            state.holding_resize(pane),
            "the new gesture owns this pane and will keep placing it, so \
             the old gesture's deadline must stop at the press rather than \
             at the first motion"
        );
        assert_eq!(
            state
                .panes
                .get(pane)
                .expect("the pane is still here")
                .slot(),
            outer,
            "and the pane is still on the tile the last drag left it on \
             rather than snapped back to its client's own size"
        );
    }

    /// **A claimed/unclaimed flip cost two unthrottled configures and reset
    /// the interval.**
    ///
    /// `settle_resize` forks per frame, so a handler whose answer changes
    /// between frames hands one pane back and forth between the bridge and
    /// the floating hold. Each direction built a fresh `Hold` with a fresh
    /// `told`, and a fresh hold is told immediately by design — so an
    /// alternating handler restored the sixty configures a second
    /// `TELL_EVERY` exists to remove. Carrying the hold across the boundary
    /// is what stops that: it is the same client in the same gesture.
    ///
    /// The fixture has no scripts, so `trigger_resize` returns false and
    /// `settle_resize`'s own unclaimed branch is what `hold_resize` is
    /// reached through here; `tiled_frame` is the claimed one.
    #[test]
    fn a_layout_changing_its_mind_does_not_buy_a_configure_each_way() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, _toplevel, _surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.map_stacked(window.clone(), (400, 300), false);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&window)
            .expect("a client in the space has a pane");
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        client.configures.clear();

        let asking = |width: i32| ResizeRequest {
            window: window.clone(),
            wanted: at(400, 300, width, 200),
            edge_at: (f64::from(400 + width), 500.0),
            edges: ResizeEdge::Right,
        };
        state.begin_resize(&window);

        // Claimed: the pane joins the bridge and is told at once.
        tiled_frame(
            &mut state,
            &asking(300),
            pane,
            at(400, 300, 300, 200),
            Duration::ZERO,
        );
        // Unclaimed, sixteen milliseconds later. Every frame asks for a
        // different width, because smithay drops a configure that repeats
        // the last size it sent and a test that dragged to the same place
        // twice would be green against a compositor with no throttle at all.
        state.hold_resize(&asking(310), Duration::from_millis(16));
        // Claimed again.
        tiled_frame(
            &mut state,
            &asking(320),
            pane,
            at(400, 300, 320, 200),
            Duration::from_millis(32),
        );
        // And unclaimed again.
        state.hold_resize(&asking(330), Duration::from_millis(48));

        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(
            client.configures.len(),
            1,
            "four frames inside one interval are one configure however many \
             times the layout changed its mind: the pane's throttle belongs \
             to the gesture, not to whichever authority happens to be \
             placing it. Got {:?}",
            client.configures
        );
        assert!(
            state.holding_resize(pane),
            "and exactly one authority is holding it at the end"
        );
    }

    /// **A pane's moved edge was derived from where its *client* is, not
    /// from where the pane was.**
    ///
    /// `moved_edges` asks which of this pane's edges a placement moved, and
    /// a pane's previous rectangle is its slot. `real_geometry` is the
    /// client's — the space's position at the size the client last
    /// committed — which during a drag is frames behind the slot, so the
    /// client's latency leaked into the answer: an edge that did not move
    /// looks moved because the client has not caught up to where it already
    /// is. `Hold::pins` then hangs a held picture against the wrong side of
    /// the window, and `anchored` gives a refusal's pixels back on an edge
    /// the user never touched.
    ///
    /// The arrangement below is the ordinary one: a client that has
    /// answered nothing, a pane a seam has already widened once, and a
    /// second placement that pulls the pane's *left* edge. Measured from
    /// the slot that is a left pull and nothing else. Measured from the
    /// client's 64x64 buffer both horizontal edges look moved — which names
    /// neither — and the vertical bottom looks moved as well.
    #[test]
    fn a_panes_moved_edge_is_measured_from_its_own_slot_not_from_its_client() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, _toplevel, _surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.map_stacked(window.clone(), (400, 300), false);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&window)
            .expect("a client in the space has a pane");
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(
            window.geometry().size,
            Size::from((64, 64)),
            "the client has answered nothing, which is what makes its \
             rectangle the wrong thing to measure against"
        );

        let request = ResizeRequest {
            window: window.clone(),
            wanted: at(400, 300, 300, 200),
            edge_at: (700.0, 500.0),
            edges: ResizeEdge::Left,
        };
        state.begin_resize(&window);
        tiled_frame(
            &mut state,
            &request,
            pane,
            at(400, 300, 300, 200),
            Duration::ZERO,
        );

        // The layout stops placing this pane for a frame and then places it
        // again, which is what makes the edges be derived a second time.
        // Driven directly for the reason `tiled_frame` gives: a Lua handler
        // that changed its mind between frames would be testing mlua.
        state.drop_bridged(pane);
        // A left pull from the slot: the left edge moves in by 40 and the
        // right edge — at 700 — does not move at all.
        tiled_frame(
            &mut state,
            &request,
            pane,
            at(440, 300, 260, 200),
            Duration::from_millis(16),
        );

        assert_eq!(
            state.resize_pins(pane),
            Some((true, false)),
            "the left edge is the one that moved and the right one is \
             standing still, so a held picture has to stay against the \
             right. Measured from the client's committed 64x64 instead, \
             both horizontal edges look moved and the pane is told it has \
             no stationary edge at all"
        );
    }

    /// **A window the space does not have still needs the throttle.**
    ///
    /// Arming required `real_geometry` to answer, and it answers `None` for
    /// a window that is not in the space. The rectangle still counted as
    /// changed — `None != Some(client)` — so the client was told on every
    /// frame and no hold was ever armed to say when to stop: the one path
    /// left running at the pre-#123 configure rate, for the length of every
    /// gesture that touched such a window.
    ///
    /// A pane's own slot is the previous rectangle the edges want anyway,
    /// so nothing needs the space's answer to arm; only case (2)'s "is the
    /// client already here" does, and `None` there means "no idea", which
    /// is a reason to send rather than a reason not to hold.
    #[test]
    fn a_pane_whose_window_left_the_space_is_still_bridged() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, _toplevel, _surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.map_stacked(window.clone(), (400, 300), false);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&window)
            .expect("a client in the space has a pane");
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );

        state.space.unmap_elem(&window);
        assert_eq!(
            state.real_geometry(&window),
            None,
            "the space no longer has it, which is the whole precondition"
        );

        let request = ResizeRequest {
            window: window.clone(),
            wanted: at(400, 300, 300, 200),
            edge_at: (700.0, 500.0),
            edges: ResizeEdge::Right,
        };
        state.begin_resize(&window);
        tiled_frame(
            &mut state,
            &request,
            pane,
            at(400, 300, 300, 200),
            Duration::ZERO,
        );
        assert!(
            state.holding_resize(pane),
            "a pane a live gesture moved is bridged whether or not the \
             space can say where its client was: without a hold nothing \
             throttles it and nothing ever ends it"
        );
    }

    /// **The keyboard belongs to the lock screen, whatever the session
    /// behind it does.**
    ///
    /// One test per route by which the keyboard used to reach an
    /// application while the session was locked, each driven the way it
    /// happens: a real application client and a real lock client on two
    /// sockets, each with a `wl_keyboard` of its own, keys pressed through
    /// the same filter the backends feed. Every route ends on the same
    /// four measures, taken from both ends of the wire: the server's
    /// keyboard is on a lock surface, the application's client was never
    /// told it has the keyboard, it was never made the clipboard's client,
    /// and a key typed reaches the lock client and not the application.
    /// Then each one unlocks and checks that the keyboard comes back,
    /// because a fix that sealed the keyboard for good would pass
    /// everything before that.
    ///
    /// Each of those route tests fails against `69835bc`, the stage they
    /// were fixed on. That was checked by putting that commit's code back
    /// under them and watching each one fail -- a security test that
    /// passes either way is worse than none.
    ///
    /// Then the tests of **who holds the lock**, from `the lock's holder`
    /// on: a second lock client, an application asking for a lock of its
    /// own, an unlock from a lock that was never granted, the lock client
    /// crashing, a mode active at the lock, a lock surface dying, the key
    /// held at unlock, a capture while locked and a menu closing. Each of
    /// those drives real clients too, with a third one where the attack or
    /// the second lock screen needs one, and each fails against
    /// `2b6550e`; the doc of each says where.
    ///
    /// Then the two where #127's close meets the rule, which neither branch
    /// could write alone: `a_close_in_flight_across_a_lock_never_takes_the_keyboard`
    /// and `a_close_with_a_menu_open_takes_the_keyboard_off_the_menu`. Each
    /// fails against the merge of the two with its own fix taken out; the
    /// doc of each says where.
    ///
    /// The X11 test is the exception to all of them: see its doc.
    ///
    /// Then the tests of **when `locked` is sent**, from
    /// `locked_is_not_sent_in_the_dispatch_that_asked_for_it` on: `lock.rs`
    /// told the lock client `locked` in the dispatch that asked for the
    /// lock, while every monitor was still showing the desktop. Each fails
    /// against this branch with `lock` confirming at once again, as it did
    /// before -- except the first half of
    /// `with_no_monitor_locked_goes_out_at_once`, which is the one case where
    /// "at once" was already right; the doc of each says where. Every lock
    /// in this module now also has its frames shown (`Session::show`),
    /// because without them `locked` does not come.
    mod lock_focus {
        use super::*;
        use smithay::backend::input::KeyState;
        use smithay::input::keyboard::Keycode;

        /// One client's end of the wire.
        struct Side {
            conn: Connection,
            queue: wayland_client::EventQueue<Client>,
            qh: QueueHandle<Client>,
            client: Client,
        }

        impl Side {
            /// A client with a keyboard and a data device, bound the way
            /// any application binds them.
            fn connect(display: &mut Display<Solium>, state: &mut Solium) -> Self {
                let (conn, queue, client) = connect(display, state);
                let qh = queue.handle();
                let mut side = Self {
                    conn,
                    queue,
                    qh,
                    client,
                };
                let seat = side.client.seat.clone().expect("wl_seat bound");
                side.client
                    .data_devices
                    .clone()
                    .expect("wl_data_device_manager bound")
                    .get_data_device(&seat, &side.qh, ());
                // The keyboard is not asked for here: the seat's handler
                // takes the one keyboard a client has, and a second would
                // be sent every key again. That costs a second round trip,
                // one to hear the seat's capabilities and one for the
                // server to see the `get_keyboard` sent in answer --
                // `a_mode_active_at_the_lock_does_not_garble_the_password`
                // counts the keys that arrive.
                side.pump(display, state);
                side.pump(display, state);
                assert!(
                    side.client.keyboard.is_some(),
                    "the seat offered a keyboard and the client took it"
                );
                side
            }

            fn pump(&mut self, display: &mut Display<Solium>, state: &mut Solium) {
                pump(
                    display,
                    state,
                    &self.conn,
                    &self.qh,
                    &mut self.queue,
                    &mut self.client,
                );
            }

            /// A round trip for a client the server may disconnect on the
            /// way, which `pump` would read as the fixture failing.
            /// Returns the protocol error it was disconnected with, if any.
            fn pump_or_error(
                &mut self,
                display: &mut Display<Solium>,
                state: &mut Solium,
            ) -> Option<wayland_client::backend::protocol::ProtocolError> {
                self.conn.display().sync(&self.qh, ());
                // A client already disconnected cannot flush; what it was
                // disconnected with is still read below.
                let _flushed = self.conn.flush();
                display
                    .dispatch_clients(state)
                    .expect("dispatching the round trip");
                display
                    .flush_clients()
                    .expect("flushing the server's events");
                match self.queue.blocking_dispatch(&mut self.client) {
                    Ok(_) => None,
                    Err(_) => self.conn.protocol_error(),
                }
            }

            /// Ask for a lock, and if `cover`, a lock surface for every
            /// monitor, the way a lock screen does. Nothing is asserted:
            /// this is also how an application behind the lock asks.
            fn ask_lock(
                &mut self,
                display: &mut Display<Solium>,
                state: &mut Solium,
                cover: bool,
            ) -> (ext_session_lock_v1::ExtSessionLockV1, Vec<LockSurfaceProxy>) {
                let locks = self
                    .client
                    .locks
                    .clone()
                    .expect("ext_session_lock_manager_v1 bound");
                let compositor = self.client.compositor.clone().expect("wl_compositor bound");
                let lock = locks.lock(&self.qh, ());
                let mut surfaces = Vec::new();
                if cover {
                    for output in self.client.outputs.clone() {
                        let surface = compositor.create_surface(&self.qh, ());
                        let role = lock.get_lock_surface(&surface, &output, &self.qh, ());
                        surfaces.push(LockSurfaceProxy { surface, role });
                    }
                }
                self.pump(display, state);
                (lock, surfaces)
            }
        }

        /// One lock surface, from the client's side.
        struct LockSurfaceProxy {
            surface: wl_surface::WlSurface,
            role: ext_session_lock_surface_v1::ExtSessionLockSurfaceV1,
        }

        impl LockSurfaceProxy {
            /// Destroy it the way `swaylock` does when its monitor goes.
            fn destroy(&self) {
                self.role.destroy();
                self.surface.destroy();
            }
        }

        impl Side {
            /// Open a window, and run the frame that follows it.
            ///
            /// `sync_panes` is what the backends call once a frame, and it
            /// is where a window arriving or going is noticed -- and where
            /// `settle_focus` is called from. A test that skipped it would
            /// skip one of the routes.
            fn open(
                &mut self,
                display: &mut Display<Solium>,
                state: &mut Solium,
            ) -> (
                Window,
                xdg_toplevel::XdgToplevel,
                wl_surface::WlSurface,
                xdg_surface::XdgSurface,
            ) {
                let opened = open_xdg(display, state, &self.conn, &self.client, &self.qh);
                state.sync_panes();
                self.pump(display, state);
                opened
            }

            /// Open a menu on `parent` that asks for a grab, as a context
            /// menu does: the grab before the first commit, as the
            /// protocol requires, with the serial of the keyboard's last
            /// enter.
            fn menu(
                &mut self,
                display: &mut Display<Solium>,
                state: &mut Solium,
                parent: &xdg_surface::XdgSurface,
            ) -> xdg_popup::XdgPopup {
                let compositor = self.client.compositor.clone().expect("wl_compositor bound");
                let wm_base = self.client.wm_base.clone().expect("xdg_wm_base bound");
                let seat = self.client.seat.clone().expect("wl_seat bound");

                let surface = compositor.create_surface(&self.qh, ());
                let xdg = wm_base.get_xdg_surface(&surface, &self.qh, ());
                let positioner = wm_base.create_positioner(&self.qh, ());
                positioner.set_size(32, 32);
                positioner.set_anchor_rect(0, 0, 1, 1);
                let popup = xdg.get_popup(Some(parent), &positioner, &self.qh, ());
                popup.grab(&seat, self.client.serial);
                surface.commit();
                self.pump(display, state);
                popup
            }
        }

        /// One monitor, an application client and a lock client that has
        /// not locked anything yet.
        struct Session {
            display: Display<Solium>,
            state: Solium,
            app: Side,
            locker: Side,
            /// The lock client's surfaces from its last `lock`, one per
            /// monitor.
            lock_surfaces: Vec<LockSurfaceProxy>,
            /// Every monitor with its `wl_output` global, so one can be
            /// unplugged the way a backend unplugs it.
            monitors: Vec<(
                Output,
                smithay::reexports::wayland_server::backend::GlobalId,
            )>,
        }

        impl Session {
            fn new() -> Self {
                Self::with_monitors(1)
            }

            /// `count` monitors side by side, each 1920x1080.
            fn with_monitors(count: i32) -> Self {
                let mut display =
                    Display::<Solium>::new().expect("creating a test wayland display");
                let mut state = Solium::new(display.handle());
                // See the #99 test: a Qt scene in a process holding a
                // libwayland connection of its own aborts the test binary.
                state
                    .decorations
                    .set_style(&mut state.panes, Some("none".to_string()));

                let mut monitors = Vec::new();
                for index in 0..count {
                    // A global, which the other fixtures' monitors are
                    // not: a lock surface is asked for per `wl_output`, so
                    // the lock client has to be able to name one. Before
                    // either client connects, so it is in both registries.
                    monitors.push(Self::monitor(&display, &mut state, index));
                }

                let app = Side::connect(&mut display, &mut state);
                let locker = Side::connect(&mut display, &mut state);
                Self {
                    display,
                    state,
                    app,
                    locker,
                    lock_surfaces: Vec::new(),
                    monitors,
                }
            }

            /// The `index`th monitor, 1920x1080 and to the right of the
            /// one before it, with its global.
            fn monitor(
                display: &Display<Solium>,
                state: &mut Solium,
                index: i32,
            ) -> (
                Output,
                smithay::reexports::wayland_server::backend::GlobalId,
            ) {
                let output = Output::new(
                    format!("lock-focus-test-{index}"),
                    PhysicalProperties {
                        size: (0, 0).into(),
                        subpixel: Subpixel::Unknown,
                        make: "solium".to_string(),
                        model: "lock-focus".to_string(),
                    },
                );
                output.change_current_state(
                    Some(Mode {
                        size: (1920, 1080).into(),
                        refresh: 60_000,
                    }),
                    None,
                    Some(Scale::Fractional(1.0)),
                    None,
                );
                let global = output.create_global::<Solium>(&display.handle());
                state.space.map_output(&output, (1920 * index, 0));
                (output, global)
            }

            /// The `index`th monitor.
            fn output(&self, index: usize) -> Output {
                self.monitors
                    .get(index)
                    .map(|(output, _)| output.clone())
                    .expect("a monitor this fixture made")
            }

            /// A monitor plugged in, the way both backends add one: a
            /// global, mapped, and `settle_monitors`.
            fn plug(&mut self) -> Output {
                let index = i32::try_from(self.monitors.len()).expect("a handful of monitors");
                let monitor = Self::monitor(&self.display, &mut self.state, index);
                let output = monitor.0.clone();
                self.monitors.push(monitor);
                self.state.settle_monitors();
                output
            }

            /// A monitor unplugged -- or switched off by the configuration,
            /// which `tty.rs` drops the same way -- as `drop_screen` and then
            /// `resync_screens` do it: the global removed, the layers closed,
            /// the output unmapped, and `settle_monitors`.
            fn unplug(&mut self, index: usize) {
                let (output, global) = self.monitors.remove(index);
                self.display.handle().remove_global::<Solium>(global);
                crate::layer::close_all(&output);
                self.state.space.unmap_output(&output);
                self.state.settle_monitors();
            }

            /// `output` shows a frame built now: the backend's half of
            /// "When `locked` is sent" in `lock.rs`, played the only way it
            /// can be without a renderer -- `lock_frame` as the frame is
            /// built, `frame_presented` when it is on the screen.
            fn show(&mut self, output: &Output) {
                let frame = self.state.lock_frame();
                self.state.frame_presented(output, frame);
            }

            /// Every monitor shows a frame built now.
            fn show_all(&mut self) {
                for (output, _) in self.monitors.clone() {
                    self.show(&output);
                }
            }

            /// Whether the lock client has been told `locked` on `lock`,
            /// after a round trip for it to hear anything that was sent.
            fn told_locked(&mut self, lock: &ext_session_lock_v1::ExtSessionLockV1) -> bool {
                self.locker.pump(&mut self.display, &mut self.state);
                self.locker
                    .client
                    .locked
                    .contains(&wayland_client::Proxy::id(lock))
            }

            /// Whether the server's keyboard is on one of the lock
            /// client's surfaces.
            ///
            /// Asked of the lock itself and not of anything in `focus.rs`:
            /// a test that checked the gate by asking the gate would pass
            /// whatever the gate did.
            fn keyboard_on_lock(&self) -> bool {
                let Some(focus) = self
                    .state
                    .seat
                    .get_keyboard()
                    .and_then(|keyboard| keyboard.current_focus())
                else {
                    return false;
                };
                self.state
                    .lock
                    .as_ref()
                    .is_some_and(|lock| lock.surfaces().any(|each| each.wl_surface() == &focus))
            }

            /// Lock the session the way a lock screen does: lock, then a
            /// surface for the monitor. Then every monitor shows the lock,
            /// which is what `locked` waits for.
            fn lock(&mut self) -> ext_session_lock_v1::ExtSessionLockV1 {
                let (lock, surfaces) =
                    self.locker
                        .ask_lock(&mut self.display, &mut self.state, true);
                self.lock_surfaces = surfaces;
                self.show_all();
                self.locker.pump(&mut self.display, &mut self.state);
                self.app.pump(&mut self.display, &mut self.state);

                assert!(
                    self.state.lock.is_some(),
                    "the lock client asked and the session did not lock, so \
                     nothing after this is about a locked session"
                );
                assert!(
                    self.locker
                        .client
                        .locked
                        .contains(&wayland_client::Proxy::id(&lock)),
                    "the lock client was not told `locked`"
                );
                assert!(
                    self.keyboard_on_lock(),
                    "the lock surface mapped and the keyboard is not on it: \
                     `focus_lock` is the one hand-off the gate must let through"
                );
                assert!(
                    self.locker.client.keyboard_on.is_some(),
                    "and the lock client was told so"
                );
                assert!(
                    self.app.client.keyboard_on.is_none(),
                    "locking did not take the keyboard away from the application"
                );
                lock
            }

            /// Type one key, and say who heard it: the application, and
            /// the lock client.
            fn type_key(&mut self) -> (bool, bool) {
                self.app.client.keys.clear();
                self.locker.client.keys.clear();
                // `a`, which is evdev 30 and 38 to xkb.
                crate::input::key(&mut self.state, Keycode::new(38), KeyState::Pressed, 1);
                crate::input::key(&mut self.state, Keycode::new(38), KeyState::Released, 2);
                self.app.pump(&mut self.display, &mut self.state);
                self.locker.pump(&mut self.display, &mut self.state);
                (
                    !self.app.client.keys.is_empty(),
                    !self.locker.client.keys.is_empty(),
                )
            }

            /// What every route ends on: the keyboard is still the lock
            /// screen's by every measure there is.
            fn assert_sealed(&mut self, route: &str, selections_before: usize) {
                assert!(
                    self.keyboard_on_lock(),
                    "{route}: the keyboard left the lock screen"
                );
                assert!(
                    self.app.client.keyboard_on.is_none(),
                    "{route}: the application's client was told it has the keyboard"
                );
                assert!(
                    self.locker.client.keyboard_on.is_some(),
                    "{route}: the lock client was not told it has the keyboard"
                );
                assert_eq!(
                    self.app.client.selections, selections_before,
                    "{route}: the application behind the lock was made the \
                     clipboard's client"
                );
                let (app, locker) = self.type_key();
                assert!(
                    !app,
                    "{route}: a key typed at the lock screen reached the \
                     application behind it, and that key is a password"
                );
                assert!(
                    locker,
                    "{route}: the key reached nobody at all, so the line \
                     above proves nothing"
                );
            }

            /// Unlock, and the keyboard comes back to a window without the
            /// mouse being touched.
            fn assert_unlocks(&mut self, lock: ext_session_lock_v1::ExtSessionLockV1) {
                lock.unlock_and_destroy();
                self.locker.pump(&mut self.display, &mut self.state);
                self.app.pump(&mut self.display, &mut self.state);

                assert!(
                    self.state.lock.is_none(),
                    "the lock client unlocked and the session did not"
                );
                let focus = self
                    .state
                    .seat
                    .get_keyboard()
                    .and_then(|keyboard| keyboard.current_focus());
                assert!(
                    focus
                        .as_ref()
                        .is_some_and(|surface| self.state.window_for(surface).is_some()),
                    "the session unlocked with the keyboard on no window. \
                     `unlock` clears `lock` before `settle_focus` so that the \
                     gate lets it through; a session that ignores typing until \
                     you move the mouse reads as one that did not unlock"
                );
                assert!(
                    self.app.client.keyboard_on.is_some(),
                    "and the application was not told it has it back"
                );
                let (app, _) = self.type_key();
                assert!(app, "unlocked, and typing does not reach the application");
            }
        }

        /// **A window opening while locked.** `new_toplevel` set the
        /// keyboard itself, straight onto the new window: an application
        /// that opened a window behind the lock took the password.
        #[test]
        fn a_window_that_opens_while_locked_does_not_take_the_keyboard() {
            let mut session = Session::new();
            session.app.open(&mut session.display, &mut session.state);
            assert!(
                session.app.client.keyboard_on.is_some(),
                "the application has the keyboard before the lock, or \
                 taking it away proves nothing"
            );
            let lock = session.lock();
            let selections = session.app.client.selections;

            session.app.open(&mut session.display, &mut session.state);

            session.assert_sealed("a window opened while locked", selections);
            session.assert_unlocks(lock);
        }

        /// **A window asking to be brought forward while locked.** A token
        /// says the request came from something the user was using, not
        /// that anyone is at the machine now. `request_activation` focuses
        /// through `focus_window`, which asks the gate first, and #134's
        /// second review added a step after that: a window off every screen
        /// has the keyboard handed on from it. Asked of a window on screen
        /// and of one off every screen, so that both branches run behind the
        /// lock and neither leaves it.
        #[test]
        fn a_window_asking_to_be_brought_forward_while_locked_does_not_take_the_keyboard() {
            let mut session = Session::new();
            let (window, _, surface, _) =
                session.app.open(&mut session.display, &mut session.state);
            let pane = session
                .state
                .panes
                .id_of(&window)
                .expect("an open window has a pane");
            let lock = session.lock();
            let selections = session.app.client.selections;

            for off_screen in [false, true] {
                if off_screen {
                    // Placed as a layout places, so that where it is going
                    // is off every screen and not only where it is.
                    let now = session.state.clock.now();
                    session.state.place(
                        pane.get(),
                        to_rect(Rectangle::new((5000, 5000).into(), (400, 300).into())),
                        AnimationSpec::default(),
                        now,
                        Standing::Free,
                    );
                }
                assert_eq!(
                    session.state.pane_on_stage(pane),
                    !off_screen,
                    "the premise: the window is where this case needs it"
                );
                let token = {
                    let (token, _) = session
                        .state
                        .activation_state
                        .create_external_token(XdgActivationTokenData::default());
                    token.as_str().to_owned()
                };
                session
                    .app
                    .client
                    .activation
                    .clone()
                    .expect("xdg_activation_v1 bound")
                    .activate(token, &surface);
                session.app.pump(&mut session.display, &mut session.state);
                session.assert_sealed(
                    &format!(
                        "a window {} asked to be brought forward while locked",
                        if off_screen {
                            "off every screen"
                        } else {
                            "on screen"
                        }
                    ),
                    selections,
                );
            }
            // Back on screen, so that unlocking has a window to give the
            // keyboard to.
            let now = session.state.clock.now();
            session.state.place(
                pane.get(),
                to_rect(Rectangle::new((100, 100).into(), (400, 300).into())),
                AnimationSpec::default(),
                now,
                Standing::Free,
            );
            session.assert_unlocks(lock);
        }

        /// **A window closing while locked.** `sync_panes` calls
        /// `settle_focus`, which returns early only if a *window* has
        /// focus. A lock surface is not a window, so it went on to its
        /// topmost arm and gave the keyboard to the top application.
        #[test]
        fn a_window_that_closes_while_locked_does_not_take_the_keyboard() {
            let mut session = Session::new();
            session.app.open(&mut session.display, &mut session.state);
            let (_window, toplevel, surface, xdg) =
                session.app.open(&mut session.display, &mut session.state);
            let lock = session.lock();
            let selections = session.app.client.selections;

            toplevel.destroy();
            xdg.destroy();
            surface.destroy();
            session.app.pump(&mut session.display, &mut session.state);
            // The frame after it: the space lets go of the dead window and
            // `sync_panes` sees the set of windows change.
            session.state.space.refresh();
            assert!(
                session.state.sync_panes(),
                "closing a window did not change the set of windows, so \
                 `settle_focus` never ran and this proves nothing"
            );

            session.assert_sealed("a window closed while locked", selections);
            session.assert_unlocks(lock);
        }

        /// **A menu opened while locked.** `grab` set the keyboard itself,
        /// onto the menu, and installed a keyboard grab that ignores every
        /// later attempt to move it.
        #[test]
        fn a_menu_that_grabs_while_locked_does_not_take_the_keyboard() {
            let mut session = Session::new();
            let (_window, _toplevel, _surface, parent) =
                session.app.open(&mut session.display, &mut session.state);
            let lock = session.lock();
            let selections = session.app.client.selections;

            let _menu = session
                .app
                .menu(&mut session.display, &mut session.state, &parent);

            assert!(
                !session
                    .state
                    .seat
                    .get_keyboard()
                    .expect("the seat has a keyboard")
                    .is_grabbed(),
                "a menu behind the lock was given a keyboard grab"
            );
            assert!(
                session.app.client.popups_done > 0,
                "the grab was refused and the menu was not told, so it waits \
                 on a grab that is not coming and is still open after unlock"
            );
            session.assert_sealed("a menu grabbed while locked", selections);
            session.assert_unlocks(lock);
        }

        /// **A menu already open when the session locks.** Its keyboard
        /// grab ignored `lock`'s attempt to take the keyboard away and
        /// `focus_lock`'s attempt to give it to the lock screen, then
        /// re-aimed the keyboard at the menu on every key.
        #[test]
        fn a_menu_open_when_the_session_locks_gives_the_keyboard_up() {
            let mut session = Session::new();
            let (_window, _toplevel, _surface, parent) =
                session.app.open(&mut session.display, &mut session.state);
            let _menu = session
                .app
                .menu(&mut session.display, &mut session.state, &parent);
            let keyboard = session
                .state
                .seat
                .get_keyboard()
                .expect("the seat has a keyboard");
            assert!(
                keyboard.is_grabbed(),
                "the menu asked for a grab and did not get one, so locking \
                 over it proves nothing"
            );

            let lock = session.lock();
            assert!(
                !keyboard.is_grabbed(),
                "the menu's keyboard grab outlived the lock"
            );
            assert!(
                session.app.client.popups_done > 0,
                "the menu open at the lock was not dismissed, so its chain \
                 outlives the lock"
            );
            let selections = session.app.client.selections;
            session.assert_sealed("a menu was open when the session locked", selections);
            session.assert_unlocks(lock);

            // The chain really was ended, not just ungrabbed. Left recorded
            // on the seat, this second menu would be told it is not the
            // topmost popup -- a protocol error, which kills the client and
            // fails the round trip inside `menu`.
            let _again = session
                .app
                .menu(&mut session.display, &mut session.state, &parent);
            assert!(
                keyboard.is_grabbed(),
                "after unlock, a menu could not grab: the chain from before \
                 the lock is still in the way"
            );
        }

        /// **`focus_window`, called while locked**, as a script's
        /// `sol.focus` calls it, and as a layout's event handler, a
        /// focus-follows-view workspace and xdg-activation all do -- none
        /// of which stop running when the session locks.
        #[test]
        fn focusing_a_window_while_locked_does_not_move_the_keyboard() {
            let mut session = Session::new();
            let (window, ..) = session.app.open(&mut session.display, &mut session.state);
            let lock = session.lock();
            let selections = session.app.client.selections;

            session
                .state
                .focus_window(&window, SERIAL_COUNTER.next_serial());
            session.app.pump(&mut session.display, &mut session.state);

            session.assert_sealed("focus_window while locked", selections);
            session.assert_unlocks(lock);
        }

        /// **The last line: a key cannot reach a surface the gate would
        /// have refused, even when something has got the keyboard there.**
        ///
        /// Done here the only way it can be, by calling the one method
        /// the gate exists to own -- which stands for the route nobody has
        /// found yet.
        #[test]
        fn a_key_does_not_reach_an_application_that_got_past_the_gate() {
            let mut session = Session::new();
            let (window, ..) = session.app.open(&mut session.display, &mut session.state);
            let lock = session.lock();

            let surface = window
                .wl_surface()
                .expect("the window has a surface")
                .into_owned();
            let keyboard = session
                .state
                .seat
                .get_keyboard()
                .expect("the seat has a keyboard");
            #[expect(
                clippy::disallowed_methods,
                reason = "going round the gate on purpose: this is the test of what holds when something does"
            )]
            keyboard.set_focus(
                &mut session.state,
                Some(surface),
                SERIAL_COUNTER.next_serial(),
            );
            session.app.pump(&mut session.display, &mut session.state);
            assert!(
                session.app.client.keyboard_on.is_some(),
                "the keyboard was not moved onto the application, so this \
                 proves nothing"
            );

            let (app, _) = session.type_key();
            assert!(
                !app,
                "a key typed while locked reached an application that held \
                 the keyboard: nothing at delivery checks the rule"
            );
            session.assert_unlocks(lock);
        }

        /// **The lock's holder: a second lock is refused, and the first
        /// keeps working.** `swayidle`'s `before-sleep` starting a second
        /// `swaylock` while the first is up. The second lock was granted
        /// and *replaced* the first: the real lock screen's surfaces were
        /// dropped and the keyboard taken off them, and the newcomer --
        /// which, told `finished`, would have exited -- was left the only
        /// lock. The user was locked out of their own session.
        ///
        /// Against `2b6550e`, fails at "can no longer be typed into".
        #[test]
        fn a_second_lock_is_refused_and_the_first_keeps_working() {
            let mut session = Session::new();
            session.app.open(&mut session.display, &mut session.state);
            let lock = session.lock();
            let selections = session.app.client.selections;

            let mut second = Side::connect(&mut session.display, &mut session.state);
            let (again, _) = second.ask_lock(&mut session.display, &mut session.state, false);
            let again_id = wayland_client::Proxy::id(&again);
            let refused = second.client.finished.contains(&again_id)
                && !second.client.locked.contains(&again_id);
            // What `swaylock` does with `finished`: destroy the lock, and
            // exit.
            again.destroy();
            let _gone = second.pump_or_error(&mut session.display, &mut session.state);
            drop(second);
            session
                .locker
                .pump(&mut session.display, &mut session.state);
            session.app.pump(&mut session.display, &mut session.state);

            let (app, locker) = session.type_key();
            assert!(
                locker,
                "after a second lock was asked for, the real lock screen can \
                 no longer be typed into: the user is locked out of their own \
                 session"
            );
            assert!(!app, "and the key reached the application");
            assert!(
                refused,
                "a second lock was granted while the lock client holding the \
                 session was still there"
            );
            session.assert_sealed("a second lock was asked for", selections);
            session.assert_unlocks(lock);
        }

        /// **An application cannot put up a lock screen of its own.** It
        /// asked for a lock -- which replaced the real one -- and a lock
        /// surface on its own `wl_output`, which smithay's duplicate-output
        /// check lets through because it compares one client's resources.
        /// The gate saw a surface in `Lock::surfaces` and gave it the
        /// keyboard: an application drawing a lock screen, and the
        /// password typed into it.
        ///
        /// Against `2b6550e`, fails at "was given the keyboard".
        #[test]
        fn an_application_cannot_put_up_a_lock_screen_of_its_own() {
            let mut session = Session::new();
            session.app.open(&mut session.display, &mut session.state);
            let lock = session.lock();
            let selections = session.app.client.selections;

            let (theirs, fake) =
                session
                    .app
                    .ask_lock(&mut session.display, &mut session.state, true);
            session
                .locker
                .pump(&mut session.display, &mut session.state);
            assert!(
                !fake.is_empty(),
                "the application asked for no lock surface, so this proves nothing"
            );
            assert!(
                session.app.client.keyboard_on.is_none(),
                "an application's own lock surface was given the keyboard: a \
                 lock screen that is not the lock screen, and the password goes \
                 into it"
            );
            assert!(
                session
                    .app
                    .client
                    .finished
                    .contains(&wayland_client::Proxy::id(&theirs)),
                "the application's lock was not refused"
            );
            session.assert_sealed("an application put up a lock screen of its own", selections);
            session.assert_unlocks(lock);
        }

        /// **Nor in the moment before the real one maps.** The session is
        /// locked the instant the lock client asks, before it has drawn
        /// anything -- and a monitor plugged in while locked is another
        /// monitor the real lock screen has not covered yet.
        ///
        /// Against `2b6550e`, fails at "before the lock screen mapped".
        #[test]
        fn an_application_cannot_put_up_a_lock_screen_before_the_real_one_maps() {
            let mut session = Session::new();
            session.app.open(&mut session.display, &mut session.state);
            let (lock, _) =
                session
                    .locker
                    .ask_lock(&mut session.display, &mut session.state, false);
            assert!(
                session.state.lock.is_some(),
                "the lock client asked and the session did not lock, so this \
                 proves nothing"
            );

            let (_theirs, _fake) =
                session
                    .app
                    .ask_lock(&mut session.display, &mut session.state, true);
            session
                .locker
                .pump(&mut session.display, &mut session.state);
            assert!(
                session.app.client.keyboard_on.is_none(),
                "in the moment before the lock screen mapped, an application's \
                 own lock surface was given the keyboard"
            );

            // The real lock screen arrives.
            let compositor = session
                .locker
                .client
                .compositor
                .clone()
                .expect("wl_compositor bound");
            let output = session
                .locker
                .client
                .output
                .clone()
                .expect("wl_output bound");
            let surface = compositor.create_surface(&session.locker.qh, ());
            let role = lock.get_lock_surface(&surface, &output, &session.locker.qh, ());
            session.lock_surfaces = vec![LockSurfaceProxy { surface, role }];
            session
                .locker
                .pump(&mut session.display, &mut session.state);
            session.app.pump(&mut session.display, &mut session.state);
            // And is on the monitor, so the lock client is told `locked`,
            // without which its unlock below would be refused
            // (`an_unlock_before_locked_unlocks_nothing`).
            session.show_all();
            assert!(
                session.told_locked(&lock),
                "the lock screen is on every monitor and was not told `locked`"
            );

            let selections = session.app.client.selections;
            session.assert_sealed(
                "an application's lock surface came before the real one",
                selections,
            );
            session.assert_unlocks(lock);
        }

        /// **An unlock from a lock that was never granted unlocks
        /// nothing.** `unlock` is not told which lock asked, and smithay
        /// 0.7 calls it for `unlock_and_destroy` on any lock object: it
        /// posts `invalid_unlock` on one that was never told `locked`, and
        /// then, with no `return` after the error, unlocks anyway. So any
        /// client could lock and then unlock -- two requests.
        ///
        /// Against `2b6550e`, fails at "in two requests". Against a fix
        /// that only refused the second lock, it fails at the same line,
        /// through smithay's missing `return`.
        #[test]
        fn an_unlock_from_a_lock_that_was_never_granted_unlocks_nothing() {
            let mut session = Session::new();
            session.app.open(&mut session.display, &mut session.state);
            let lock = session.lock();
            let selections = session.app.client.selections;

            let mut intruder = Side::connect(&mut session.display, &mut session.state);
            let (theirs, _) = intruder.ask_lock(&mut session.display, &mut session.state, false);
            theirs.unlock_and_destroy();
            let error = intruder.pump_or_error(&mut session.display, &mut session.state);

            assert!(
                session.state.lock.is_some(),
                "a client that does not hold the lock unlocked the session in \
                 two requests"
            );
            let error = error.expect(
                "the intruder was not disconnected: an unlock on a lock that \
                 was never told `locked` is `invalid_unlock`",
            );
            // The code only. `unlock_and_destroy` is a destructor, so by
            // the time the error arrives the client has already forgotten
            // the object it is about and cannot say which interface it was.
            assert_eq!(
                error.code,
                ext_session_lock_v1::Error::InvalidUnlock as u32,
                "disconnected, but not for `invalid_unlock`: {error:?}"
            );
            session
                .locker
                .pump(&mut session.display, &mut session.state);
            session.app.pump(&mut session.display, &mut session.state);
            session.assert_sealed("an unlock from a lock that was never granted", selections);
            // And the holder's own unlock is untouched by all of it.
            session.assert_unlocks(lock);
        }

        /// **The lock client crashing leaves the session locked**, and a
        /// new lock client can take over. The protocol says a lock client
        /// dying must not unlock the session. What holds the lock then is
        /// nobody, and that must not turn into anybody: a lock refused
        /// while the holder lived stays refused after it has died.
        ///
        /// Against `2b6550e`, fails at "was granted": the rival's lock
        /// simply replaced the real one, and its unlock then worked
        /// whether the real lock client crashed or not.
        #[test]
        fn the_lock_client_crashing_leaves_the_session_locked() {
            let mut session = Session::new();
            session.app.open(&mut session.display, &mut session.state);
            let _crashing = session.lock();
            let selections = session.app.client.selections;

            let mut rival = Side::connect(&mut session.display, &mut session.state);
            let (refused, _) = rival.ask_lock(&mut session.display, &mut session.state, false);
            assert!(
                rival
                    .client
                    .finished
                    .contains(&wayland_client::Proxy::id(&refused)),
                "a second lock was granted while the lock client holding the \
                 session was still there"
            );

            // The lock client crashes: its connection closes with the
            // session still locked. Its replacement is connected first,
            // the way one is started from another terminal.
            let recovery = Side::connect(&mut session.display, &mut session.state);
            let crashed = std::mem::replace(&mut session.locker, recovery);
            session.lock_surfaces.clear();
            drop(crashed);
            session.app.pump(&mut session.display, &mut session.state);
            session
                .locker
                .pump(&mut session.display, &mut session.state);

            assert!(
                session.state.lock.is_some(),
                "the lock client crashed and the session unlocked"
            );
            assert!(
                session.app.client.keyboard_on.is_none(),
                "the lock client crashed and the application was given the keyboard"
            );
            let (app, _) = session.type_key();
            assert!(
                !app,
                "the lock client crashed and a key reached the application"
            );

            refused.unlock_and_destroy();
            let _gone = rival.pump_or_error(&mut session.display, &mut session.state);
            assert!(
                session.state.lock.is_some(),
                "with the lock client dead, a lock refused while it lived \
                 unlocked the session"
            );

            // Recovered: a new lock client takes over, and unlocks.
            let lock = session.lock();
            session.assert_sealed("a new lock client took over from a crashed one", selections);
            session.assert_unlocks(lock);
        }

        /// **A mode active when the session locks does not garble the
        /// password.** Presses reached the lock screen, but the filter
        /// intercepted every release while a mode was active, so the lock
        /// client auto-repeated each key it was never told came up -- and
        /// with bindings off while locked, the mode could not be left.
        ///
        /// Against `2b6550e`, fails at "never told it came up".
        #[test]
        fn a_mode_active_at_the_lock_does_not_garble_the_password() {
            let mut session = Session::new();
            session.app.open(&mut session.display, &mut session.state);
            // A mode owns input, the way a script's overview or resize
            // mode takes it.
            session.state.script_grab = true;
            let lock = session.lock();

            session.locker.client.key_events.clear();
            session.app.client.keys.clear();
            // `a`, which is evdev 30 and 38 to xkb.
            crate::input::key(&mut session.state, Keycode::new(38), KeyState::Pressed, 1);
            crate::input::key(&mut session.state, Keycode::new(38), KeyState::Released, 2);
            session
                .locker
                .pump(&mut session.display, &mut session.state);
            session.app.pump(&mut session.display, &mut session.state);
            assert_eq!(
                session.locker.client.key_events,
                vec![(30, true), (30, false)],
                "the lock screen was told a key went down and never told it \
                 came up, so it repeats it: a mode active at the lock garbles \
                 every key of the password"
            );
            assert!(
                session.app.client.keys.is_empty(),
                "and the key reached the application"
            );

            // The mode ends; the rest is the ordinary unlock.
            session.state.script_grab = false;
            session.assert_unlocks(lock);
        }

        /// **A lock surface that dies hands the keyboard to one that did
        /// not.** A monitor unplugged or a laptop undocked while locked:
        /// the lock client destroys that monitor's surface when its output
        /// goes. If the keyboard was on it, nothing moved it -- `focus_lock`
        /// runs only when a surface maps, and `settle_focus` refused every
        /// window without offering the lock screen instead -- so keys went
        /// nowhere until another surface arrived.
        ///
        /// Against `2b6550e`, fails at "went nowhere".
        #[test]
        fn a_lock_surface_that_dies_hands_the_keyboard_to_one_that_did_not() {
            let mut session = Session::with_monitors(2);
            session.app.open(&mut session.display, &mut session.state);
            let lock = session.lock();
            assert_eq!(
                session.lock_surfaces.len(),
                2,
                "not a lock surface on each of two monitors, so this proves nothing"
            );

            let focused = session
                .locker
                .client
                .keyboard_on
                .clone()
                .expect("the lock screen has the keyboard");
            let (gone, left): (Vec<_>, Vec<_>) = std::mem::take(&mut session.lock_surfaces)
                .into_iter()
                .partition(|each| wayland_client::Proxy::id(&each.surface) == focused);
            let gone = gone
                .into_iter()
                .next()
                .expect("the keyboard is on one of the lock screen's surfaces");
            gone.destroy();
            session
                .locker
                .pump(&mut session.display, &mut session.state);
            session.app.pump(&mut session.display, &mut session.state);

            let survivor = left
                .first()
                .map(|each| wayland_client::Proxy::id(&each.surface));
            assert_eq!(
                session.locker.client.keyboard_on, survivor,
                "the lock surface the keyboard was on went away and the \
                 keyboard went nowhere: typing at the lock screen does nothing \
                 until another surface maps"
            );
            session.lock_surfaces = left;
            let selections = session.app.client.selections;
            session.assert_sealed("the focused lock surface died", selections);
            session.assert_unlocks(lock);
        }

        /// **The key that unlocks is not handed to the window.** The Enter
        /// that submits the password is still held when the lock client
        /// unlocks, and the keyboard went straight back to the window,
        /// whose `wl_keyboard.enter` then said Enter was held -- a key it
        /// never saw pressed, typed at the lock screen.
        ///
        /// Against `2b6550e`, fails at "told a key is held".
        #[test]
        fn the_key_that_unlocks_is_not_handed_to_the_window() {
            let mut session = Session::new();
            session.app.open(&mut session.display, &mut session.state);
            let lock = session.lock();

            // Enter, which is evdev 28 and 36 to xkb, down at the lock
            // screen when the lock client unlocks.
            crate::input::key(&mut session.state, Keycode::new(36), KeyState::Pressed, 1);
            lock.unlock_and_destroy();
            session
                .locker
                .pump(&mut session.display, &mut session.state);
            session.app.pump(&mut session.display, &mut session.state);
            assert!(
                session.state.lock.is_none(),
                "the lock client unlocked and the session did not"
            );
            assert!(
                session.app.client.keyboard_on.is_none()
                    || !session.app.client.enter_keys.contains(&28),
                "the window was told a key is held that it never saw pressed: \
                 the Enter typed at the lock screen"
            );

            session.app.client.keys.clear();
            crate::input::key(&mut session.state, Keycode::new(36), KeyState::Released, 2);
            session.app.pump(&mut session.display, &mut session.state);
            assert!(
                session.app.client.keyboard_on.is_some(),
                "the key came up and the keyboard did not go back to the window"
            );
            assert!(
                !session.app.client.enter_keys.contains(&28),
                "the window was told a key is held that it never saw pressed"
            );
            assert!(
                !session.app.client.keys.contains(&28),
                "the window was sent the release of a key it never saw pressed"
            );
            let (app, _) = session.type_key();
            assert!(app, "unlocked, and typing does not reach the application");
        }

        /// **Nothing captures the screen while the session is locked.**
        /// What is on it then is the lock screen, and a recording of that
        /// is the password's length and the rhythm it was typed at. Every
        /// client but the lock client is behind the lock and any of them
        /// can ask. Both ways in are driven: a capture asked for while
        /// locked, and one asked for before the lock and copied after. A
        /// capture already *queued* when the session locks is refused in
        /// `screencopy::settle`, which needs a renderer and is not reached
        /// from here.
        ///
        /// Against `2b6550e`, fails at "was offered a buffer".
        #[test]
        fn nothing_captures_the_screen_while_the_session_is_locked() {
            let mut session = Session::new();
            session.app.open(&mut session.display, &mut session.state);
            let manager = session
                .app
                .client
                .screencopy
                .clone()
                .expect("zwlr_screencopy_manager_v1 bound");
            let output = session.app.client.output.clone().expect("wl_output bound");
            let before = manager.capture_output(0, &output, &session.app.qh, ());
            session.app.pump(&mut session.display, &mut session.state);
            assert_eq!(
                session.app.client.captures_offered, 1,
                "unlocked, a capture was not offered a buffer, so refusing one \
                 proves nothing"
            );
            let lock = session.lock();

            let _during = manager.capture_output(0, &output, &session.app.qh, ());
            session.app.pump(&mut session.display, &mut session.state);
            assert_eq!(
                session.app.client.captures_offered, 1,
                "a capture asked for while locked was offered a buffer: an \
                 application can record the lock screen"
            );
            assert_eq!(
                session.app.client.captures_failed, 1,
                "and it was never told it failed"
            );

            let shm = session.app.client.shm.clone().expect("wl_shm bound");
            let fd = anon_file(64 * 64 * 4);
            let pool = shm.create_pool(fd.as_fd(), 64 * 64 * 4, &session.app.qh, ());
            let buffer = pool.create_buffer(
                0,
                64,
                64,
                64 * 4,
                wl_shm::Format::Xrgb8888,
                &session.app.qh,
                (),
            );
            before.copy(&buffer);
            session.app.pump(&mut session.display, &mut session.state);
            assert_eq!(
                session.app.client.captures_failed, 2,
                "a capture asked for before the lock and copied after it was \
                 not refused"
            );

            session.assert_unlocks(lock);
        }

        /// **A menu that closes lets go of its grab.** `popup_grab` was set
        /// when a menu grabbed and cleared only by `release_grabs` at the
        /// next lock, so a menu closed an hour before was still recorded
        /// as the chain holding the seat when the session next locked.
        /// Harmless as things stand -- see `popup_destroyed` -- and tested
        /// because the field's one reader is the lock.
        ///
        /// Against `2b6550e`, fails at "still recorded".
        #[test]
        fn a_menu_that_closes_lets_go_of_its_grab() {
            let mut session = Session::new();
            let (_window, _toplevel, _surface, parent) =
                session.app.open(&mut session.display, &mut session.state);
            let menu = session
                .app
                .menu(&mut session.display, &mut session.state, &parent);
            assert!(
                session.state.popup_grab.is_some(),
                "the menu's grab was never recorded, so this proves nothing"
            );

            menu.destroy();
            session.app.pump(&mut session.display, &mut session.state);
            assert!(
                session.state.popup_grab.is_none(),
                "a menu closed and its grab is still recorded as the one \
                 holding the seat"
            );
        }

        /// **A close in flight across a lock and its unlock never hands a
        /// window the keyboard.** #127 gives a closing window back from
        /// three places -- a refusal's deadline, a dialog answering the
        /// close, and `settle_closing`'s retry -- and every one of them
        /// ends in `settle_focus`, the call that once handed a locked
        /// session's keyboard to the top application.
        ///
        /// Locked, each step is measured the way every route in this module
        /// is (`assert_sealed`): the close's deadline, a dialog's answer, a
        /// refusal's deadline. Then the same two closes are in flight when
        /// the lock client unlocks with Enter still down, which is
        /// `refocus_on_release`'s tenth of a second. A dialog and a
        /// refusal both land in it, and neither may give a window the
        /// keyboard until the key is up, or that window is told in
        /// `wl_keyboard.enter` that Enter is held. The retry goes through
        /// the same `give_back` and needs a busy transform slot to reach;
        /// it is not driven here.
        ///
        /// Against this merge without `settle_focus`'s
        /// `refocus_on_release` return, fails at "a dialog's answer was
        /// handed the keyboard while the unlock key was held".
        #[test]
        fn a_close_in_flight_across_a_lock_never_takes_the_keyboard() {
            let mut session = Session::new();
            let (refused, _refused_top, ..) =
                session.app.open(&mut session.display, &mut session.state);
            let (answered, answered_top, ..) =
                session.app.open(&mut session.display, &mut session.state);
            let refused = session
                .state
                .panes
                .id_of(&refused)
                .expect("an open window has a pane");
            let answered = session
                .state
                .panes
                .id_of(&answered)
                .expect("an open window has a pane");
            let leaving = |state: &Solium, pane: crate::pane::PaneId| {
                state.panes.get(pane).is_some_and(Pane::leaving)
            };

            // super+q on both, and the lock screen straight after.
            session.state.close_pane(refused);
            session.state.close_pane(answered);
            let lock = session.lock();
            let selections = session.app.client.selections;

            // The close's deadline: the requests go out.
            session
                .state
                .clock
                .advance(present::CLOSING + Duration::from_millis(10));
            let asked = session.state.clock.now();
            session.state.settle_closing(asked);
            session.app.pump(&mut session.display, &mut session.state);
            assert!(
                [refused, answered].into_iter().all(|pane| {
                    session
                        .state
                        .panes
                        .get(pane)
                        .is_some_and(|pane| pane.asked_at().is_some())
                }),
                "both closes were asked, or nothing below is a close in flight"
            );
            session.assert_sealed("a close's deadline passed while locked", selections);

            // One of them answered with a dialog, which gives it back.
            let (_dialog, dialog_top, ..) =
                session.app.open(&mut session.display, &mut session.state);
            dialog_top.set_parent(Some(&answered_top));
            session.app.pump(&mut session.display, &mut session.state);
            assert!(
                !leaving(&session.state, answered),
                "the dialog did not give its parent back, so this proves nothing"
            );
            session.assert_sealed("a dialog answered a close while locked", selections);

            // The other said nothing, and its deadline gives it back.
            session.state.clock.advance(Duration::from_millis(1100));
            let now = session.state.clock.now();
            session.state.settle_refused(now);
            session.app.pump(&mut session.display, &mut session.state);
            assert!(
                !leaving(&session.state, refused),
                "the refusal's deadline did not give the window back, so this \
                 proves nothing"
            );
            session.assert_sealed("a refused close was given back while locked", selections);

            // Both closed again and asked, a second dialog open to answer
            // one of them -- opened while locked, so it takes no keyboard --
            // and Enter (evdev 28, 36 to xkb) down at the lock screen when
            // it unlocks.
            let (_second, second_top, ..) =
                session.app.open(&mut session.display, &mut session.state);
            session.state.close_pane(refused);
            session.state.close_pane(answered);
            session
                .state
                .clock
                .advance(present::CLOSING + Duration::from_millis(10));
            let asked = session.state.clock.now();
            session.state.settle_closing(asked);
            crate::input::key(&mut session.state, Keycode::new(36), KeyState::Pressed, 3);
            lock.unlock_and_destroy();
            session
                .locker
                .pump(&mut session.display, &mut session.state);
            session.app.pump(&mut session.display, &mut session.state);
            assert!(
                session.state.lock.is_none() && session.state.refocus_on_release,
                "the premise: unlocked, with the keyboard waiting for the key \
                 that unlocked to come up"
            );
            assert!(
                session.app.client.keyboard_on.is_none(),
                "the premise: nothing has the keyboard while the key is down"
            );

            second_top.set_parent(Some(&answered_top));
            session.app.pump(&mut session.display, &mut session.state);
            assert!(
                !leaving(&session.state, answered),
                "the second dialog did not give its parent back, so this \
                 proves nothing"
            );
            assert!(
                session.app.client.keyboard_on.is_none(),
                "a dialog's answer was handed the keyboard while the unlock key \
                 was held, and its window told Enter is down: {:?}",
                session.app.client.enter_keys
            );

            session.state.clock.advance(Duration::from_millis(1100));
            let now = session.state.clock.now();
            session.state.settle_refused(now);
            session.app.pump(&mut session.display, &mut session.state);
            assert!(
                !leaving(&session.state, refused),
                "the refusal's deadline did not give the window back, so this \
                 proves nothing"
            );
            assert!(
                session.app.client.keyboard_on.is_none(),
                "a refusal's deadline was handed the keyboard while the unlock \
                 key was held, and its window told Enter is down: {:?}",
                session.app.client.enter_keys
            );

            // The key comes up, and only now does a window take the keyboard.
            session.app.client.keys.clear();
            crate::input::key(&mut session.state, Keycode::new(36), KeyState::Released, 4);
            session.app.pump(&mut session.display, &mut session.state);
            assert!(
                session.app.client.keyboard_on.is_some(),
                "the key came up and the keyboard did not go back to a window"
            );
            assert!(
                !session.app.client.enter_keys.contains(&28),
                "the window was told a key is held that it never saw pressed"
            );
            assert!(
                !session.app.client.keys.contains(&28),
                "the window was sent the release of a key it never saw pressed"
            );
            let (app, _) = session.type_key();
            assert!(app, "unlocked, and typing does not reach the application");
        }

        /// **A window closed with a menu open takes the menu's grab with
        /// it.** #127 takes the keyboard off a closing window once its
        /// animation lands, by giving the keyboard to nothing. With a menu
        /// open that did nothing: the menu's keyboard grab ignores every
        /// `set_focus` until its chain ends, and `focused_window` answers
        /// for the menu with its window, so `settle_focus` saw a focused
        /// window and left it. The keyboard stayed on the closed window's
        /// menu for the whole grace period, and every key typed after the
        /// close went to it.
        /// `release_grabs_of` is the fix, and `release_grabs` -- which
        /// does the same for everyone when the session locks -- the model.
        ///
        /// Against this merge without `release_grabs_of` in
        /// `hand_off_keyboard`, fails at "the menu's keyboard grab
        /// outlived its window's close".
        #[test]
        fn a_close_with_a_menu_open_takes_the_keyboard_off_the_menu() {
            let mut session = Session::new();
            let (kept, ..) = session.app.open(&mut session.display, &mut session.state);
            let (closing, _toplevel, _surface, parent) =
                session.app.open(&mut session.display, &mut session.state);
            let _menu = session
                .app
                .menu(&mut session.display, &mut session.state, &parent);
            let keyboard = session
                .state
                .seat
                .get_keyboard()
                .expect("the seat has a keyboard");
            assert!(
                keyboard.is_grabbed() && session.state.is_focused(&closing),
                "the premise: a menu open on the focused window, holding a grab"
            );

            let pane = session
                .state
                .panes
                .id_of(&closing)
                .expect("an open window has a pane");
            session.state.close_pane(pane);
            session
                .state
                .clock
                .advance(present::CLOSING + Duration::from_millis(10));
            let asked = session.state.clock.now();
            session.state.settle_closing(asked);
            session.app.pump(&mut session.display, &mut session.state);

            assert!(
                !keyboard.is_grabbed(),
                "the menu's keyboard grab outlived its window's close"
            );
            assert!(
                session.app.client.popups_done > 0,
                "the closed window's menu was left open"
            );
            assert_eq!(
                session.state.focused_window().as_ref(),
                Some(&kept),
                "the keyboard did not move to the window that is still drawn"
            );
            let (app, _) = session.type_key();
            assert!(app, "the key reached nobody at all");
            assert_eq!(
                session
                    .app
                    .client
                    .typed
                    .last()
                    .and_then(|(surface, _)| *surface),
                Some(surface_id(&kept)),
                "a key typed after the close did not reach the window that is \
                 drawn"
            );
        }

        /// **The rule the X11 clipboard bridge asks: no X11 client may
        /// read the selection while the session is locked.** There is no
        /// focus to gate on the X11 side of the bridge, and every X11
        /// client is behind the lock.
        ///
        /// What this does *not* cover, said plainly because its first
        /// version claimed it did: the bridge itself. It asks
        /// `Solium::x11_may_read_selection` under a real lock taken by a
        /// real lock client, and that is all. The one line that connects
        /// the rule to X11 -- `XwmHandler::allow_selection_access` in
        /// `xwayland.rs`, which returns it -- is not called here, because
        /// it cannot be: it takes an `XwmId`, which smithay constructs only
        /// inside a running X11 window manager, and there is no X server in
        /// a unit test. Replacing that line with `true` would leave this
        /// green. There is no X11 client here either.
        #[test]
        fn the_x11_selection_rule_refuses_while_the_session_is_locked() {
            let mut session = Session::new();
            session.app.open(&mut session.display, &mut session.state);
            assert!(
                session.state.x11_may_read_selection(),
                "unlocked, the two halves of the session paste into each other"
            );
            let lock = session.lock();
            assert!(
                !session.state.x11_may_read_selection(),
                "locked, an X11 client could read whatever the lock screen copied"
            );
            session.assert_unlocks(lock);
            assert!(
                session.state.x11_may_read_selection(),
                "and unlocked again, it can"
            );
        }

        /// **#126 with #130: a window that closes itself behind the lock
        /// hands the keyboard to nobody but the lock.**
        ///
        /// The window goes on fading as it would unlocked -- it is drawn
        /// by nothing while the screen is locked, `render::elements`
        /// returns before any window -- and what it leaves has no surface
        /// to be given the keyboard. The window going is a change to the
        /// window list, which is when `settle_focus` runs, and while locked
        /// that is the lock's own settle and nothing else.
        #[test]
        fn a_window_closing_itself_behind_the_lock_leaves_the_keyboard_on_it() {
            let mut session = Session::new();
            let _below = session.app.open(&mut session.display, &mut session.state);
            let (window, going, _, _) = session.app.open(&mut session.display, &mut session.state);
            let pane = session.state.panes.id_of(&window).expect("a pane");
            let selections = session.app.client.selections;
            let _lock = session.lock();

            going.destroy();
            session.app.pump(&mut session.display, &mut session.state);
            session.state.space.refresh();
            session.state.sync_panes();
            session.app.pump(&mut session.display, &mut session.state);
            assert!(
                session.state.panes.get(pane).is_some_and(Pane::ghost),
                "the premise: the window is fading out"
            );
            session.assert_sealed("a window that closed itself", selections);
        }

        /// **`locked` is not sent in the dispatch that asked for the lock**,
        /// and everything else still is: `lock` set, the next frame built
        /// under it, a frame asked for, the keyboard taken off the
        /// application. Only the promise waits, because nothing has reached
        /// a screen yet -- and `locked` used to go out right here, with the
        /// monitor still scanning out the desktop.
        ///
        /// Against this branch with `lock` confirming at once, as it did
        /// before, fails at "before any monitor had shown the lock".
        #[test]
        fn locked_is_not_sent_in_the_dispatch_that_asked_for_it() {
            let mut session = Session::new();
            session.app.open(&mut session.display, &mut session.state);
            assert!(
                session.app.client.keyboard_on.is_some(),
                "the application has the keyboard before the lock, or taking \
                 it away proves nothing"
            );
            session.state.redraw = false;

            let (lock, surfaces) =
                session
                    .locker
                    .ask_lock(&mut session.display, &mut session.state, true);
            session.lock_surfaces = surfaces;
            session.app.pump(&mut session.display, &mut session.state);

            assert!(
                session.state.lock.is_some(),
                "the lock client asked and the session did not lock"
            );
            assert!(
                session.state.lock_frame().is_some(),
                "the next frame is not built under the lock: the blanking waits \
                 as well"
            );
            assert!(
                session.state.redraw,
                "no frame was asked for, so nothing replaces the desktop"
            );
            assert!(
                session.app.client.keyboard_on.is_none(),
                "the application kept the keyboard while `locked` waits"
            );
            assert!(
                !session
                    .locker
                    .client
                    .locked
                    .contains(&wayland_client::Proxy::id(&lock)),
                "the lock client was told `locked` in the dispatch that asked \
                 for it, before any monitor had shown the lock"
            );
        }

        /// **`locked` waits for every monitor to show the lock.** One
        /// monitor showing it, however many frames it shows, is one monitor:
        /// the other is still scanning out whatever it had.
        ///
        /// Against the old confirmation, fails at "had shown nothing".
        #[test]
        fn locked_waits_for_every_monitor_to_show_the_lock() {
            let mut session = Session::with_monitors(2);
            let (left, right) = (session.output(0), session.output(1));
            let (lock, surfaces) =
                session
                    .locker
                    .ask_lock(&mut session.display, &mut session.state, true);
            session.lock_surfaces = surfaces;

            for _ in 0..3 {
                session.show(&left);
            }
            assert!(
                !session.told_locked(&lock),
                "the lock client was told `locked` while the right-hand monitor \
                 had shown nothing of the lock"
            );
            session.show(&right);
            assert!(
                session.told_locked(&lock),
                "every monitor showed the lock and the lock client was not told \
                 `locked`"
            );
        }

        /// **A flip already in flight when the session locks does not
        /// count**, and nor does a frame built under the lock this one took
        /// over from. `tty.rs` skips a screen whose flip is pending, so the
        /// first flip to complete after a lock can be a frame queued before
        /// it: the desktop. Each is handed back here the way a vblank hands
        /// it back, with the lock it was built under, before the lock's own
        /// frame.
        ///
        /// What this plays is `Solium`'s half. The tty half -- that each
        /// frame's own user data comes back out of `frame_submitted` -- needs
        /// a GPU and is not reached here.
        ///
        /// Against the old confirmation, fails at "on the desktop's flip".
        /// Against a fix that counted any frame presented after the lock,
        /// fails at the same line.
        #[test]
        fn a_flip_already_in_flight_at_the_lock_does_not_count() {
            let mut session = Session::new();
            session.app.open(&mut session.display, &mut session.state);
            let monitor = session.output(0);
            // Built, and queued, before the lock.
            let in_flight = session.state.lock_frame();
            assert!(
                in_flight.is_none(),
                "the premise: a frame built before the lock carries no lock"
            );

            let (first, surfaces) =
                session
                    .locker
                    .ask_lock(&mut session.display, &mut session.state, true);
            session.lock_surfaces = surfaces;
            session.state.frame_presented(&monitor, in_flight);
            assert!(
                !session.told_locked(&first),
                "the lock client was told `locked` on the desktop's flip: the \
                 frame that reached the screen was built before the lock"
            );
            session.show(&monitor);
            assert!(
                session.told_locked(&first),
                "the lock's own frame reached the screen and the lock client \
                 was not told `locked`"
            );

            // The lock client crashes with a frame of its lock in flight, and
            // a new one takes over before that frame flips.
            let in_flight = session.state.lock_frame();
            let recovery = Side::connect(&mut session.display, &mut session.state);
            let crashed = std::mem::replace(&mut session.locker, recovery);
            session.lock_surfaces.clear();
            drop(crashed);
            session.app.pump(&mut session.display, &mut session.state);
            let (second, surfaces) =
                session
                    .locker
                    .ask_lock(&mut session.display, &mut session.state, true);
            session.lock_surfaces = surfaces;
            assert!(
                session.state.lock_frame().is_some() && session.state.lock_frame() != in_flight,
                "the premise: the new lock is not the one that frame was built under"
            );
            session.state.frame_presented(&monitor, in_flight);
            assert!(
                !session.told_locked(&second),
                "the new lock client was told `locked` on a frame built under \
                 the lock it took over from"
            );
            session.show(&monitor);
            assert!(
                session.told_locked(&second),
                "the new lock's own frame reached the screen and its lock client \
                 was not told `locked`"
            );
        }

        /// **A monitor switched off does not hold `locked` back.** Solium
        /// has no DPMS or idle power-off: the one way a monitor goes dark is
        /// `enabled = false`, and `tty.rs` drops a monitor switched off that
        /// way just as it drops one unplugged. A lock asked for after that
        /// waits for the monitor that is still on, and for nothing else.
        ///
        /// Against the old confirmation, fails at "the one still on had
        /// shown".
        #[test]
        fn a_switched_off_monitor_does_not_hold_locked_back() {
            let mut session = Session::with_monitors(2);
            let on = session.output(0);
            session.unplug(1);
            session
                .locker
                .pump(&mut session.display, &mut session.state);

            let (lock, surfaces) =
                session
                    .locker
                    .ask_lock(&mut session.display, &mut session.state, true);
            session.lock_surfaces = surfaces;
            assert!(
                !session.told_locked(&lock),
                "the lock client was told `locked` before the one still on had \
                 shown the lock"
            );
            session.show(&on);
            assert!(
                session.told_locked(&lock),
                "the monitor still on showed the lock, and `locked` waits for \
                 the one switched off"
            );
        }

        /// **A monitor unplugged while `locked` waits does not hold it back
        /// for ever.** The left-hand one has shown the lock, the right-hand
        /// one never will because it has gone, and `locked` goes out as it
        /// goes, with no frame after.
        ///
        /// Against the old confirmation, fails at "had shown nothing".
        #[test]
        fn a_monitor_unplugged_while_locking_does_not_hold_locked_back() {
            let mut session = Session::with_monitors(2);
            let left = session.output(0);
            let (lock, surfaces) =
                session
                    .locker
                    .ask_lock(&mut session.display, &mut session.state, true);
            session.lock_surfaces = surfaces;
            session.show(&left);
            assert!(
                !session.told_locked(&lock),
                "the lock client was told `locked` while the right-hand monitor \
                 had shown nothing of the lock"
            );

            session.unplug(1);
            assert!(
                session.told_locked(&lock),
                "the monitor `locked` was waiting for was unplugged, and the lock \
                 client is waiting still"
            );
        }

        /// **With no monitor, `locked` goes out at once**: there is no
        /// screen for anything to be on. Both ways there can be none --
        /// none when the lock is asked for, and the only one unplugged
        /// before it showed the lock.
        ///
        /// The first half is what the old confirmation did as well, and
        /// passes against it. The second fails against it at "the
        /// premise".
        #[test]
        fn with_no_monitor_locked_goes_out_at_once() {
            let mut session = Session::with_monitors(0);
            let (lock, _) = session
                .locker
                .ask_lock(&mut session.display, &mut session.state, true);
            assert!(
                session
                    .locker
                    .client
                    .locked
                    .contains(&wayland_client::Proxy::id(&lock)),
                "with no monitor to wait for, the lock client was not told \
                 `locked` in the dispatch that asked for it"
            );

            let mut session = Session::new();
            let (lock, surfaces) =
                session
                    .locker
                    .ask_lock(&mut session.display, &mut session.state, true);
            session.lock_surfaces = surfaces;
            assert!(
                !session.told_locked(&lock),
                "the premise: with a monitor, `locked` waits for it"
            );
            session.unplug(0);
            assert!(
                session.told_locked(&lock),
                "the only monitor was unplugged before it showed the lock, and \
                 the lock client is waiting still"
            );
        }

        /// **A monitor plugged in while `locked` waits must show the lock
        /// too.** It is a screen like the others, and `locked` is a promise
        /// about every screen there is.
        ///
        /// Against the old confirmation, fails at "had shown nothing".
        #[test]
        fn a_monitor_plugged_in_while_locking_must_show_the_lock_too() {
            let mut session = Session::new();
            let first = session.output(0);
            let (lock, surfaces) =
                session
                    .locker
                    .ask_lock(&mut session.display, &mut session.state, true);
            session.lock_surfaces = surfaces;

            let second = session.plug();
            session.show(&first);
            assert!(
                !session.told_locked(&lock),
                "the lock client was told `locked` while the monitor plugged in \
                 had shown nothing of the lock"
            );
            session.show(&second);
            assert!(
                session.told_locked(&lock),
                "both monitors showed the lock and the lock client was not told \
                 `locked`"
            );
        }

        /// **A lock client that goes before it is told `locked` leaves the
        /// session locked**, and a new one can take over. Both ways it can
        /// go: giving up with `destroy`, which the protocol allows before
        /// `locked`, and crashing. Neither unlocks anything, the `locked` it
        /// was owed is kept and never spent, and the lock client that takes
        /// over is told `locked` for its own lock once that is on the monitor.
        ///
        /// What this cannot see is what the owed `locked` sends when it is
        /// dropped at the takeover: nothing, but a client discards events on
        /// an object it has destroyed and a crashed one reads nothing, so no
        /// client here could tell. That half rests on `destroyed` in
        /// `lock.rs`.
        ///
        /// Against the old confirmation, fails at "the premise". With
        /// `confirm_lock` confirming a lock whose client has gone, fails at
        /// "was spent".
        #[test]
        fn a_lock_client_that_goes_before_locked_leaves_the_session_locked() {
            for crash in [false, true] {
                let route = if crash { "crashed" } else { "gave up" };
                let mut session = Session::new();
                session.app.open(&mut session.display, &mut session.state);
                let selections = session.app.client.selections;
                let (lock, surfaces) =
                    session
                        .locker
                        .ask_lock(&mut session.display, &mut session.state, true);
                assert!(
                    !session.told_locked(&lock),
                    "{route}: the premise: the lock client has not been told \
                     `locked`"
                );

                // Its replacement is connected first either way, the way one
                // is started from another terminal. One that gave up stays
                // connected throughout.
                let recovery = Side::connect(&mut session.display, &mut session.state);
                let mut going = std::mem::replace(&mut session.locker, recovery);
                let _gave_up = if crash {
                    drop(going);
                    None
                } else {
                    for surface in &surfaces {
                        surface.destroy();
                    }
                    lock.destroy();
                    let error = going.pump_or_error(&mut session.display, &mut session.state);
                    assert!(
                        error.is_none(),
                        "{route}: `destroy` before `locked` is legal, and the lock \
                         client was disconnected for it: {error:?}"
                    );
                    Some(going)
                };
                session.app.pump(&mut session.display, &mut session.state);
                // Frames go on reaching the screen, and there is nobody to
                // tell.
                session.show_all();
                session.app.pump(&mut session.display, &mut session.state);
                assert!(
                    session
                        .state
                        .lock
                        .as_ref()
                        .is_some_and(crate::lock::Lock::pending),
                    "{route}: the `locked` owed to a lock client that has gone was \
                     spent, on nobody"
                );

                assert!(
                    session.state.lock.is_some(),
                    "{route}: the lock client went before `locked` and the \
                     session unlocked"
                );
                assert!(
                    session.app.client.keyboard_on.is_none(),
                    "{route}: and the application was given the keyboard"
                );
                let (app, _) = session.type_key();
                assert!(!app, "{route}: and a key reached the application");

                let lock = session.lock();
                session.assert_sealed(&format!("{route}, then a new lock took over"), selections);
                session.assert_unlocks(lock);
            }
        }

        /// **An unlock before `locked` unlocks nothing**, from the lock's
        /// own holder as from anyone. It is `invalid_unlock` -- the
        /// protocol's error for unlocking a lock never told `locked` -- and
        /// smithay posts it and then unlocks anyway, for the holder as for
        /// the intruder in
        /// `an_unlock_from_a_lock_that_was_never_granted_unlocks_nothing`.
        /// While `locked` went out in the dispatch that granted the lock the
        /// holder could not get here; now it can, and a lock client thrown
        /// off for a protocol error leaves the session locked like one that
        /// crashed.
        ///
        /// Against the old confirmation, fails at "the premise". With the
        /// confirmation fixed and the holder's unlock passed to smithay
        /// untouched, fails at "unlocked the session".
        #[test]
        fn an_unlock_before_locked_unlocks_nothing() {
            let mut session = Session::new();
            session.app.open(&mut session.display, &mut session.state);
            let selections = session.app.client.selections;
            let (lock, _surfaces) =
                session
                    .locker
                    .ask_lock(&mut session.display, &mut session.state, true);
            assert!(
                !session
                    .locker
                    .client
                    .locked
                    .contains(&wayland_client::Proxy::id(&lock)),
                "the premise: the lock client has not been told `locked`"
            );

            lock.unlock_and_destroy();
            let error = session
                .locker
                .pump_or_error(&mut session.display, &mut session.state);
            assert!(
                session.state.lock.is_some(),
                "the lock client unlocked the session before it was told `locked`"
            );
            let error = error.expect(
                "the lock client was not disconnected: an unlock before `locked` \
                 is `invalid_unlock`",
            );
            assert_eq!(
                error.code,
                ext_session_lock_v1::Error::InvalidUnlock as u32,
                "disconnected, but not for `invalid_unlock`: {error:?}"
            );

            // Thrown off, it has gone the way a crashed one goes: a new lock
            // client takes over.
            session.locker = Side::connect(&mut session.display, &mut session.state);
            session.app.pump(&mut session.display, &mut session.state);
            assert!(
                session.app.client.keyboard_on.is_none(),
                "the lock client was thrown off and the application was given \
                 the keyboard"
            );
            let lock = session.lock();
            session.assert_sealed("an unlock before `locked`, then a new lock", selections);
            session.assert_unlocks(lock);
        }
    }

    /// **#115: what a client says about its own size**, read from the real
    /// protocol and told to the layouts.
    ///
    /// No frames here, for the reason [`tiled_fixture`] gives, so a window's
    /// own terms and its client's are the same numbers; that the frame is
    /// added when there is one is `limits::a_limit_is_read_in_the_windows_own_terms_frame_included`.
    mod client_sizes {
        use super::*;

        /// One monitor, one client, and whatever scripts a test installs.
        struct Desk {
            display: Display<Solium>,
            state: Solium,
            conn: Connection,
            queue: wayland_client::EventQueue<Client>,
            qh: QueueHandle<Client>,
            client: Client,
        }

        impl Desk {
            fn new() -> Self {
                let mut display =
                    Display::<Solium>::new().expect("creating a test wayland display");
                let mut state = Solium::new(display.handle());
                state
                    .decorations
                    .set_style(&mut state.panes, Some("none".to_string()));
                a_screen(&mut state, "sizes-test", (0, 0));
                let (conn, queue, client) = connect(&mut display, &mut state);
                let qh = queue.handle();
                Self {
                    display,
                    state,
                    conn,
                    queue,
                    qh,
                    client,
                }
            }

            /// `body` as the compositor's scripts, against the shipped ones,
            /// for the reason `reflow_on_close::Desk::install` gives.
            fn install(&mut self, body: &str) {
                static NEXT: std::sync::atomic::AtomicUsize =
                    std::sync::atomic::AtomicUsize::new(0);
                let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let directory = std::env::temp_dir().join(format!(
                    "solium-sizes-state-{}-{serial}",
                    std::process::id()
                ));
                let _ = std::fs::create_dir_all(&directory);
                let entry = directory.join("init.lua");
                std::fs::write(
                    &entry,
                    format!(
                        "package.path = {shipped:?} .. \"/?.lua\"\n{body}\n",
                        shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
                    ),
                )
                .expect("writing the entry point");
                self.state.scripts = Some(Scripts::load(&entry).expect("loading the test scripts"));
            }

            fn evaluate(&self, lua: &str) -> String {
                self.state
                    .scripts
                    .as_ref()
                    .map(|scripts| scripts.evaluate(lua))
                    .unwrap_or_default()
            }

            fn pump(&mut self) {
                pump(
                    &mut self.display,
                    &mut self.state,
                    &self.conn,
                    &self.qh,
                    &mut self.queue,
                    &mut self.client,
                );
            }

            fn open(&mut self) -> (Window, xdg_toplevel::XdgToplevel, wl_surface::WlSurface) {
                let (window, toplevel, surface) = open_surface(
                    &mut self.display,
                    &mut self.state,
                    &self.conn,
                    &self.client,
                    &self.qh,
                );
                self.state.sync_panes();
                self.pump();
                (window, toplevel, surface)
            }

            /// This window's row in the window list a script is handed now.
            fn row(&self, window: &Window) -> WindowInfo {
                let id = self.state.window_id(window);
                self.state
                    .snapshot()
                    .windows
                    .into_iter()
                    .find(|row| row.id == id)
                    .expect("the window is listed")
            }

            fn place(&mut self, command: Command) {
                self.state.apply(Outcome {
                    commands: vec![command],
                    ..Outcome::default()
                });
            }
        }

        fn size(w: i32, h: i32) -> Option<Size<i32, Logical>> {
            Some(Size::from((w, h)))
        }

        fn instant() -> AnimationSpec {
            AnimationSpec {
                duration: Duration::ZERO,
                ..AnimationSpec::default()
            }
        }

        /// **An xdg window says how small and how large it can be, and it is
        /// what the window has committed that counts.**
        ///
        /// `set_min_size` and `set_max_size` are double-buffered: the requests
        /// alone change nothing a layout sees, and the commit after them does.
        /// 0 on a side is no limit on that side, and both 0 is no limit at all.
        /// And the application's name comes with it, which is what
        /// `tiling.client_size_ignore` matches.
        #[test]
        fn an_xdg_window_says_how_small_and_how_large_it_can_be() {
            let mut desk = Desk::new();
            let (window, toplevel, surface) = desk.open();
            toplevel.set_app_id("sizes-test".to_owned());
            toplevel.set_min_size(300, 200);
            toplevel.set_max_size(900, 700);
            desk.pump();
            let row = desk.row(&window);
            assert_eq!(
                (row.min, row.max),
                (None, None),
                "a limit asked for and not yet committed was read"
            );

            surface.commit();
            desk.pump();
            let row = desk.row(&window);
            assert_eq!(row.min, size(300, 200));
            assert_eq!(row.max, size(900, 700));
            assert_eq!(row.app_id, "sizes-test");

            toplevel.set_min_size(0, 250);
            toplevel.set_max_size(0, 0);
            surface.commit();
            desk.pump();
            let row = desk.row(&window);
            assert_eq!(row.min, size(0, 250), "one side limited and not the other");
            assert_eq!(row.max, None, "no limit either way is none at all");
        }

        /// **A change of minimum is told to the layouts once**, through the
        /// `layout` event every change to what a layout decides with goes
        /// through -- and a commit that changes nothing tells them nothing.
        #[test]
        fn a_change_of_minimum_is_told_once() {
            let mut desk = Desk::new();
            desk.install("layouts = 0\nsol.on(\"layout\", function() layouts = layouts + 1 end)");
            let (_window, toplevel, surface) = desk.open();
            let told = |desk: &Desk| desk.evaluate("return tostring(layouts)");
            let before: u32 = told(&desk).parse().expect("a count");

            toplevel.set_min_size(400, 300);
            surface.commit();
            desk.pump();
            assert_eq!(
                told(&desk),
                (before + 1).to_string(),
                "the change was not told"
            );

            surface.commit();
            desk.pump();
            surface.commit();
            desk.pump();
            assert_eq!(
                told(&desk),
                (before + 1).to_string(),
                "commits that changed nothing were told as changes"
            );

            toplevel.set_min_size(500, 300);
            surface.commit();
            desk.pump();
            assert_eq!(told(&desk), (before + 2).to_string());
        }

        /// **A window that opens with a minimum hears it once, at `open`.**
        /// The client says 640x480 in the bufferless commit xdg-shell starts a
        /// window with, and draws in the next: the layout's `open` has the
        /// minimum in the window's row, and no `layout` comes ahead of it --
        /// a pass for a window no layout has heard of -- or after it.
        #[test]
        fn a_window_that_opens_with_a_minimum_hears_it_once() {
            let mut desk = Desk::new();
            desk.install(
                "events = {}\n\
                 sol.on(\"open\", function(id)\n\
                     for _, window in ipairs(sol.windows()) do\n\
                         if window.id == id then\n\
                             local min = window.min\n\
                             events[#events + 1] = \"open \" .. (min and (min.w .. \"x\" .. min.h) or \"nil\")\n\
                         end\n\
                     end\n\
                 end)\n\
                 sol.on(\"layout\", function() events[#events + 1] = \"layout\" end)",
            );
            let compositor = desk.client.compositor.clone().expect("wl_compositor bound");
            let wm_base = desk.client.wm_base.clone().expect("xdg_wm_base bound");
            let surface = compositor.create_surface(&desk.qh, ());
            let xdg_surface = wm_base.get_xdg_surface(&surface, &desk.qh, ());
            let toplevel = xdg_surface.get_toplevel(&desk.qh, ());
            toplevel.set_min_size(640, 480);
            surface.commit();
            desk.pump();
            commit_buffer(&desk.client, &desk.qh, &surface, 64, 64);
            desk.pump();
            assert_eq!(
                desk.evaluate("return table.concat(events, \",\")"),
                "open 640x480"
            );
        }

        /// **A limit past any screen is read as the most there is**, from a
        /// real client: `set_min_size(i32::MAX, 0)` is a column of 32767 in
        /// the scrolling layout, not one whose neighbour starts past the end
        /// of `i32`.
        #[test]
        fn an_xdg_limit_past_any_screen_is_read_as_the_most_there_is() {
            let mut desk = Desk::new();
            let (window, toplevel, surface) = desk.open();
            toplevel.set_min_size(i32::MAX, 0);
            toplevel.set_max_size(i32::MAX, i32::MAX);
            surface.commit();
            desk.pump();
            let row = desk.row(&window);
            assert_eq!(row.min, size(super::super::snapshot::MOST, 0));
            assert_eq!(
                row.max,
                size(super::super::snapshot::MOST, super::super::snapshot::MOST)
            );
        }

        /// **A window launched with `sol.spawn` whose minimum does not fit the
        /// tile it was given goes where `overflow` says**, as a window whose
        /// minimum was known at its `open` would.
        ///
        /// Window 1 fills the 1896x1056 inside the gap; window 2 is launched
        /// and given half of it before its application exists. The
        /// application then says 1800x1000 -- in its first, bufferless commit,
        /// and again with its first frame in the same commit, which is the one
        /// that needs the limits noticed before the frame is. Beside window 1,
        /// held at the 160 minimum, the most it can have is 1724 across; below
        /// it, 948 down. So there is no room either way, and the shipped chain
        /// sends it to workspace 2 -- which a launched window used to miss,
        /// being rebalanced in its half and left cramped. Once: a frame after
        /// that moves nothing.
        #[test]
        fn a_launched_window_whose_minimum_does_not_fit_goes_where_overflow_says() {
            for with_its_first_frame in [false, true] {
                let mut desk = Desk::new();
                desk.install(
                    "require(\"modes\")\n\
                     require(\"workspaces\")\n\
                     require(\"tiling\")\n\
                     require(\"scrolling\")",
                );
                assert!(desk.state.trigger("super+t"), "super+t was not handled");
                let _working = desk.open();
                let source = crate::pane::loading_source(None);
                let launched =
                    desk.state
                        .open_loading("app", Some(std::process::id()), source, None);
                let workspace = |desk: &Desk| {
                    desk.evaluate(&format!(
                        "return tostring(require(\"workspaces\").of[{}])",
                        launched.get()
                    ))
                };
                assert_eq!(
                    workspace(&desk),
                    "1",
                    "the premise: launched beside window 1"
                );

                let compositor = desk.client.compositor.clone().expect("wl_compositor bound");
                let wm_base = desk.client.wm_base.clone().expect("xdg_wm_base bound");
                let surface = compositor.create_surface(&desk.qh, ());
                let xdg_surface = wm_base.get_xdg_surface(&surface, &desk.qh, ());
                let toplevel = xdg_surface.get_toplevel(&desk.qh, ());
                toplevel.set_min_size(1800, 1000);
                if with_its_first_frame {
                    commit_buffer(&desk.client, &desk.qh, &surface, 64, 64);
                } else {
                    surface.commit();
                }
                desk.pump();
                let says = if with_its_first_frame {
                    "said with its first frame"
                } else {
                    "said before its first frame"
                };
                assert!(
                    desk.state
                        .panes
                        .get(launched)
                        .and_then(Pane::client)
                        .is_some(),
                    "{says}: the premise: the application arrived in the window it was \
                     launched into"
                );
                assert_eq!(
                    workspace(&desk),
                    "2",
                    "{says}: the launched window stayed in a tile that cannot hold it"
                );
                assert_eq!(
                    desk.state.panes.get(launched).and_then(Pane::placed),
                    Some(at(12, 12, 1896, 1056)),
                    "{says}: it was not given workspace 2 whole"
                );

                commit_buffer(&desk.client, &desk.qh, &surface, 64, 64);
                desk.pump();
                assert_eq!(
                    workspace(&desk),
                    "2",
                    "{says}: a frame after it moved it again"
                );
            }
        }

        /// **A window centred in its tile is dragged from its tile.** A layout
        /// places it at 800x600 inside a 1600x1000 tile, with the tile as
        /// `tile`: the client is told 800x600, the tile is what it is held in,
        /// and the edge a drag starts from is the tile's -- the seam -- and not
        /// the window's, 400 pixels inside it.
        #[test]
        fn a_centred_window_is_dragged_from_its_tile() {
            let mut desk = Desk::new();
            let (window, toplevel, _surface) = desk.open();
            let pane = desk
                .state
                .panes
                .id_of(&window)
                .expect("a client in the space has a pane");
            let id = desk.state.window_id(&window);
            desk.place(Command::Place {
                id,
                rect: Rect {
                    x: 400.0,
                    y: 200.0,
                    w: 800.0,
                    h: 600.0,
                },
                animation: instant(),
                tile: true,
                inside: Some(Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 1600.0,
                    h: 1000.0,
                }),
                cramped: false,
            });
            desk.pump();
            let tile = at(0, 0, 1600, 1000);
            assert_eq!(
                desk.state.panes.get(pane).and_then(Pane::placed),
                Some(tile)
            );
            let laid_out = desk
                .state
                .pane_laid_out(&window)
                .expect("the window has a pane");
            assert_eq!(
                laid_out.0, tile,
                "a drag would start from the window's own edge"
            );
            let pointer: Point<f64, Logical> = (1190.0, 500.0).into();
            let (edge, _) =
                crate::input::resize::dragged_edge(laid_out, ResizeEdge::Right, pointer, pointer);
            assert!((edge - 1600.0).abs() < 1e-9, "{edge}");
            let told = desk
                .client
                .configures
                .iter()
                .rev()
                .find(|(of, ..)| *of == wayland_client::Proxy::id(&toplevel))
                .map(|(_, w, h)| (*w, *h));
            assert_eq!(
                told,
                Some((800, 600)),
                "the client was told the tile's size"
            );
        }

        /// **The layout says a window is cramped, and the window list says so
        /// back** -- as long as the layout goes on saying it. A placement that
        /// does not say it, a placement out of a tile and a layout letting go
        /// each leave the window not cramped.
        #[test]
        fn a_layout_says_a_window_is_cramped_and_the_window_list_says_so_back() {
            let mut desk = Desk::new();
            let (window, _toplevel, _surface) = desk.open();
            let id = desk.state.window_id(&window);
            let rect = Rect {
                x: 0.0,
                y: 0.0,
                w: 600.0,
                h: 400.0,
            };
            let place = |tile: bool, cramped: bool| Command::Place {
                id,
                rect,
                animation: instant(),
                tile,
                inside: None,
                cramped,
            };
            desk.place(place(true, true));
            assert!(
                desk.row(&window).cramped,
                "the layout's word did not come back"
            );
            desk.place(place(true, false));
            assert!(
                !desk.row(&window).cramped,
                "a placement that did not say it"
            );
            desk.place(place(true, true));
            desk.place(Command::Unplace { id });
            assert!(!desk.row(&window).cramped, "let go by its layout");
            desk.place(place(false, true));
            assert!(!desk.row(&window).cramped, "placed out of a tile");
        }

        /// **A window that becomes cramped is told again, so a bar can see
        /// it.** A layout says `cramped` on a `layout` pass, and a bar reading
        /// `sol.windows()` in the same pass cannot see it -- the list is the
        /// snapshot from before -- so the compositor runs the pass once more,
        /// and the bar sees it there. A pass that changes nothing is not told
        /// again; and a layout that changes its mind on every pass is told
        /// once, not for ever.
        #[test]
        fn a_window_that_becomes_cramped_is_told_again_so_a_bar_can_see_it() {
            let mut desk = Desk::new();
            let (window, _toplevel, _surface) = desk.open();
            let id = desk.state.window_id(&window);
            desk.install(&format!(
                "seen = {{}}\n\
                 sol.on(\"layout\", function()\n\
                     sol.place({id}, {{ x = 0, y = 0, w = 600, h = 400, cramped = true }})\n\
                 end)\n\
                 sol.on(\"layout\", function()\n\
                     for _, window in ipairs(sol.windows()) do\n\
                         if window.id == {id} then seen[#seen + 1] = tostring(window.cramped) end\n\
                     end\n\
                 end)"
            ));
            let seen = |desk: &Desk| desk.evaluate("return table.concat(seen, \",\")");
            desk.state.trigger_relayout();
            assert_eq!(
                seen(&desk),
                "false,true",
                "the bar never saw the window cramped"
            );
            desk.state.trigger_relayout();
            assert_eq!(
                seen(&desk),
                "false,true,true",
                "told again with nothing changed"
            );

            desk.install(&format!(
                "passes, flip = 0, true\n\
                 sol.on(\"layout\", function()\n\
                     passes = passes + 1\n\
                     flip = not flip\n\
                     sol.place({id}, {{ x = 0, y = 0, w = 600, h = 400, cramped = flip }})\n\
                 end)"
            ));
            desk.state.trigger_relayout();
            assert_eq!(desk.evaluate("return tostring(passes)"), "2");
        }

        /// A floating drag of `window` by its bottom-right corner, begun at
        /// 400x300 from (100, 100), to the pointer at `to`: (300, 250) asks
        /// for 200x150.
        fn dragged(desk: &Desk, window: &Window, to: (f64, f64)) -> Rectangle<i32, Logical> {
            crate::input::resize::drag_rect(
                &desk.state,
                window,
                at(100, 100, 400, 300),
                ResizeEdge::BottomRight,
                (500.0, 400.0).into(),
                to.into(),
            )
        }

        /// A window whose application calls itself `app_id` and says it
        /// cannot be under 300x200 or over 900x700.
        fn limited_window(
            desk: &mut Desk,
            app_id: &str,
        ) -> (Window, xdg_toplevel::XdgToplevel, wl_surface::WlSurface) {
            let (window, toplevel, surface) = desk.open();
            toplevel.set_app_id(app_id.to_owned());
            toplevel.set_min_size(300, 200);
            toplevel.set_max_size(900, 700);
            surface.commit();
            desk.pump();
            (window, toplevel, surface)
        }

        /// **A floating drag is held to what the client accepts**: the limits
        /// in the window's own terms, and the rectangle the drag makes held
        /// to them -- asked of `drag_rect`, which is what the grab makes its
        /// rectangle with.
        #[test]
        fn a_floating_drag_is_held_to_what_the_client_accepts() {
            let mut desk = Desk::new();
            let (window, _toplevel, _surface) = limited_window(&mut desk, "sizes-test");
            assert_eq!(
                desk.state.outer_limits(&window),
                (Size::from((300, 200)), Size::from((900, 700)))
            );
            assert_eq!(
                dragged(&desk, &window, (300.0, 250.0)),
                at(100, 100, 300, 200)
            );
            assert_eq!(
                dragged(&desk, &window, (2100.0, 1600.0)),
                at(100, 100, 900, 700)
            );
        }

        /// **A floating drag of an application the user does not believe is
        /// not held**: one named in `tiling.client_size_ignore`, or any at all
        /// under `floating.client_limits = "ignore"` -- as `sizes.lua` hands
        /// them over, in `Command::ClientSizes`. An application not on the list
        /// is still held.
        #[test]
        fn a_floating_drag_of_an_application_not_believed_is_not_held() {
            let mut desk = Desk::new();
            let (window, _toplevel, _surface) = limited_window(&mut desk, "liar");
            let small = |desk: &Desk| dragged(desk, &window, (300.0, 250.0));
            for (sizes, says) in [
                (
                    crate::script::ClientSizes {
                        floating: true,
                        ignored: vec!["liar".to_owned()],
                    },
                    "an application in client_size_ignore",
                ),
                (
                    crate::script::ClientSizes {
                        floating: false,
                        ignored: Vec::new(),
                    },
                    "floating.client_limits = \"ignore\"",
                ),
            ] {
                desk.place(Command::ClientSizes(sizes));
                assert_eq!(
                    small(&desk),
                    at(100, 100, 200, 150),
                    "{says}: held all the same"
                );
            }
            desk.place(Command::ClientSizes(crate::script::ClientSizes {
                floating: true,
                ignored: vec!["someone-else".to_owned()],
            }));
            assert_eq!(
                small(&desk),
                at(100, 100, 300, 200),
                "an application not on the list was not held"
            );
            assert!(
                crate::script::ClientSizes {
                    floating: true,
                    ignored: vec![String::new()],
                }
                .believes(""),
                "a window with no name was taken for one on the list"
            );
        }
    }

    /// **#128: the layout hears a close when it is asked for, and hears a
    /// refusal when the window comes back.**
    ///
    /// The compositor's half of the contract, against the shipped layouts
    /// and a real client: when `closing`, `refused` and `close` are sent,
    /// in what order, over which routes, and what the window being closed
    /// looks like meanwhile. What the layouts *do* with each event is
    /// `script.rs`'s `reflow_on_close` module, which can drive them without
    /// a display.
    ///
    /// Scripts are installed straight into `Solium::scripts` rather than
    /// through `start_scripts`, and after the windows have opened. Both are
    /// to keep the arrangement down to `adopt`'s, which is one call with a
    /// known order, and to keep what the scripts asked for at load -- a
    /// wallpaper, in the shipped `init.lua` -- from being built at all: a
    /// Qt scene in a process holding a libwayland connection of its own
    /// aborts the test binary (see the #99 test).
    mod reflow_on_close {
        use super::*;

        /// Writes down every event it hears, in order, as `"name id"` --
        /// with a `*` when the window's own row in that event's snapshot
        /// says `leaving`, and a `?` when the window is not in it at all.
        const RECORDER: &str = r#"
events = {}
local function row(id)
    for _, window in ipairs(sol.windows()) do
        if window.id == id then return window end
    end
end
for _, name in ipairs({ "open", "closing", "refused", "close" }) do
    sol.on(name, function(id)
        local window = row(id)
        local mark = (window == nil and "?") or (window.leaving and "*") or ""
        events[#events + 1] = name .. " " .. id .. mark
    end)
end
"#;

        /// The same, for the two events every layout written before #128
        /// knows: a layout that listens for neither of the new ones.
        const OLD_RECORDER: &str = r#"
events = {}
for _, name in ipairs({ "open", "close" }) do
    sol.on(name, function(id) events[#events + 1] = name .. " " .. id end)
end
"#;

        /// One monitor, one client, and the scripts in `body`.
        struct Desk {
            display: Display<Solium>,
            state: Solium,
            conn: Connection,
            queue: wayland_client::EventQueue<Client>,
            qh: QueueHandle<Client>,
            client: Client,
        }

        impl Desk {
            fn new() -> Self {
                Self::on(|state| {
                    a_screen(state, "reflow-test", (0, 0));
                })
            }

            /// [`Self::new`] with a second monitor to the right of the
            /// first: see [`side_by_side`]. The left one keeps the name
            /// every helper here asks `workspaces` about.
            fn side_by_side() -> Self {
                Self::on(|state| {
                    side_by_side(state, "reflow-test");
                })
            }

            /// The desk, on the monitors `screens` maps.
            fn on(screens: impl FnOnce(&mut Solium)) -> Self {
                let mut display =
                    Display::<Solium>::new().expect("creating a test wayland display");
                let mut state = Solium::new(display.handle());
                state
                    .decorations
                    .set_style(&mut state.panes, Some("none".to_string()));
                screens(&mut state);
                let (conn, queue, client) = connect(&mut display, &mut state);
                let qh = queue.handle();
                Self {
                    display,
                    state,
                    conn,
                    queue,
                    qh,
                    client,
                }
            }

            /// Load an entry point that runs `body` against the shipped
            /// scripts, and hand the compositor those scripts.
            ///
            /// `package.path` is written by the entry for the reason the
            /// layout harness in `script.rs` gives: `Scripts::load` would
            /// otherwise search the developer's own configuration first.
            /// A directory per call, for the reason it gives too.
            fn install(&mut self, body: &str) {
                static NEXT: std::sync::atomic::AtomicUsize =
                    std::sync::atomic::AtomicUsize::new(0);
                let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let directory = std::env::temp_dir().join(format!(
                    "solium-reflow-state-{}-{serial}",
                    std::process::id()
                ));
                let _ = std::fs::create_dir_all(&directory);
                let entry = directory.join("init.lua");
                std::fs::write(
                    &entry,
                    format!(
                        "package.path = {shipped:?} .. \"/?.lua\"\n{body}\n",
                        shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
                    ),
                )
                .expect("writing the entry point");
                self.state.scripts = Some(Scripts::load(&entry).expect("loading the test scripts"));
            }

            /// A shipped layout, switched on over the windows already
            /// open, with `before` run ahead of it and the recorder after.
            fn arrange(&mut self, layout: &str, before: &str) {
                self.install(&format!(
                    "{before}\nrequire(\"modes\")\nrequire({layout:?})\n{RECORDER}"
                ));
                let key = if layout == "tiling" {
                    "super+t"
                } else {
                    "super+s"
                };
                assert!(
                    self.state.trigger(key),
                    "{layout}: the layout key was not handled"
                );
                // And let the arrangement land, so a close starts from a
                // window drawn where it lives rather than part way along
                // the slide into its slot.
                self.state.clock.advance(Duration::from_secs(1));
                let now = self.state.clock.now();
                self.state.settle(now);
            }

            fn open(&mut self) -> (Window, xdg_toplevel::XdgToplevel, crate::pane::PaneId) {
                let (window, toplevel, _surface, _xdg) = open_xdg(
                    &mut self.display,
                    &mut self.state,
                    &self.conn,
                    &self.client,
                    &self.qh,
                );
                self.state.sync_panes();
                self.pump();
                let pane = self
                    .state
                    .panes
                    .id_of(&window)
                    .expect("a client in the space has a pane");
                (window, toplevel, pane)
            }

            fn pump(&mut self) {
                pump(
                    &mut self.display,
                    &mut self.state,
                    &self.conn,
                    &self.qh,
                    &mut self.queue,
                    &mut self.client,
                );
            }

            /// What the recorder heard, comma-separated.
            fn events(&self) -> String {
                self.state
                    .scripts
                    .as_ref()
                    .map(|scripts| scripts.evaluate("return table.concat(events, \",\")"))
                    .unwrap_or_default()
            }

            /// The rectangle the layout last asked for this pane.
            fn placed(&self, pane: crate::pane::PaneId) -> Rectangle<i32, Logical> {
                self.state
                    .panes
                    .get(pane)
                    .and_then(Pane::placed)
                    .expect("the layout has placed this pane")
            }

            /// The pane the keyboard is on.
            fn focused(&self) -> crate::pane::PaneId {
                self.state
                    .focused_window()
                    .and_then(|window| self.state.panes.id_of(&window))
                    .expect("a window has the keyboard")
            }

            /// The close's request goes out: past `CLOSING`, and the
            /// client hears it. Returns the instant it was asked at.
            fn ask(&mut self) -> Duration {
                self.state
                    .clock
                    .advance(present::CLOSING + Duration::from_millis(10));
                let asked = self.state.clock.now();
                self.state.settle_closing(asked);
                self.pump();
                asked
            }

            /// The client says nothing, and the grace period runs out.
            /// Returns the instant the window was given back at.
            fn refuse(&mut self) -> Duration {
                self.state.clock.advance(Duration::from_millis(1100));
                let now = self.state.clock.now();
                self.state.settle_refused(now);
                self.pump();
                now
            }

            /// [`Self::open`], with the client's surface as well: for a
            /// test that has to make the client answer a configure with a
            /// buffer of the size it was given, as a real one does.
            fn open_surface(&mut self) -> Opened {
                let (window, toplevel, surface, xdg) = open_xdg(
                    &mut self.display,
                    &mut self.state,
                    &self.conn,
                    &self.client,
                    &self.qh,
                );
                self.state.sync_panes();
                self.pump();
                let pane = self
                    .state
                    .panes
                    .id_of(&window)
                    .expect("a client in the space has a pane");
                Opened {
                    toplevel,
                    pane,
                    surface,
                    xdg,
                }
            }

            /// The client commits a buffer of exactly the client area the
            /// layout last gave this pane -- the answer to its configure.
            fn answer(&mut self, opened: &Opened) {
                let client = inner(self.placed(opened.pane), self.state.insets_of(opened.pane));
                commit_buffer(
                    &self.client,
                    &self.qh,
                    &opened.surface,
                    client.size.w,
                    client.size.h,
                );
                self.pump();
            }

            /// A modal dialog for `parent`: the toplevel, `set_parent`,
            /// `set_modal` and its first buffer in one flush, so the
            /// compositor reads every one of them in a single dispatch.
            /// Returns the dialog.
            fn open_dialog_for(&mut self, parent: &xdg_toplevel::XdgToplevel) -> Window {
                let compositor = self.client.compositor.clone().expect("wl_compositor bound");
                let wm_base = self.client.wm_base.clone().expect("xdg_wm_base bound");
                let dialogs = self.client.dialogs.clone().expect("xdg_wm_dialog_v1 bound");
                let before: Vec<Window> = self.state.space.elements().cloned().collect();
                let surface = compositor.create_surface(&self.qh, ());
                let xdg = wm_base.get_xdg_surface(&surface, &self.qh, ());
                let toplevel = xdg.get_toplevel(&self.qh, ());
                toplevel.set_parent(Some(parent));
                dialogs.get_xdg_dialog(&toplevel, &self.qh, ()).set_modal();
                commit_buffer(&self.client, &self.qh, &surface, 64, 64);
                self.conn.flush().expect("flushing the dialog's requests");
                self.display
                    .dispatch_clients(&mut self.state)
                    .expect("dispatching the dialog's requests");
                self.state
                    .space
                    .elements()
                    .find(|window| !before.contains(window))
                    .cloned()
                    .expect("new_toplevel mapped the dialog")
            }
        }

        /// A window [`Desk::open_surface`] opened.
        struct Opened {
            toplevel: xdg_toplevel::XdgToplevel,
            pane: crate::pane::PaneId,
            surface: wl_surface::WlSurface,
            /// What a menu names as its parent.
            xdg: xdg_surface::XdgSurface,
        }

        /// A window a test closes under `layout`, and the windows beside
        /// it: `(closed, others)`.
        ///
        /// Chosen so that a sweep places some other window *after* the one
        /// being closed, which is the one `Space::map_element` leaves above
        /// it: both shipped layouts place left to right. In tiling that is
        /// the left of two windows. In scrolling it is the middle of three,
        /// because closing the left of two moves nothing: the strip keeps
        /// the column that is left where it was. Which of the others moves
        /// into the space is the layout's business, so the tests find out
        /// rather than say.
        fn closing_scene(desk: &mut Desk, layout: &str) -> (Opened, Vec<Opened>) {
            let count = if layout == "tiling" { 2 } else { 3 };
            let mut opened: Vec<Opened> = (0..count).map(|_| desk.open_surface()).collect();
            desk.arrange(layout, "");
            opened.sort_by_key(|each| desk.placed(each.pane).loc.x);
            let closed = opened.remove(count - 2);
            assert!(
                opened
                    .iter()
                    .all(|other| desk.placed(other.pane).loc.x != desk.placed(closed.pane).loc.x),
                "{layout}: the premise is windows side by side"
            );
            (closed, opened)
        }

        /// Close `closed`, and say which of `others` the layout moved.
        fn close_and_see_who_moved<'a>(
            desk: &mut Desk,
            layout: &str,
            closed: &Opened,
            others: &'a [Opened],
        ) -> &'a Opened {
            let before: Vec<Rectangle<i32, Logical>> =
                others.iter().map(|other| desk.placed(other.pane)).collect();
            desk.state.close_pane(closed.pane);
            others
                .iter()
                .zip(before)
                .find(|(other, was)| desk.placed(other.pane) != *was)
                .map(|(other, _)| other)
                .unwrap_or_else(|| {
                    panic!("{layout}: the premise is a neighbour moving into the space")
                })
        }

        /// How far a pane can have moved in `seconds` of real time without
        /// jumping, in pixels, and how far its opacity can have changed.
        ///
        /// The clock is real time, so the sweeps of one dispatch are
        /// microseconds apart and each starts from where the one before it
        /// had got to by then. That is motion rather than a jump, and it is
        /// bounded: no placement these tests drive is faster than 180 ms
        /// (tiling's `snap`), the return fade is 150 ms, and `OutCubic`'s
        /// steepest slope is 3, at the start -- across at most a screen's
        /// width and one unit of opacity.
        fn drift(seconds: f64) -> (f64, f32) {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "an opacity bound, far inside f32"
            )]
            let opacity = (3.0 / 0.15 * seconds) as f32;
            (1.0 + 1920.0 * 3.0 / 0.18 * seconds, opacity)
        }

        /// Whether `top` is drawn over `under`: earlier in `on_screen`,
        /// which is topmost first and the order `render` draws in, after
        /// `sync_panes` has brought it in line with the space the way every
        /// frame does.
        fn above(state: &mut Solium, top: crate::pane::PaneId, under: crate::pane::PaneId) -> bool {
            state.sync_panes();
            let order: Vec<crate::pane::PaneId> = state
                .on_screen()
                .into_iter()
                .map(|(pane, _)| pane)
                .collect();
            let at = |pane| order.iter().position(|each| *each == pane);
            at(top)
                .zip(at(under))
                .is_some_and(|(top, under)| top < under)
        }

        /// What `sol.window_at` answers at `point`, asked from Lua of the
        /// compositor as it is this instant -- the snapshot a handler
        /// dispatched now would be handed -- as text: a window's id, or
        /// `nil`.
        fn window_at_from_lua(desk: &Desk, point: Point<f64, Logical>) -> String {
            let snapshot = desk.state.snapshot();
            desk.state
                .scripts
                .as_ref()
                .map(|scripts| {
                    scripts.evaluate_in(
                        snapshot,
                        &format!(
                            "return tostring(sol.window_at({:?}, {:?}))",
                            point.x, point.y
                        ),
                    )
                })
                .unwrap_or_default()
        }

        const LAYOUTS: [&str; 2] = ["tiling", "scrolling"];

        /// Two windows side by side under `layout`, and which is which:
        /// `(the one the keyboard is on, the other)`.
        fn two(
            desk: &mut Desk,
            layout: &str,
            before: &str,
        ) -> (crate::pane::PaneId, crate::pane::PaneId) {
            let (_, _, first) = desk.open();
            let (_, _, second) = desk.open();
            desk.arrange(layout, before);
            let focused = desk.focused();
            let other = if focused == first { second } else { first };
            assert_ne!(
                desk.placed(focused).loc.x,
                desk.placed(other).loc.x,
                "{layout}: the premise is two windows side by side"
            );
            (focused, other)
        }

        /// **Close a window and the layout closes up around it at once,
        /// while the client is still there and has not even been asked.**
        ///
        /// The window closed is the one the keyboard is on, and the
        /// keyboard stays on it: it leaves when the animation lands
        /// (`hand_off_keyboard`), and a layout that moved it now -- the
        /// scrolling layout's `settle` would -- takes it off a window that
        /// is still on screen.
        ///
        /// The window closed fades where it stood. Nothing places it once
        /// it is out of the arrangement, and #127's pinning keeps it from
        /// following anything that does.
        #[test]
        fn a_closing_window_hands_its_space_over_before_its_client_is_gone() {
            for layout in LAYOUTS {
                let mut desk = Desk::new();
                let (dying, survivor) = two(&mut desk, layout, "");
                let was = desk.placed(survivor);
                let stood = desk.placed(dying);
                let outer = desk
                    .state
                    .pane_outer_of(dying)
                    .expect("a mapped pane has a rectangle");

                let pressed = desk.state.clock.now();
                desk.state.close_pane(dying);
                desk.pump();

                assert!(
                    desk.client.closes.is_empty() && desk.state.panes.get(dying).is_some(),
                    "{layout}: the premise -- the client has not even been asked, so \
                     everything below happened before it could have gone"
                );
                assert_eq!(
                    desk.events(),
                    format!("closing {}*", dying.get()),
                    "{layout}"
                );
                let now = desk.placed(survivor);
                assert_ne!(
                    now, was,
                    "{layout}: the survivor is where it was, so the closed window's space \
                     is still reserved for it"
                );
                match layout {
                    "tiling" => assert!(
                        now.size.w > was.size.w,
                        "tiling: the neighbour did not grow into the space: {was:?} -> {now:?}"
                    ),
                    _ => assert_eq!(
                        now.loc.x, stood.loc.x,
                        "scrolling: the strip did not close the gap: {was:?} -> {now:?}"
                    ),
                }
                assert_eq!(
                    desk.focused(),
                    dying,
                    "{layout}: the keyboard left the closing window before its animation \
                     landed"
                );
                let fading = drawn_now(&desk.state, dying, pressed + Duration::from_millis(95));
                assert!(
                    fading.opacity < 0.9
                        && (fading.rect.loc.x - f64::from(outer.loc.x)).abs() < 100.0,
                    "{layout}: the closing window is not fading where it stood: {fading:?}, \
                     and it stood at {outer:?}"
                );
            }
        }

        /// **Refuse the close, and the window comes back beside its old
        /// neighbour.**
        ///
        /// Two windows, so the neighbour was a single window and tiling
        /// puts everything back exactly; scrolling is asserted as order and
        /// adjacency, because putting a window back focuses its column and
        /// the view may follow.
        ///
        /// And `refused` is sent once, and `open` not at all: the window
        /// never went, and an `open` would animate it in a second time and
        /// run the scrolling layout's `settle`.
        #[test]
        fn a_refused_close_comes_back_beside_its_old_neighbour() {
            for layout in LAYOUTS {
                let mut desk = Desk::new();
                let (dying, survivor) = two(&mut desk, layout, "");
                let before = (desk.placed(dying), desk.placed(survivor));

                desk.state.close_pane(dying);
                desk.ask();
                desk.refuse();
                // And more frames, well past every deadline, which must not
                // produce a second one.
                desk.state.clock.advance(Duration::from_secs(3));
                let later = desk.state.clock.now();
                desk.state.settle(later);
                desk.pump();

                let id = dying.get();
                assert_eq!(
                    desk.events(),
                    format!("closing {id}*,refused {id}"),
                    "{layout}: the refusal was not told once, and once only, with the window \
                     no longer leaving"
                );
                let after = (desk.placed(dying), desk.placed(survivor));
                match layout {
                    "tiling" => assert_eq!(
                        after, before,
                        "tiling: the window did not come back where it was"
                    ),
                    _ => {
                        let (back, other) = after;
                        let (left, right) = if back.loc.x < other.loc.x {
                            (back, other)
                        } else {
                            (other, back)
                        };
                        assert_eq!(
                            before.0.loc.x < before.1.loc.x,
                            back.loc.x < other.loc.x,
                            "scrolling: the window came back on the other side of its \
                             neighbour"
                        );
                        assert_eq!(
                            left.loc.x + left.size.w + 12,
                            right.loc.x,
                            "scrolling: the two are not neighbours: {left:?}, {right:?}"
                        );
                    }
                }
            }
        }

        /// **A refused window fades back in from where it vanished**, and
        /// does not appear at once over the neighbour that grew into it.
        ///
        /// `give_back` starts the return from the held, transparent end of
        /// the leaving animation; the layout putting the window back then
        /// replaces that with `move_pane`'s slide, which begins from what is
        /// on screen -- that same transparent frame -- rather than from full
        /// opacity at the rectangle the window had.
        #[test]
        fn a_refused_window_fades_back_in_from_where_it_vanished() {
            for layout in LAYOUTS {
                let mut desk = Desk::new();
                let (dying, _) = two(&mut desk, layout, "");
                desk.state.close_pane(dying);
                desk.ask();
                let back = desk.refuse();
                assert!(
                    desk.events().ends_with(&format!("refused {}", dying.get())),
                    "{layout}: the premise is a refusal the layout heard"
                );

                // The slide starts from the fade as it was when the layout
                // placed the window, a moment of real time after `back`.
                let (_, fading) = drift((desk.state.clock.now() - back).as_secs_f64());
                let first = drawn_now(&desk.state, dying, back);
                assert!(
                    first.opacity < 0.05 + fading,
                    "{layout}: the refused window was drawn at opacity {} on the frame it \
                     came back, rather than fading in from nothing",
                    first.opacity
                );
                let landed = drawn_now(&desk.state, dying, back + Duration::from_millis(400));
                let slot = desk.placed(dying);
                assert!(
                    (landed.opacity - 1.0).abs() < f32::EPSILON
                        && (landed.rect.loc.x - f64::from(slot.loc.x)).abs() < 1.0,
                    "{layout}: the return did not land where the layout put the window: \
                     {landed:?}, placed at {slot:?}"
                );
            }
        }

        /// **A client that goes at its grace deadline is not refused after
        /// it closed.**
        ///
        /// The order a frame runs in is the whole of it: the Wayland
        /// dispatch -- where the client's destroy arrives and `close` is
        /// sent -- then `settle`, where the grace deadline is read, and
        /// only then `sync_panes`, which retires the pane. A client
        /// destroying its toplevel on the frame its grace ran out was
        /// still `asked_at` in that `settle`, was given back, and the
        /// layout was told `refused` after `close` -- and put a window that
        /// no longer exists back into its tree for good.
        #[test]
        fn a_client_that_goes_at_its_grace_deadline_is_not_refused_after_it_closed() {
            let mut desk = Desk::new();
            let (_, _, kept) = desk.open();
            let (_, toplevel, going) = desk.open();
            desk.arrange("tiling", "");
            let whole = desk.placed(kept);

            desk.state.close_pane(going);
            let asked = desk.ask();
            // The frame on which the grace runs out: the client goes in the
            // dispatch, and `settle` runs after it at the deadline.
            toplevel.destroy();
            desk.pump();
            desk.state.settle(asked + Duration::from_millis(1000));
            desk.state.space.refresh();
            desk.state.sync_panes();
            desk.pump();

            let id = going.get();
            assert_eq!(
                desk.events(),
                format!("closing {id}*,close {id}*"),
                "the layout was told something after the window was gone"
            );
            assert!(
                desk.state.panes.get(going).is_none(),
                "the premise: the pane is retired"
            );
            // And the arrangement holds no leaf for it: laid out again, the
            // window that stayed has the screen.
            desk.state.trigger_relayout();
            assert!(
                desk.placed(kept).size.w > whole.size.w,
                "the tree still divides the screen with a window that is gone: {:?}",
                desk.placed(kept)
            );
        }

        /// **A layout placing a window in `close` does not show it again.**
        ///
        /// A stateless layout places every row it is handed, and `close`'s
        /// snapshot still lists the window that went. The close timers are
        /// disarmed after that dispatch rather than before it, so the pane
        /// is still leaving while the handler runs and `move_pane` leaves
        /// the invisible transform alone; the other order made it an
        /// ordinary pane for one frame and put it back at full opacity.
        #[test]
        fn a_layout_placing_a_closed_window_does_not_show_it_again() {
            let mut desk = Desk::new();
            let (_, toplevel, pane) = desk.open();
            desk.install(
                r#"
sol.on("close", function()
    for _, window in ipairs(sol.windows()) do
        sol.place(window.id, { x = window.x, y = window.y, w = window.w, h = window.h })
    end
end)
"#,
            );

            desk.state.close_pane(pane);
            let asked = desk.ask();
            assert!(
                drawn_now(&desk.state, pane, asked).opacity.abs() < f32::EPSILON,
                "the premise: the window is held invisible while the client decides"
            );
            toplevel.destroy();
            desk.pump();
            let now = desk.state.clock.now();
            assert!(
                desk.state.panes.get(pane).is_some(),
                "the premise: the pane is still here until `sync_panes`"
            );
            assert!(
                drawn_now(&desk.state, pane, now).opacity.abs() < f32::EPSILON,
                "a layout placing the window in `close` drew it again at opacity {}",
                drawn_now(&desk.state, pane, now).opacity
            );
        }

        /// **Every way to close a window tells the layout at once.**
        ///
        /// Script events are dispatched by taking the scripts out of
        /// `Solium::scripts` for the length of the call, so a close that
        /// reached `close_pane` *during* a dispatch would find nothing there
        /// and `closing` would be lost without a word. Each route is driven
        /// the way it arrives: the frame's close button through
        /// `frame_action`, which the pointer calls; `sol.close` from a
        /// binding and from an event handler, both of which queue a
        /// command that `apply` runs after the dispatch has put the
        /// scripts back; the same from inside a `closing` handler, which is
        /// a dispatch inside `apply`; and `super+q`, the shipped binding in
        /// `init.lua`.
        #[test]
        fn every_close_route_tells_the_layout_the_close_has_begun() {
            let mut desk = Desk::new();
            let (_, _, button) = desk.open();
            let (_, _, bound) = desk.open();
            let (_, _, handled) = desk.open();
            let (_, _, first) = desk.open();
            let (_, _, second) = desk.open();
            desk.install(&format!(
                "{RECORDER}\n\
                 sol.bind(\"super+x\", function() sol.close({bound}) end)\n\
                 local armed = true\n\
                 sol.on(\"layout\", function()\n\
                     if armed then armed = false; sol.close({handled}) end\n\
                 end)\n\
                 sol.on(\"closing\", function(id)\n\
                     if id == {first} then sol.close({second}) end\n\
                 end)\n",
                bound = bound.get(),
                handled = handled.get(),
                first = first.get(),
                second = second.get(),
            ));

            desk.state.frame_action(button, Action::Close);
            assert!(desk.state.trigger("super+x"), "the binding was not handled");
            desk.state.trigger_relayout();
            desk.state.close_pane(first);

            assert_eq!(
                desk.events(),
                format!(
                    "closing {}*,closing {}*,closing {}*,closing {}*,closing {}*",
                    button.get(),
                    bound.get(),
                    handled.get(),
                    first.get(),
                    second.get()
                ),
                "a close route did not tell the layout it had begun"
            );

            // `super+q`, as it ships: the whole of `init.lua`, whose
            // binding closes the window the keyboard is on.
            let mut desk = Desk::new();
            let (_, _, focused) = desk.open();
            assert_eq!(
                desk.focused(),
                focused,
                "the premise: a window has the keyboard"
            );
            desk.install(&format!(
                "dofile({init:?})\n{RECORDER}",
                init = concat!(env!("CARGO_MANIFEST_DIR"), "/lua/init.lua"),
            ));
            assert!(desk.state.trigger("super+q"), "super+q was not handled");
            assert_eq!(
                desk.events(),
                format!("closing {}*", focused.get()),
                "super+q did not tell the layout the close had begun"
            );
        }

        /// **A layout that listens for neither new event hears exactly what
        /// it always heard.** `close` when the window is gone and not a
        /// moment before -- whether it was asked to close or went by
        /// itself -- and nothing at all for a close that was refused: not a
        /// `close` for the moment it was asked, and not an `open` for the
        /// moment it came back.
        ///
        /// This is not a change and did not fail before it; it is here so
        /// that one cannot be made quietly. Against a `give_back` that
        /// told scripts `opened` instead of `refused`, it fails at "a
        /// refused close".
        #[test]
        fn a_layout_that_knows_neither_new_event_hears_what_it_always_did() {
            let mut desk = Desk::new();
            let (_, toplevel, asked) = desk.open();
            let (_, itself_toplevel, itself) = desk.open();
            desk.install(OLD_RECORDER);

            desk.state.close_pane(asked);
            desk.ask();
            assert_eq!(
                desk.events(),
                "",
                "the close was announced before the window went"
            );
            desk.refuse();
            assert_eq!(
                desk.events(),
                "",
                "a refused close told the layout something"
            );

            desk.state.close_pane(asked);
            desk.ask();
            toplevel.destroy();
            desk.pump();
            itself_toplevel.destroy();
            desk.pump();
            assert_eq!(
                desk.events(),
                format!("close {},close {}", asked.get(), itself.get()),
                "the layout was not told each window went, once"
            );
        }

        /// **The window list says which windows are on their way out**, in
        /// every event about one: `closing`'s, `close`'s -- including for a
        /// window that closed itself and was never `closing` -- and not
        /// `refused`'s, where the window is staying.
        #[test]
        fn the_window_list_says_which_windows_are_leaving() {
            let mut desk = Desk::new();
            let (_, _, refused) = desk.open();
            let (_, itself_toplevel, itself) = desk.open();
            desk.install(RECORDER);

            desk.state.close_pane(refused);
            desk.ask();
            desk.refuse();
            itself_toplevel.destroy();
            desk.pump();

            assert_eq!(
                desk.events(),
                format!(
                    "closing {r}*,refused {r},close {i}*",
                    r = refused.get(),
                    i = itself.get()
                )
            );
        }

        /// **A refusal is told once, on the frame the window comes back**,
        /// by whichever route brings it back -- and not on a frame whose
        /// give-back was declined, when the window is still invisible and a
        /// layout putting it back would be putting back nothing.
        ///
        /// The dialog route here; the grace deadline is
        /// `a_refused_close_comes_back_beside_its_old_neighbour`. The
        /// declined one is driven with `present::jam_slot`, which never
        /// lets go, so what is asserted is the half this can reach: nothing
        /// is told while the give-back is owed.
        #[test]
        fn a_refusal_tells_the_layout_once_on_the_frame_the_window_comes_back() {
            let mut desk = Desk::new();
            let (_, parent_top, parent) = desk.open();
            let (_, jammed_top, jammed) = desk.open();
            desk.install(RECORDER);

            desk.state.close_pane(parent);
            let (_, dialog_top, _) = desk.open();
            dialog_top.set_parent(Some(&parent_top));
            desk.pump();
            desk.ask();
            desk.refuse();
            let id = parent.get();
            let heard = desk.events();
            assert!(
                heard.starts_with(&format!("closing {id}*,"))
                    && heard.ends_with(&format!("refused {id}")),
                "the dialog's answer was not told as a refusal: {heard}"
            );
            assert_eq!(
                heard.matches("refused").count(),
                1,
                "a refusal was told more than once: {heard}"
            );

            desk.state.close_pane(jammed);
            if let Some(busy) = desk.state.panes.get(jammed) {
                present::jam_slot(busy);
            }
            let (_, answer_top, _) = desk.open();
            answer_top.set_parent(Some(&jammed_top));
            desk.pump();
            desk.ask();
            desk.state.clock.advance(Duration::from_millis(100));
            let later = desk.state.clock.now();
            desk.state.settle_closing(later);
            assert!(
                desk.state.panes.get(jammed).is_some_and(Pane::leaving),
                "the premise: the give-back is still owed"
            );
            assert!(
                !desk.events().contains(&format!("refused {}", jammed.get())),
                "a refusal was told on a frame its give-back was declined: {}",
                desk.events()
            );
        }

        /// **A dialog that answers a close keeps the keyboard, in the
        /// scrolling layout too.** Putting the window back focuses its
        /// column, and `settle` would hand the keyboard to that column --
        /// off the "save your changes?" the user is being asked. `refused`
        /// uses `apply`, and `open` is not sent, because its handler
        /// settles.
        #[test]
        fn a_dialog_that_refuses_a_close_keeps_the_keyboard() {
            let mut desk = Desk::new();
            let (_, parent_top, parent) = desk.open();
            let (_, _, _other) = desk.open();
            desk.arrange("scrolling", "");

            desk.state.close_pane(parent);
            let (dialog, dialog_top, _) = desk.open();
            assert!(
                desk.state.is_focused(&dialog),
                "the premise: the dialog has the keyboard"
            );
            dialog_top.set_parent(Some(&parent_top));
            desk.pump();
            assert!(
                desk.events()
                    .ends_with(&format!("refused {}", parent.get())),
                "the premise: the dialog's answer was a refusal the layout heard: {}",
                desk.events()
            );
            assert!(
                desk.state.is_focused(&dialog),
                "the layout took the keyboard off the dialog when it put its window back"
            );
        }

        /// **A window refused by a dialog fades back in, and its neighbour
        /// moves once, from where it was drawn.**
        ///
        /// The dialog route is the common refusal -- "save your changes?"
        /// -- and a client that sends the toplevel, `set_parent` and
        /// `set_modal` in one flush (GTK, by the account of the review that
        /// found this) makes it several layout sweeps in one dispatch:
        /// `refused` runs one from inside `give_back`, `parent_changed`
        /// another, and `modal_changed` a third. Each sweep after the
        /// first used to start every window it placed from `Frame::real` of
        /// the rectangle the previous sweep had just written to the space:
        /// the refused window at full opacity, and -- in tiling, where the
        /// neighbour changes size as well as place -- the neighbour at its
        /// new position and its old, committed size, a 954px jump before
        /// anything had been drawn. So both are read at the instant the
        /// dialog arrived, which is before any of those animations has
        /// moved more than real time allows, and must be what was on screen
        /// then.
        ///
        /// The neighbour's client answers its configure first, as a real
        /// one does: the space holding the size the layout gave it is what
        /// makes the jump a jump, rather than an artefact of a fixture that
        /// never resizes.
        #[test]
        fn a_refusal_by_dialog_fades_the_window_back_and_moves_its_neighbour_once() {
            for layout in LAYOUTS {
                let mut desk = Desk::new();
                let (dying, others) = closing_scene(&mut desk, layout);
                let survivor = close_and_see_who_moved(&mut desk, layout, &dying, &others);
                desk.ask();
                desk.answer(survivor);
                // Past the neighbour's slide into the space, and well inside
                // the grace period.
                desk.state.clock.advance(Duration::from_millis(300));
                let settled = desk.state.clock.now();
                desk.state.settle(settled);

                let arrived = desk.state.clock.now();
                let neighbour = drawn_now(&desk.state, survivor.pane, arrived);
                let dialog = desk.open_dialog_for(&dying.toplevel);
                let (moving, fading) = drift((desk.state.clock.now() - arrived).as_secs_f64());
                assert!(
                    desk.state.is_modal(&dialog)
                        && desk
                            .events()
                            .contains(&format!("refused {}", dying.pane.get())),
                    "{layout}: the premise is a modal dialog whose answer was a refusal the \
                     layout heard: {}",
                    desk.events()
                );

                let back = drawn_now(&desk.state, dying.pane, arrived);
                assert!(
                    back.opacity < 0.05 + fading,
                    "{layout}: the refused window was drawn at opacity {} on the frame its \
                     dialog arrived, rather than fading in from nothing",
                    back.opacity
                );
                let moved = drawn_now(&desk.state, survivor.pane, arrived);
                assert!(
                    (moved.rect.loc.x - neighbour.rect.loc.x).abs() < moving
                        && (moved.rect.size.w - neighbour.rect.size.w).abs() < moving,
                    "{layout}: the neighbour jumped on the frame the dialog arrived: it was \
                     drawn at {:?} and is now drawn at {:?}",
                    neighbour.rect,
                    moved.rect
                );

                // And both still land where the layout put them.
                let later = arrived + Duration::from_secs(1);
                let landed = drawn_now(&desk.state, dying.pane, later);
                let slot = desk.placed(dying.pane);
                assert!(
                    (landed.opacity - 1.0).abs() < f32::EPSILON
                        && (landed.rect.loc.x - f64::from(slot.loc.x)).abs() < 1.0,
                    "{layout}: the refused window did not land in its slot: {landed:?}, \
                     placed at {slot:?}"
                );
                let slot = desk.placed(survivor.pane);
                let landed = drawn_now(&desk.state, survivor.pane, later);
                assert!(
                    (landed.rect.loc.x - f64::from(slot.loc.x)).abs() < 1.0,
                    "{layout}: the neighbour did not land in its slot: {landed:?}, placed \
                     at {slot:?}"
                );
            }
        }

        /// **A window being closed fades in front of the neighbour that
        /// moves into its space.**
        ///
        /// The sweep `closing` runs places every survivor and none of the
        /// dying window, and `Space::map_element` puts each window it places
        /// on top -- so the neighbour growing into the space (tiling) or
        /// sliding into it (scrolling) was stacked over the window fading
        /// there, and covered most of the fade. And in scrolling the
        /// window the keyboard is on can be below its neighbour to begin
        /// with, because the strip's own sweeps stack left to right. So a
        /// window is raised as its close begins, and a later sweep inside
        /// the fade -- another window opening, a client's `set_parent` --
        /// does not bury it either.
        #[test]
        fn a_closing_window_fades_in_front_of_the_neighbour_moving_into_its_space() {
            for layout in LAYOUTS {
                let mut desk = Desk::new();
                let (dying, others) = closing_scene(&mut desk, layout);
                let moved = close_and_see_who_moved(&mut desk, layout, &dying, &others);
                assert!(
                    above(&mut desk.state, dying.pane, moved.pane),
                    "{layout}: the neighbour moving into the closed window's space is drawn \
                     over the window fading there"
                );
                for other in &others {
                    assert!(
                        above(&mut desk.state, dying.pane, other.pane),
                        "{layout}: a window the close did not move was stacked over the \
                         window fading"
                    );
                }

                desk.state.clock.advance(Duration::from_millis(80));
                desk.state.trigger_relayout();
                for other in &others {
                    assert!(
                        above(&mut desk.state, dying.pane, other.pane),
                        "{layout}: a layout sweep during the fade stacked a neighbour over \
                         the window fading"
                    );
                }
            }
        }

        /// **A refused window fades back in front of the neighbour giving
        /// its space back.**
        ///
        /// The mirror of the close: the sweep `refused` runs places every
        /// window, left to right, so a neighbour placed after the window
        /// coming back -- giving back the space it had moved into -- was
        /// stacked over the window fading into it. And the dialog route
        /// sweeps again inside the return, so a relayout during it is
        /// driven too.
        #[test]
        fn a_refused_window_fades_back_in_front_of_the_neighbour_making_room() {
            for layout in LAYOUTS {
                let mut desk = Desk::new();
                let (dying, others) = closing_scene(&mut desk, layout);
                close_and_see_who_moved(&mut desk, layout, &dying, &others);
                desk.ask();
                desk.refuse();
                assert!(
                    desk.events()
                        .ends_with(&format!("refused {}", dying.pane.get())),
                    "{layout}: the premise is a refusal the layout heard"
                );
                for other in &others {
                    assert!(
                        above(&mut desk.state, dying.pane, other.pane),
                        "{layout}: a neighbour giving the space back is drawn over the \
                         window fading back into it"
                    );
                }

                desk.state.clock.advance(Duration::from_millis(50));
                desk.state.trigger_relayout();
                for other in &others {
                    assert!(
                        above(&mut desk.state, dying.pane, other.pane),
                        "{layout}: a layout sweep during the return stacked a neighbour \
                         over the window fading back in"
                    );
                }
            }
        }

        /// **#135: `sol.window_at` over a window fading out answers the
        /// neighbour that grew into its place.** The close has landed and
        /// the request has gone out, so the window is held at opacity zero,
        /// stacked in front of the neighbour the layout moved into its
        /// space -- which is what the user sees there. The Rust hit test
        /// has seen past it since #127 (`Frame::covers`); `sol.window_at`
        /// matched the rectangle and found the window nobody can see, so a
        /// scrolling drop there was dropped onto a window in no strip and
        /// went nowhere: here the third window, dropped there, joins the
        /// neighbour's column.
        #[test]
        fn sol_window_at_over_a_window_fading_out_answers_the_neighbour_in_its_place() {
            for layout in LAYOUTS {
                let mut desk = Desk::new();
                let (dying, others) = closing_scene(&mut desk, layout);
                // Every client draws the tile it was given, as a real one
                // does, so each is drawn over the whole of it.
                for opened in others.iter().chain([&dying]) {
                    desk.answer(opened);
                }
                // The keyboard elsewhere, so the request going out hands
                // nothing on: a window given the keyboard is raised, and
                // that would put a neighbour back over the fade.
                let elsewhere = desk
                    .state
                    .panes
                    .get(others[0].pane)
                    .and_then(Pane::client)
                    .cloned()
                    .expect("the pane has its client");
                desk.state
                    .focus_window(&elsewhere, SERIAL_COUNTER.next_serial());
                desk.state.clock.advance(Duration::from_secs(1));
                let now = desk.state.clock.now();
                desk.state.settle(now);
                let tile = desk.placed(dying.pane);
                let point = Point::<f64, Logical>::from((
                    f64::from(tile.loc.x + tile.size.w / 2),
                    f64::from(tile.loc.y + tile.size.h / 2),
                ));
                let moved = close_and_see_who_moved(&mut desk, layout, &dying, &others);
                desk.answer(moved);
                let moved = moved.pane;
                desk.ask();
                desk.state.clock.advance(Duration::from_millis(300));
                let now = desk.state.clock.now();
                desk.state.settle(now);
                desk.state.sync_panes();
                desk.pump();

                let now = desk.state.clock.now();
                let fading = drawn_now(&desk.state, dying.pane, now);
                assert!(
                    !fading.shows()
                        && fading.rect.contains(point)
                        && drawn_now(&desk.state, moved, now).covers(point)
                        && above(&mut desk.state, dying.pane, moved),
                    "{layout}: the premise: the window being closed is drawn at nothing over \
                     {point:?}, in front of the neighbour now drawn there"
                );
                assert_eq!(
                    window_at_from_lua(&desk, point),
                    moved.get().to_string(),
                    "{layout}: `sol.window_at` over the neighbour that grew into a closing \
                     window's place answered the window drawn at nothing, {}",
                    dying.pane.get()
                );

                if layout == "scrolling"
                    && let Some(third) = others.iter().find(|other| other.pane != moved)
                {
                    let dropped = desk
                        .state
                        .panes
                        .get(third.pane)
                        .and_then(Pane::client)
                        .cloned()
                        .expect("the pane has its client");
                    desk.state.trigger_drop(&dropped, point.x, point.y);
                    desk.state.clock.advance(Duration::from_secs(1));
                    let now = desk.state.clock.now();
                    desk.state.settle(now);
                    assert_eq!(
                        desk.placed(third.pane).loc.x,
                        desk.placed(moved).loc.x,
                        "a window dropped over the neighbour in a closing window's place did \
                         not join its column"
                    );
                }
            }
        }

        /// **Pinned as it stands: `sol.window_at` sees through the panes
        /// the snapshot leaves out, where the Rust walk stops at them.**
        /// One rule, [`owns`], asked over two lists: `window_under_at`
        /// walks every pane, and `sol.window_at` the snapshot, which
        /// leaves out a pane that is not managed (a menu, a tooltip), a
        /// pane whose application has not arrived when
        /// `loading.reserves_a_slot` is off, and a pane scripts have been
        /// told has gone. Over each, the Rust walk answers it -- or, for
        /// a pane with no client, nothing -- and a script is answered with
        /// the window behind. Older than #134, and a change of its own;
        /// when one of the lists changes, this is the test that has to.
        #[test]
        fn sol_window_at_sees_through_the_panes_the_snapshot_leaves_out() {
            /// A floating window at `loc`, drawing `size`.
            fn floating(desk: &mut Desk, loc: (i32, i32), size: (i32, i32)) -> Opened {
                let opened = desk.open_surface();
                commit_buffer(&desk.client, &desk.qh, &opened.surface, size.0, size.1);
                desk.pump();
                let window = desk
                    .state
                    .panes
                    .get(opened.pane)
                    .and_then(Pane::client)
                    .cloned()
                    .expect("the pane has its client");
                desk.state.map_stacked(window, loc, false);
                desk.state.sync_panes();
                opened
            }

            let point = Point::<f64, Logical>::from((960.0, 540.0));
            for kind in ["unmanaged", "a slot not reserved", "gone"] {
                let mut desk = Desk::new();
                desk.install("");
                let under = floating(&mut desk, (100, 100), (1600, 900));
                let over = if kind == "a slot not reserved" {
                    desk.state.loading.reserves_a_slot = false;
                    let source = crate::pane::loading_source(None);
                    desk.state.open_loading("app", None, source, None)
                } else {
                    floating(&mut desk, (200, 200), (1200, 700)).pane
                };
                desk.state.clock.advance(Duration::from_secs(1));
                let now = desk.state.clock.now();
                desk.state.settle(now);
                if let Some(pane) = desk.state.panes.get_mut(over) {
                    match kind {
                        "unmanaged" => pane.unmanage(),
                        "gone" => pane.went(),
                        _ => {}
                    }
                }
                let client = desk
                    .state
                    .panes
                    .get(over)
                    .and_then(Pane::client)
                    .map(|window| desk.state.window_id(window));

                assert_eq!(
                    desk.state
                        .window_under(point)
                        .map(|(window, _)| desk.state.window_id(&window)),
                    client,
                    "{kind}: the Rust walk does not stop at the pane in front"
                );
                assert!(
                    desk.state
                        .snapshot()
                        .windows
                        .iter()
                        .all(|row| row.id != over.get()),
                    "{kind}: the snapshot lists the pane in front"
                );
                assert_eq!(
                    window_at_from_lua(&desk, point),
                    under.pane.get().to_string(),
                    "{kind}: `sol.window_at` does not see through the pane in front to the \
                     window behind -- the two lists agree now, and `owns`'s note is out of date"
                );
            }
        }

        /// **A window that has gone is not laid out again, or closed again,
        /// in the frame it went.**
        ///
        /// The pane outlives `close` by the rest of the frame -- it is
        /// retired in `sync_panes` -- and `trigger_close` retires its close
        /// timers so nothing gives it back. That used to make it an
        /// ordinary window to everything else in the frame: listed in
        /// `sol.windows()` as not leaving, so a stateless layout placed it
        /// and `move_pane` replaced the transform holding it invisible with
        /// one at full opacity, and `close_pane` would start a close on it,
        /// telling a layout `closing` after `close`.
        #[test]
        fn a_window_that_has_gone_is_neither_placed_nor_closed_again() {
            let mut desk = Desk::new();
            let (_, toplevel, pane) = desk.open();
            desk.install(&format!(
                r#"{RECORDER}
sol.on("layout", function()
    for _, window in ipairs(sol.windows()) do
        sol.place(window.id, {{ x = window.x, y = window.y, w = window.w, h = window.h }})
    end
end)
"#
            ));
            desk.state.close_pane(pane);
            desk.ask();
            toplevel.destroy();
            desk.pump();
            let id = pane.get();
            assert!(
                desk.events() == format!("closing {id}*,close {id}*")
                    && desk.state.panes.get(pane).is_some(),
                "the premise: `close` was told and the pane is still here until \
                 `sync_panes`: {}",
                desk.events()
            );

            desk.state.trigger_relayout();
            let now = desk.state.clock.now();
            let drawn = drawn_now(&desk.state, pane, now);
            assert!(
                drawn.opacity.abs() < f32::EPSILON,
                "a layout sweep after `close` drew the window that went at opacity {}",
                drawn.opacity
            );

            desk.state.close_pane(pane);
            assert_eq!(
                desk.events(),
                format!("closing {id}*,close {id}*"),
                "the window that went was closed again, after `close`"
            );
        }

        /// **A `monitors` event in the frame a window went keeps no leaf
        /// for it.** `adopt` exists to put back whatever the arrangement is
        /// missing, and the window that went was still listed, as a window
        /// that was not leaving -- so it was put back into its tree, for
        /// good: nothing sends a second `close`.
        ///
        /// Both settings: `"when_gone"` treats a leaving window as an
        /// ordinary one, so for it only leaving the window that went out of
        /// the list entirely is enough.
        #[test]
        fn adopt_in_the_frame_a_window_went_keeps_no_leaf_for_it() {
            for before in [
                "",
                "require(\"config\").tiling.reflow_on_close = \"when_gone\"",
            ] {
                let mut desk = Desk::new();
                let (_, _, kept) = desk.open();
                let (_, toplevel, going) = desk.open();
                desk.arrange("tiling", before);
                let half = desk.placed(kept);

                desk.state.close_pane(going);
                desk.ask();
                toplevel.destroy();
                desk.pump();
                desk.state.trigger_monitors_changed();
                desk.state.space.refresh();
                desk.state.sync_panes();
                desk.pump();
                assert!(
                    desk.state.panes.get(going).is_none(),
                    "the premise: the pane is retired"
                );

                desk.state.trigger_relayout();
                assert!(
                    desk.placed(kept).size.w > half.size.w,
                    "{before:?}: the tree still divides the screen with a window that is \
                     gone: {:?}",
                    desk.placed(kept)
                );
            }
        }

        /// **A client slower to quit than the grace period moves the layout
        /// three times.** Not a fault this suite can fix and not one it
        /// hides: the compositor cannot tell a client that is still quitting
        /// from one that has decided to stay, so at the grace deadline the
        /// window is given back, and it goes when the client finally does.
        /// A layout that closes up at once therefore grows the neighbour at
        /// the press, shrinks it when `refused` puts the window back, and
        /// grows it again at `close`. `reflow_on_close = "when_gone"` is the
        /// setting that moves it once; `config.lua` says so.
        #[test]
        fn a_client_slower_than_the_grace_period_moves_the_layout_three_times() {
            let mut desk = Desk::new();
            let (_, _, kept) = desk.open();
            let (_, toplevel, slow) = desk.open();
            desk.arrange("tiling", "");
            let half = desk.placed(kept);

            desk.state.close_pane(slow);
            let grown = desk.placed(kept);
            desk.ask();
            desk.refuse();
            let back = desk.placed(kept);
            toplevel.destroy();
            desk.pump();
            let gone = desk.placed(kept);

            let id = slow.get();
            assert_eq!(
                desk.events(),
                format!("closing {id}*,refused {id},close {id}*")
            );
            assert!(
                grown.size.w > half.size.w && back == half && gone.size.w > half.size.w,
                "the neighbour was {half:?}, {grown:?} at the press, {back:?} at the \
                 refusal and {gone:?} once the client went"
            );
        }

        // #128 and #133 together.
        //
        // Every test above commits a 64x64 buffer, which no tile cuts, and
        // no #133 test closes a window, so the cut never engaged in a close
        // or a refusal. The ones below give a window a client that will not
        // shrink as far as its tile, and close it.

        /// Two windows side by side under tiling, and the left one's client
        /// committing a buffer 200 pixels wider than its tile -- a browser
        /// at its minimum width -- while the right one's fills its own:
        /// `(left, right, the left one's tile)`. The left one, because a
        /// tiling sweep places it first and its neighbour after it, which
        /// is the order a close has to raise it against.
        fn an_oversized_window_beside_another(
            desk: &mut Desk,
            before: &str,
        ) -> (Opened, Opened, Rectangle<i32, Logical>) {
            let mut opened: Vec<Opened> = (0..2).map(|_| desk.open_surface()).collect();
            desk.arrange("tiling", before);
            opened.sort_by_key(|each| desk.placed(each.pane).loc.x);
            let right = opened.pop().expect("two windows were opened");
            let left = opened.pop().expect("two windows were opened");
            let tile = desk.placed(left.pane);
            desk.answer(&right);
            commit_buffer(
                &desk.client,
                &desk.qh,
                &left.surface,
                tile.size.w + 200,
                tile.size.h,
            );
            desk.pump();
            desk.state.sync_panes();
            assert_eq!(
                committed(&desk.state, left.pane),
                Size::from((tile.size.w + 200, tile.size.h)),
                "the premise: the client committed more than its tile"
            );
            assert_eq!(
                desk.state.pane_outer_of(left.pane),
                Some(tile),
                "the premise: and it is held inside the tile"
            );
            (left, right, tile)
        }

        /// The size `pane`'s client last committed.
        fn committed(state: &Solium, pane: crate::pane::PaneId) -> Size<i32, Logical> {
            state
                .panes
                .get(pane)
                .and_then(Pane::client)
                .map(|window| window.geometry().size)
                .expect("the pane has a client")
        }

        /// How `pane` is drawn at `at`, and how its client's buffer goes
        /// into that frame: through `render::place_client`, which is what
        /// `render::elements` draws the surfaces with and what
        /// `surface_under` inverts.
        fn pictured(
            state: &Solium,
            pane: crate::pane::PaneId,
            at: Duration,
        ) -> (Frame, crate::render::Placed) {
            let held = state.panes.get(pane).expect("the pane is here");
            let outer = state.pane_outer(held);
            let frame = state.drawn_at(held, outer, at);
            let drawn = crate::render::place_client(
                state,
                held,
                &frame,
                outer.size,
                committed(state, pane),
            );
            (frame, drawn)
        }

        /// What is wrong with how a client wider than its picture is drawn
        /// into a frame, or `None` when nothing is.
        ///
        /// Across, where every window these tests assert on is wider than
        /// anything its frame pictures, the buffer is cut to the drawn
        /// client rectangle and scaled by the frame's zoom and by nothing
        /// else: a picture of the window, cut, and not the window squashed
        /// into it. Down, a glide can picture the window taller than its
        /// buffer and stretch it, as every glide does, so there it is only
        /// held to not being scaled below the zoom. A thousandth under is
        /// allowed for the half pixel `render::fit` does not call a cut;
        /// the squash this looks for is a sixth.
        fn why_not_cut(frame: &Frame, drawn: &crate::render::Placed) -> Option<String> {
            let factor = drawn.fit.factor;
            if drawn.fit.crop != Some(drawn.client) {
                return Some(format!("not cut to its picture: {:?}", drawn.fit));
            }
            if (factor.x - frame.zoom.0).abs() > 1e-9 {
                return Some(format!(
                    "scaled across by {} at a zoom of {}",
                    factor.x, frame.zoom.0
                ));
            }
            if factor.y < frame.zoom.1 * (1.0 - 1e-3) {
                return Some(format!(
                    "squashed down to {} at a zoom of {}",
                    factor.y, frame.zoom.1
                ));
            }
            None
        }

        /// Whether `rect` lies inside `tile`, give or take half a pixel.
        fn inside(rect: Rectangle<f64, Logical>, tile: Rectangle<i32, Logical>) -> bool {
            let tile = tile.to_f64();
            rect.loc.x >= tile.loc.x - 0.5
                && rect.loc.y >= tile.loc.y - 0.5
                && rect.loc.x + rect.size.w <= tile.loc.x + tile.size.w + 0.5
                && rect.loc.y + rect.size.h <= tile.loc.y + tile.size.h + 0.5
        }

        /// Whether two rectangles are the same to within a millionth of a
        /// pixel: a blend's last frame is float arithmetic.
        fn same(one: Rectangle<f64, Logical>, other: Rectangle<f64, Logical>) -> bool {
            (one.loc.x - other.loc.x).abs() < 1e-6
                && (one.loc.y - other.loc.y).abs() < 1e-6
                && (one.size.w - other.size.w).abs() < 1e-6
                && (one.size.h - other.size.h).abs() < 1e-6
        }

        /// Put the pointer at `point`, as a motion with no surface under it
        /// would: what `sol.cursor()` reads when a layout decides where a
        /// new window goes.
        fn point_at(state: &mut Solium, point: Point<f64, Logical>) {
            let pointer = state.seat.get_pointer().expect("the seat has a pointer");
            pointer.motion(
                state,
                None,
                &smithay::input::pointer::MotionEvent {
                    location: point,
                    serial: SERIAL_COUNTER.next_serial(),
                    time: 0,
                },
            );
            pointer.frame(state);
        }

        /// **A window being closed is cut to the tile it left for every
        /// frame of its fade.**
        ///
        /// `closing` takes the window out of the tiling tree and the
        /// neighbour grows into its space at once, and nothing lets the
        /// window out of its tile: `Pane::placed` is kept on purpose,
        /// because it is what `render::fit` cuts the fading window to. The
        /// client committed 200 pixels more than its tile. Cleared either
        /// way below, the assertion on `Pane::placed` fails first, and the
        /// two under it fail on their own without it: cleared at `closing`,
        /// that whole buffer is scaled into every frame of the fade and the
        /// cut assertion fails; cleared before `present::close` reads the
        /// pane's rectangle, the fade starts at the committed width, over
        /// the neighbour, and the one before it fails.
        ///
        /// Every 10 ms from the press to past `CLOSING`, where the window
        /// is held invisible, and across a relayout 80 ms in -- a window
        /// opening or a client's `set_parent` causes one.
        #[test]
        fn a_closed_window_is_cut_to_the_tile_it_left_for_the_whole_fade() {
            let mut desk = Desk::new();
            let (dying, survivor, tile) = an_oversized_window_beside_another(&mut desk, "");
            let beside = desk.placed(survivor.pane);

            let pressed = desk.state.clock.now();
            desk.state.close_pane(dying.pane);
            desk.pump();
            assert!(
                desk.placed(survivor.pane).size.w > beside.size.w,
                "the premise: the neighbour grew into the space at once"
            );

            for step in 0..=21_u64 {
                if step == 8 {
                    desk.state.clock.advance(Duration::from_millis(80));
                    desk.state.trigger_relayout();
                }
                let ms = 10 * step;
                assert_eq!(
                    desk.state.panes.get(dying.pane).and_then(Pane::placed),
                    Some(tile),
                    "{ms}ms into the fade the window was let out of the tile it left"
                );
                let (frame, drawn) =
                    pictured(&desk.state, dying.pane, pressed + Duration::from_millis(ms));
                assert!(
                    inside(frame.rect, tile),
                    "{ms}ms into the fade the window reaches outside the tile it left: \
                     {:?}, and the tile is {tile:?}",
                    frame.rect
                );
                if let Some(why) = why_not_cut(&frame, &drawn) {
                    panic!("{ms}ms into the fade the window is {why}");
                }
            }
            let (held, _) = pictured(
                &desk.state,
                dying.pane,
                pressed + Duration::from_millis(210),
            );
            assert!(
                held.opacity.abs() < f32::EPSILON && (held.zoom.0 - 0.86).abs() < 1e-9,
                "the premise: the fade ran to its held end: {held:?}"
            );
        }

        /// **A mode switched during a fade does not squash the window
        /// leaving.**
        ///
        /// `modes.use` lets every window out of its tile when the layout in
        /// charge changes, and that includes a window being closed. On
        /// stage the let-go cleared `Pane::placed` at once while the fade's
        /// frames stayed pinned at the size of the tile the window was
        /// closed in, so `render::fit`, which cuts only a tiled pane,
        /// scaled the whole of a buffer 200 pixels wider than that tile
        /// into them for the rest of the fade. A let-go now waits while the
        /// window is leaving (`Pane::let_go`), and each way one arrives is
        /// driven 60 ms in: a switch to floating, a switch to scrolling, a
        /// script placing the window with `tile = false`, and a switch to
        /// scrolling with both layouts waiting for the client -- where
        /// scrolling adopts the leaving window and gives it a column during
        /// the fade.
        ///
        /// And a let-go that waited is not lost. Refused after the switch to
        /// floating, the window is in no tile, and its return lands at the
        /// size its client committed, because `give_back` takes the let-go
        /// before it aims the return; aimed first, the return lands at the
        /// tile's width. Refused after scrolling gave it a column, it is
        /// still in a tile: the column cancelled the let-go. That last case
        /// passes on stage too, where nothing waited to be cancelled; it is
        /// here for the cancel, and fails without it.
        #[test]
        fn a_mode_switched_during_a_fade_does_not_squash_the_window_leaving() {
            // Scrolling loaded as well as tiling, so that `super+s` has a
            // layout to switch to.
            const SCROLLING: &str = "require(\"scrolling\")";
            const BOTH_WAIT: &str = "require(\"scrolling\")\n\
                                     require(\"config\").tiling.reflow_on_close = \"when_gone\"\n\
                                     require(\"config\").scrolling.reflow_on_close = \"when_gone\"";
            for (name, before, key) in [
                ("a switch to floating", SCROLLING, Some("super+t")),
                ("a switch to scrolling", SCROLLING, Some("super+s")),
                ("a place with tile = false", SCROLLING, None),
                (
                    "a switch to scrolling waiting for the client",
                    BOTH_WAIT,
                    Some("super+s"),
                ),
            ] {
                let mut desk = Desk::new();
                let (dying, _survivor, tile) =
                    an_oversized_window_beside_another(&mut desk, before);
                let pressed = desk.state.clock.now();
                desk.state.close_pane(dying.pane);
                desk.pump();

                desk.state.clock.advance(Duration::from_millis(60));
                match key {
                    Some(key) => {
                        assert!(desk.state.trigger(key), "{name}: {key} was not handled");
                    }
                    None => desk.state.apply(Outcome {
                        commands: vec![Command::Place {
                            id: dying.pane.get(),
                            rect: to_rect(tile),
                            animation: AnimationSpec::default(),
                            tile: false,
                            inside: None,
                            cramped: false,
                        }],
                        ..Outcome::default()
                    }),
                }
                assert!(
                    desk.state
                        .panes
                        .get(dying.pane)
                        .and_then(Pane::placed)
                        .is_some(),
                    "{name}: the window being closed was let out of its tile during its fade"
                );
                for step in 6..=21_u64 {
                    let ms = 10 * step;
                    let (frame, drawn) =
                        pictured(&desk.state, dying.pane, pressed + Duration::from_millis(ms));
                    if let Some(why) = why_not_cut(&frame, &drawn) {
                        panic!("{name}: {ms}ms into the fade the window is {why}");
                    }
                }

                desk.ask();
                let back = desk.refuse();
                assert!(
                    desk.events()
                        .ends_with(&format!("refused {}", dying.pane.get())),
                    "{name}: the premise is a refusal the layout heard: {}",
                    desk.events()
                );
                let placed = desk.state.panes.get(dying.pane).and_then(Pane::placed);
                if key == Some("super+t") {
                    assert_eq!(
                        placed, None,
                        "{name}: the window came back floating and still held in its old tile"
                    );
                    let outer = desk
                        .state
                        .pane_outer_of(dying.pane)
                        .expect("the pane is here");
                    assert_eq!(
                        outer.size,
                        committed(&desk.state, dying.pane),
                        "{name}: the premise: out of every tile, the pane is the size its \
                         client committed"
                    );
                    let landed = drawn_now(&desk.state, dying.pane, back + Duration::from_secs(1));
                    assert!(
                        same(landed.rect, outer.to_f64()),
                        "{name}: the return lands at {:?} rather than at the size the client \
                         committed, {outer:?}",
                        landed.rect
                    );
                } else {
                    assert!(
                        placed.is_some(),
                        "{name}: the layout in charge holds the window in a tile, and the \
                         refusal let it out"
                    );
                }
            }
        }

        /// **A give-back that declines keeps the fade cut, after a mode
        /// switch as before one.**
        ///
        /// `give_back` takes a let-go that waited before it aims the
        /// window's return, and a give-back can decline -- `present::clear`
        /// answers `false` when the transform slot is busy -- which leaves
        /// the window fading. So what it took is put back, and the fade is
        /// still cut to its tile. Declined here on the dialog route, inside
        /// `CLOSING`, with `present::jam_slot`, 60 ms after a switch to
        /// floating let the window go.
        #[test]
        fn a_give_back_that_declines_keeps_a_fade_cut_after_a_mode_switch() {
            let mut desk = Desk::new();
            let (dying, _survivor, tile) = an_oversized_window_beside_another(&mut desk, "");
            desk.state.close_pane(dying.pane);
            desk.state.clock.advance(Duration::from_millis(60));
            assert!(desk.state.trigger("super+t"), "super+t was not handled");
            if let Some(busy) = desk.state.panes.get(dying.pane) {
                present::jam_slot(busy);
            }
            let _dialog = desk.open_dialog_for(&dying.toplevel);

            let held = desk.state.panes.get(dying.pane).expect("the pane is here");
            assert!(
                held.leaving() && held.answered(),
                "the premise: the dialog answered the close, and the give-back declined"
            );
            assert_eq!(
                held.placed(),
                Some(tile),
                "a give-back that declined let the fading window out of the tile it is cut to"
            );
        }

        /// **A rescue during a fade keeps the let-go the window is waiting
        /// on.**
        ///
        /// `rescue_offscreen` drags a window left on no screen back onto
        /// one, and keeps its standing as it found it -- which for a window
        /// let go of during its close is "tiled for the fade, and owed the
        /// let-go". So it moves the tile and does not set it: setting it is
        /// a layout's newer word and cancels the let-go, and the window,
        /// refused after the switch to floating, would come back held in
        /// the tile it was rescued to.
        #[test]
        fn a_rescue_during_a_fade_keeps_the_let_go_it_is_waiting_on() {
            let mut desk = Desk::new();
            let (dying, _survivor, tile) = an_oversized_window_beside_another(&mut desk, "");
            desk.state.close_pane(dying.pane);
            desk.state.clock.advance(Duration::from_millis(60));
            assert!(desk.state.trigger("super+t"), "super+t was not handled");
            let window = desk
                .state
                .panes
                .get(dying.pane)
                .and_then(Pane::client)
                .cloned()
                .expect("the pane has a client");
            desk.state.map_stacked(window, (-5000, 300), false);
            desk.state.sync_panes();

            desk.state.rescue_offscreen();
            let rescued = desk.state.panes.get(dying.pane).and_then(Pane::placed);
            assert!(
                rescued.is_some_and(|rescued| rescued != tile && rescued.loc.x >= 0),
                "the premise: the rescue brought the fading window back, still in the tile \
                 it is cut to, moved with it: {rescued:?}"
            );
            desk.ask();
            desk.refuse();
            assert_eq!(
                desk.state.panes.get(dying.pane).and_then(Pane::placed),
                None,
                "the rescue cancelled the let-go, and the window came back floating and held \
                 in a tile"
            );
        }

        /// **A refused window wider than its tile is cut to its new tile
        /// from the frame it comes back on, while its zoom grows from the
        /// close's 0.86 to 1.**
        ///
        /// A third window opens while the first is held invisible, so the
        /// tile the refused window comes back to is not the one it was
        /// closed in. Its return starts from the held end of the close --
        /// the old tile's picture, shrunk to 0.86 -- and glides into the new
        /// tile at the layout's pace. The pane is held inside the new tile
        /// from the first frame; every frame draws the buffer at its own
        /// size times the zoom, cut to the picture and never squashed into
        /// it; and the zoom only grows, reaching 1 as the window lands cut
        /// to exactly its new tile.
        #[test]
        fn a_refused_oversized_window_is_cut_to_its_new_tile_as_it_grows_back() {
            let mut desk = Desk::new();
            let (dying, _survivor, tile) = an_oversized_window_beside_another(&mut desk, "");
            let wide = committed(&desk.state, dying.pane);
            desk.state.close_pane(dying.pane);
            desk.ask();
            let _third = desk.open_surface();
            let back = desk.refuse();
            assert!(
                desk.events()
                    .ends_with(&format!("refused {}", dying.pane.get())),
                "the premise is a refusal the layout heard: {}",
                desk.events()
            );
            let new_tile = desk.placed(dying.pane);
            assert!(
                new_tile != tile && new_tile.size.w < wide.w,
                "the premise: back in a different tile, which it is still wider than: \
                 {tile:?} became {new_tile:?}"
            );
            assert_eq!(
                desk.state.pane_outer_of(dying.pane),
                Some(new_tile),
                "the refused window is held inside its new tile from the frame it comes back"
            );

            let (_, fading) = drift((desk.state.clock.now() - back).as_secs_f64());
            let (first, _) = pictured(&desk.state, dying.pane, back);
            assert!(
                (first.zoom.0 - 0.86).abs() < 1e-9 + 0.14 * f64::from(fading)
                    && first.opacity < 0.05 + fading,
                "the return starts from the held end of the close, not from the window at \
                 rest: {first:?}"
            );
            let mut zoom = first.zoom.0;
            for step in 0..=30_u64 {
                let ms = 10 * step;
                let (frame, drawn) =
                    pictured(&desk.state, dying.pane, back + Duration::from_millis(ms));
                if let Some(why) = why_not_cut(&frame, &drawn) {
                    panic!("{ms}ms into the return the window is {why}");
                }
                assert!(
                    frame.zoom.0 >= zoom - 1e-9,
                    "{ms}ms into the return the zoom went back from {zoom} to {}",
                    frame.zoom.0
                );
                zoom = frame.zoom.0;
            }
            let (landed, drawn) = pictured(&desk.state, dying.pane, back + Duration::from_secs(1));
            assert!(
                (landed.zoom.0 - 1.0).abs() < 1e-9 && (landed.zoom.1 - 1.0).abs() < 1e-9,
                "the return lands at a zoom of {:?}",
                landed.zoom
            );
            assert!(
                same(landed.rect, new_tile.to_f64())
                    && drawn
                        .fit
                        .crop
                        .is_some_and(|crop| same(crop, new_tile.to_f64())),
                "the return lands cut to its new tile {new_tile:?}: drawn at {:?}, cut to {:?}",
                landed.rect,
                drawn.fit.crop
            );
        }

        /// **A new window's layout pass part of the way through another
        /// window's open, or through its return from a refused close,
        /// carries that animation on, still cut.**
        ///
        /// `move_pane` starts every glide from what is on screen
        /// (`present::frame`), so a window a sweep moves while it is still
        /// opening, or still fading back in, goes on from where it had got
        /// to -- the zoom it had reached and the opacity -- rather than
        /// from a window at rest. For a client wider than its tile that
        /// zoom is what the cut is a picture of, so both are driven with
        /// one: 80 ms in, with the pointer over it, a new window opens and
        /// the layout splits the window being watched. On the frame the new
        /// window arrives it is drawn where it was, every frame after is
        /// cut at its zoom, and it lands cut to the tile the pass gave it.
        ///
        /// The open is the shipped `open.lua`'s, which `tiling`'s own
        /// placement carries on from, in the order `init.lua` loads them.
        #[test]
        fn a_layout_pass_part_way_through_an_open_or_a_return_keeps_the_window_cut() {
            for case in ["open", "return"] {
                let mut desk = Desk::new();
                let pane = if case == "open" {
                    desk.arrange("tiling", "require(\"open\")");
                    let opening = desk.open_surface();
                    let tile = desk.placed(opening.pane);
                    commit_buffer(
                        &desk.client,
                        &desk.qh,
                        &opening.surface,
                        tile.size.w + 200,
                        tile.size.h,
                    );
                    desk.pump();
                    desk.state.sync_panes();
                    opening.pane
                } else {
                    let (dying, _survivor, _tile) =
                        an_oversized_window_beside_another(&mut desk, "");
                    desk.state.close_pane(dying.pane);
                    desk.ask();
                    desk.refuse();
                    dying.pane
                };
                let was = desk.placed(pane);
                assert!(
                    committed(&desk.state, pane).w > was.size.w,
                    "{case}: the premise: the client is wider than its tile"
                );

                desk.state.clock.advance(Duration::from_millis(80));
                let arrived = desk.state.clock.now();
                let (before, _) = pictured(&desk.state, pane, arrived);
                assert!(
                    before.zoom.0 > 0.9 && before.zoom.0 < 0.99 && before.opacity < 0.99,
                    "{case}: the premise: part of the way in: {before:?}"
                );
                let centre = was.to_f64();
                point_at(
                    &mut desk.state,
                    (
                        centre.loc.x + centre.size.w / 2.0,
                        centre.loc.y + centre.size.h / 2.0,
                    )
                        .into(),
                );
                let _new = desk.open_surface();
                let now = desk.placed(pane);
                assert_ne!(
                    now, was,
                    "{case}: the premise: the new window's pass gave the window a new tile"
                );
                assert!(
                    committed(&desk.state, pane).w > now.size.w,
                    "{case}: the premise: which the client is wider than too"
                );

                let (moving, fading) = drift((desk.state.clock.now() - arrived).as_secs_f64());
                let (after, _) = pictured(&desk.state, pane, arrived);
                assert!(
                    (after.rect.loc.x - before.rect.loc.x).abs() < moving
                        && (after.rect.loc.y - before.rect.loc.y).abs() < moving
                        && (after.rect.size.w - before.rect.size.w).abs() < moving
                        && (after.rect.size.h - before.rect.size.h).abs() < moving
                        && (after.zoom.0 - before.zoom.0).abs() < f64::from(fading)
                        && (after.opacity - before.opacity).abs() < fading,
                    "{case}: the window jumped on the frame the new window arrived: drawn at \
                     {before:?}, and then at {after:?}"
                );
                let mut zoom = after.zoom.0;
                for step in 0..=30_u64 {
                    let ms = 10 * step;
                    let (frame, drawn) =
                        pictured(&desk.state, pane, arrived + Duration::from_millis(ms));
                    if let Some(why) = why_not_cut(&frame, &drawn) {
                        panic!("{case}: {ms}ms after the new window arrived the window is {why}");
                    }
                    assert!(
                        frame.zoom.0 >= zoom - 1e-9,
                        "{case}: {ms}ms after the new window arrived the zoom went back from \
                         {zoom} to {}",
                        frame.zoom.0
                    );
                    zoom = frame.zoom.0;
                }
                let (landed, drawn) = pictured(&desk.state, pane, arrived + Duration::from_secs(1));
                assert!(
                    (landed.zoom.0 - 1.0).abs() < 1e-9
                        && same(landed.rect, now.to_f64())
                        && drawn.fit.crop.is_some_and(|crop| same(crop, now.to_f64())),
                    "{case}: the window lands cut to the tile the pass gave it, {now:?}: drawn \
                     at {landed:?}, cut to {:?}",
                    drawn.fit.crop
                );
            }
        }

        /// **A menu reaching past the tile of a window fading out in front
        /// is that window's while it shows, and not once it has gone.**
        ///
        /// A menu is drawn uncut, past its parent's tile, and a press on it
        /// is the menu's even over the neighbour's tile
        /// (`a_menu_past_its_parents_tile_takes_the_press`). A close raises
        /// the window and fades it in front of the neighbour growing into
        /// its space, so the menu is in front too, over a neighbour that
        /// now reaches under it. Part of the way through the fade a press
        /// on the menu lands on the menu's own pixel through the fade's
        /// zoom -- a menu scales with its window -- and once the window is
        /// invisible the neighbour has it.
        #[test]
        fn a_menu_past_the_tile_of_a_window_fading_out_is_the_windows_while_it_shows() {
            let mut desk = Desk::new();
            let (dying, survivor, tile) = an_oversized_window_beside_another(&mut desk, "");
            let anchor = (tile.size.w - 20, 50);
            let _menu = drawn_popup(
                &mut desk.display,
                &mut desk.state,
                &desk.conn,
                &desk.qh,
                &mut desk.queue,
                &mut desk.client,
                &dying.xdg,
                anchor,
                (120, 80),
            );
            desk.state.sync_panes();
            // The compositor's side of each surface, which is what
            // `surface_under` answers with.
            let toplevel_of = |state: &Solium, pane| {
                state
                    .panes
                    .get(pane)
                    .and_then(Pane::client)
                    .and_then(Window::toplevel)
                    .map(|toplevel| toplevel.wl_surface().clone())
                    .expect("the pane has an xdg toplevel")
            };
            let menu = smithay::desktop::PopupManager::popups_for_surface(&toplevel_of(
                &desk.state,
                dying.pane,
            ))
            .next()
            .map(|(popup, _)| popup.wl_surface().clone())
            .expect("the menu is tracked on its parent");
            let neighbour = toplevel_of(&desk.state, survivor.pane);

            desk.state.close_pane(dying.pane);
            desk.pump();
            desk.state.sync_panes();
            let beside = desk.placed(survivor.pane);

            desk.state.clock.advance(Duration::from_millis(60));
            let (frame, drawn) = pictured(&desk.state, dying.pane, desk.state.clock.now());
            assert!(
                frame.shows() && frame.opacity < 0.95 && frame.zoom.0 < 0.99,
                "the premise: part of the way through the fade: {frame:?}"
            );
            // The menu's pixel 60,40, where the fade draws it: the buffer's
            // corner, and the menu's place in the window plus the pixel's
            // place in the menu, scaled with the window.
            let into = (60.0, 40.0);
            let on_the_menu =
                |drawn: &crate::render::Placed, into: (f64, f64)| -> Point<f64, Logical> {
                    (
                        drawn.origin.x + (f64::from(anchor.0) + into.0) * drawn.fit.factor.x,
                        drawn.origin.y + (f64::from(anchor.1) + into.1) * drawn.fit.factor.y,
                    )
                        .into()
                };
            let point = on_the_menu(&drawn, into);
            assert!(
                point.x > f64::from(tile.loc.x + tile.size.w) && beside.to_f64().contains(point),
                "the premise: the point is past the fading window's tile, over the neighbour \
                 that has grown into its space: {point:?}, {tile:?}, {beside:?}"
            );
            let (under, origin) = desk
                .state
                .surface_under(point)
                .expect("something is under the point");
            assert_eq!(
                under, menu,
                "the menu of a window fading in front of its neighbour lost the pixel it drew"
            );
            let within = point - origin;
            assert!(
                (within.x - into.0).abs() < 1.0 && (within.y - into.1).abs() < 1.0,
                "a press on the fading menu reached {within:?} of it, not {into:?}"
            );

            // And where the held end of the fade would draw a pixel of the
            // menu, had it anything to draw: the menu shrinks with its
            // window, so the point it had 60 ms in is no longer on it. Its
            // pixel 110,40, which the held end still draws past the tile.
            desk.state.clock.advance(Duration::from_millis(150));
            let (gone, drawn) = pictured(&desk.state, dying.pane, desk.state.clock.now());
            assert!(!gone.shows(), "the premise: the fade is over: {gone:?}");
            let point = on_the_menu(&drawn, (110.0, 40.0));
            assert!(
                point.x > f64::from(tile.loc.x + tile.size.w) && beside.to_f64().contains(point),
                "the premise: that point is past the tile too, over the neighbour: {point:?}"
            );
            assert_eq!(
                desk.state.surface_under(point).map(|(surface, _)| surface),
                Some(neighbour),
                "once the window is invisible its menu has no pixel, and the neighbour \
                 has the press"
            );
        }

        // #126: a window its own application closes.
        //
        // Every test above closes a window through `close_pane`, which
        // animates a live client and asks it afterwards. The ones below
        // close it from the client's side, which is what `exit`, `Ctrl+D`
        // and an application's own Quit are -- and a disconnect, which is
        // the same objects going in another order.

        /// What a pane kept of a window whose client has gone.
        fn left_of(state: &Solium, pane: crate::pane::PaneId) -> &crate::pane::Left {
            state
                .panes
                .get(pane)
                .and_then(Pane::left)
                .expect("the pane survives its client, as what fades out")
        }

        /// Let every animation so far land, so what follows starts from a
        /// window drawn where it lives.
        fn land(desk: &mut Desk) {
            desk.state.clock.advance(Duration::from_secs(1));
            let now = desk.state.clock.now();
            desk.state.settle(now);
            desk.state.space.refresh();
            desk.state.sync_panes();
        }

        /// A second client of the desk's compositor, for a test that has
        /// to disconnect one: `(connection, queue, client)`.
        fn another_client(
            desk: &mut Desk,
        ) -> (Connection, wayland_client::EventQueue<Client>, Client) {
            connect(&mut desk.display, &mut desk.state)
        }

        /// **#126: a window whose client destroys its own toplevel fades
        /// out as one the compositor closes does, and is gone on time with
        /// nothing else happening.**
        ///
        /// It used to get no frames at all: the pane was retired by the
        /// first `sync_panes` after the client went, and the frame drawn
        /// before that had nothing of the client left to draw. Here the
        /// pane survives as `Content::Leaving`, pinned to where it stood,
        /// under `present::close`'s transform -- and is dropped by the
        /// frame's own `settle` within a frame of `pane::LEAVING`, with no
        /// client event, no `sync_panes` and no layout behind it.
        #[test]
        fn a_window_that_closes_itself_fades_out_and_is_gone_on_time() {
            let mut desk = Desk::new();
            let opened = desk.open_surface();
            let pane = opened.pane;
            land(&mut desk);
            let stood = desk.state.pane_outer_of(pane).expect("a mapped pane");

            opened.toplevel.destroy();
            desk.pump();

            let left = left_of(&desk.state, pane);
            let since = left.since;
            assert_eq!(left.outer, stood, "it fades from the rectangle it stood at");
            assert!(
                matches!(left.remains, crate::pane::Remains::Lost),
                "no renderer here, so nothing was imported: it is drawn as its frame \
                 and a fill, and never as nothing"
            );
            assert!(
                desk.state
                    .on_screen()
                    .iter()
                    .any(|(each, window)| *each == pane && window.is_none()),
                "it is in the walk the renderer draws, and has no client"
            );

            let at =
                |state: &Solium, after: Duration| state.drawn_id_at(pane, stood, since + after);
            let start = at(&desk.state, Duration::ZERO);
            let half = at(&desk.state, present::CLOSING / 2);
            let end = at(&desk.state, present::CLOSING);
            assert!(
                (start.opacity - 1.0).abs() < 1e-3 && start.rect == stood.to_f64(),
                "it starts as it stood: {start:?}"
            );
            let centre = |rect: Rectangle<f64, Logical>| {
                (
                    rect.loc.x + rect.size.w / 2.0,
                    rect.loc.y + rect.size.h / 2.0,
                )
            };
            let (cx, cy) = centre(stood.to_f64());
            let (hx, hy) = centre(half.rect);
            assert!(
                half.opacity > 0.1
                    && half.opacity < 0.9
                    && half.rect.size.w < f64::from(stood.size.w)
                    && half.rect.size.h < f64::from(stood.size.h)
                    && (hx - cx).abs() < 1.0
                    && (hy - cy).abs() < 1.0,
                "half way through it is shrinking and fading where it stood, as \
                 `present::close` draws a close: {half:?}"
            );
            assert!(
                !end.shows(),
                "and at the end of the fade it is gone: {end:?}"
            );

            let frame = Duration::from_millis(16);
            assert!(
                desk.state.settle(since + crate::pane::LEAVING - frame),
                "while it fades the frames keep coming: nothing else would ask for one"
            );
            assert!(desk.state.panes.get(pane).is_some(), "and it is still here");
            desk.state.settle(since + crate::pane::LEAVING + frame);
            assert!(
                desk.state.panes.get(pane).is_none(),
                "a frame after its fade it is gone, and only that frame's settle ran"
            );
        }

        /// **#126: a client that disconnects -- a crash, a `kill` -- goes
        /// the same way, and is told gone once.**
        ///
        /// Its objects are destroyed in id order, so its surface goes
        /// before its toplevel: `CompositorHandler::destroyed` hears of it
        /// first and takes it out of the space, and `toplevel_destroyed`
        /// then has nothing to find. Two notices, one `close`.
        #[test]
        fn a_client_that_disconnects_fades_out_and_is_told_gone_once() {
            let mut desk = Desk::new();
            desk.install(RECORDER);
            let (conn, queue, client) = another_client(&mut desk);
            let qh = queue.handle();
            let (window, toplevel) =
                open_window(&mut desk.display, &mut desk.state, &conn, &client, &qh);
            desk.state.sync_panes();
            let pane = desk.state.panes.id_of(&window).expect("a pane");
            land(&mut desk);
            let stood = desk.state.pane_outer_of(pane).expect("a mapped pane");

            drop((toplevel, qh, queue, client, conn));
            desk.display
                .dispatch_clients(&mut desk.state)
                .expect("dispatching the disconnect");
            desk.display.flush_clients().expect("flushing");

            let left = left_of(&desk.state, pane);
            assert_eq!(left.outer, stood, "it fades from where it stood");
            assert!(
                !desk.state.space.elements().any(|each| *each == window),
                "and the space has let go of it"
            );
            let since = left.since;
            let half = desk
                .state
                .drawn_id_at(pane, stood, since + present::CLOSING / 2);
            assert!(
                half.opacity > 0.1 && half.opacity < 0.9,
                "half way through its fade: {half:?}"
            );
            let id = pane.get();
            assert_eq!(desk.events(), format!("open {id},close {id}*"));

            desk.state.space.refresh();
            desk.state.sync_panes();
            desk.state
                .settle(since + crate::pane::LEAVING + Duration::from_millis(16));
            assert!(desk.state.panes.get(pane).is_none(), "and gone on time");
            assert_eq!(
                desk.events(),
                format!("open {id},close {id}*"),
                "told once, however many ways it was heard going"
            );
        }

        /// **#126: several windows of one application going at once each
        /// fade, and each is told gone once.**
        #[test]
        fn every_window_of_a_client_that_disconnects_fades_out() {
            let mut desk = Desk::new();
            desk.install(RECORDER);
            let (conn, queue, client) = another_client(&mut desk);
            let qh = queue.handle();
            let (first, one) = open_window(&mut desk.display, &mut desk.state, &conn, &client, &qh);
            let (second, two) =
                open_window(&mut desk.display, &mut desk.state, &conn, &client, &qh);
            desk.state.sync_panes();
            let panes = [&first, &second].map(|window| {
                desk.state
                    .panes
                    .id_of(window)
                    .expect("a pane for each window")
            });
            land(&mut desk);

            drop((one, two, qh, queue, client, conn));
            desk.display
                .dispatch_clients(&mut desk.state)
                .expect("dispatching the disconnect");

            for pane in panes {
                assert!(
                    desk.state.panes.get(pane).is_some_and(Pane::ghost),
                    "each window is fading out"
                );
            }
            let [a, b] = panes.map(crate::pane::PaneId::get);
            assert_eq!(
                desk.events(),
                format!("open {a},open {b},close {a}*,close {b}*"),
                "and each is told gone, once"
            );
        }

        /// **#126: what the renderer imported is what a window leaves --
        /// taken in `toplevel_destroyed`, every surface of its tree, and
        /// still there after the surfaces are destroyed.**
        ///
        /// smithay's `DummyRenderer` stands in for the GPU: it imports
        /// shared memory into a texture of the buffer's size and files it
        /// by its context, as GLES does, with no GPU at all. The client
        /// destroys the lot in the order an orderly exit does, in one flush,
        /// so by the time anything could look the surfaces are gone -- and
        /// smithay's own reset has dropped what it held for them.
        #[test]
        fn an_orderly_exit_leaves_the_picture_the_renderer_imported() {
            use smithay::backend::renderer::element::surface::{
                WaylandSurfaceRenderElement, render_elements_from_surface_tree,
            };
            use smithay::backend::renderer::{Renderer as _, test::DummyRenderer};

            let mut desk = Desk::new();
            desk.state.textures = Some(crate::remains::Textures::Dummy(DummyRenderer.context_id()));
            let opened = desk.open_surface();
            let compositor = desk.client.compositor.clone().expect("wl_compositor bound");
            let subcompositor = desk
                .client
                .subcompositor
                .clone()
                .expect("wl_subcompositor bound");
            let child = compositor.create_surface(&desk.qh, ());
            let sub = subcompositor.get_subsurface(&child, &opened.surface, &desk.qh, ());
            sub.set_position(8, 8);
            sub.set_desync();
            commit_buffer(&desk.client, &desk.qh, &child, 16, 16);
            opened.surface.commit();
            desk.pump();

            // What a frame does: import every surface of the tree.
            let root = desk
                .state
                .panes
                .get(opened.pane)
                .and_then(Pane::client)
                .and_then(|window| window.wl_surface().map(std::borrow::Cow::into_owned))
                .expect("the window's surface");
            let drawn: Vec<WaylandSurfaceRenderElement<DummyRenderer>> =
                render_elements_from_surface_tree(
                    &mut DummyRenderer,
                    &root,
                    (0, 0),
                    1.0,
                    1.0,
                    smithay::backend::renderer::element::Kind::Unspecified,
                );
            assert_eq!(drawn.len(), 2, "the premise: two surfaces drawn");

            opened.toplevel.destroy();
            opened.xdg.destroy();
            sub.destroy();
            child.destroy();
            opened.surface.destroy();
            desk.pump();

            assert!(
                smithay::backend::renderer::utils::with_renderer_surface_state(&root, |state| {
                    state.buffer().is_none()
                })
                .unwrap_or(true),
                "the premise: smithay has let go of the surface's state"
            );
            let crate::pane::Remains::Picture(picture) = &left_of(&desk.state, opened.pane).remains
            else {
                panic!("a window with a picture fades out from it");
            };
            assert_eq!(
                picture.dummy_sizes(),
                vec![(16, 16), (64, 64)],
                "both surfaces, topmost first, from the textures imported before \
                 the client went"
            );
        }

        /// **#126: the same for a client that disconnects**, which is heard
        /// of at its surface's destruction rather than its toplevel's --
        /// before smithay unlinks the surface from its tree or runs the
        /// hook that drops what the renderer imported for it.
        #[test]
        fn a_client_that_disconnects_leaves_the_picture_the_renderer_imported() {
            use smithay::backend::renderer::element::surface::{
                WaylandSurfaceRenderElement, render_elements_from_surface_tree,
            };
            use smithay::backend::renderer::{Renderer as _, test::DummyRenderer};

            let mut desk = Desk::new();
            desk.state.textures = Some(crate::remains::Textures::Dummy(DummyRenderer.context_id()));
            let (conn, mut queue, mut client) = another_client(&mut desk);
            let qh = queue.handle();
            let (window, toplevel, surface) =
                open_surface(&mut desk.display, &mut desk.state, &conn, &client, &qh);
            desk.state.sync_panes();
            let pane = desk.state.panes.id_of(&window).expect("a pane");
            let compositor = client.compositor.clone().expect("wl_compositor bound");
            let subcompositor = client
                .subcompositor
                .clone()
                .expect("wl_subcompositor bound");
            let child = compositor.create_surface(&qh, ());
            let sub = subcompositor.get_subsurface(&child, &surface, &qh, ());
            sub.set_desync();
            commit_buffer(&client, &qh, &child, 16, 16);
            surface.commit();
            pump(
                &mut desk.display,
                &mut desk.state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            let root = window
                .wl_surface()
                .map(std::borrow::Cow::into_owned)
                .expect("the window's surface");
            let drawn: Vec<WaylandSurfaceRenderElement<DummyRenderer>> =
                render_elements_from_surface_tree(
                    &mut DummyRenderer,
                    &root,
                    (0, 0),
                    1.0,
                    1.0,
                    smithay::backend::renderer::element::Kind::Unspecified,
                );
            assert_eq!(drawn.len(), 2, "the premise: two surfaces drawn");

            drop((sub, child, surface, toplevel, qh, queue, client, conn));
            desk.display
                .dispatch_clients(&mut desk.state)
                .expect("dispatching the disconnect");

            let crate::pane::Remains::Picture(picture) = &left_of(&desk.state, pane).remains else {
                panic!("a window with a picture fades out from it");
            };
            assert_eq!(picture.dummy_sizes(), vec![(16, 16), (64, 64)]);
        }

        /// **#126's review: a client that disconnects keeps a subsurface
        /// whose `wl_surface` is older than its window's.** Its objects go
        /// in id order, and ids are recycled, so a video's or a page's
        /// surface can be destroyed -- and unlinked, and its pixels dropped
        /// -- before the window's own is. The picture is taken at the first
        /// surface of the window's tree to go, while all of it is there.
        ///
        /// The subsurface's `wl_surface` is created before the window's, so
        /// its id is lower; its `wl_subsurface` after both, so that is
        /// destroyed after the window's surface and unlinks nothing first.
        /// One whose `wl_subsurface` is the older is
        /// `a_client_that_disconnects_keeps_a_subsurface_whose_wl_subsurface_is_older`.
        #[test]
        fn a_client_that_disconnects_keeps_a_subsurface_older_than_its_window() {
            use smithay::backend::renderer::element::surface::{
                WaylandSurfaceRenderElement, render_elements_from_surface_tree,
            };
            use smithay::backend::renderer::{Renderer as _, test::DummyRenderer};

            let mut desk = Desk::new();
            desk.state.textures = Some(crate::remains::Textures::Dummy(DummyRenderer.context_id()));
            let (conn, mut queue, mut client) = another_client(&mut desk);
            let qh = queue.handle();
            let compositor = client.compositor.clone().expect("wl_compositor bound");
            let child = compositor.create_surface(&qh, ());
            let (window, toplevel, surface) =
                open_surface(&mut desk.display, &mut desk.state, &conn, &client, &qh);
            desk.state.sync_panes();
            let pane = desk.state.panes.id_of(&window).expect("a pane");
            let subcompositor = client
                .subcompositor
                .clone()
                .expect("wl_subcompositor bound");
            let sub = subcompositor.get_subsurface(&child, &surface, &qh, ());
            sub.set_desync();
            commit_buffer(&client, &qh, &child, 16, 16);
            surface.commit();
            pump(
                &mut desk.display,
                &mut desk.state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            assert!(
                wayland_client::Proxy::id(&child).protocol_id()
                    < wayland_client::Proxy::id(&surface).protocol_id(),
                "the premise: the subsurface's surface is the older"
            );
            let root = window
                .wl_surface()
                .map(std::borrow::Cow::into_owned)
                .expect("the window's surface");
            let drawn: Vec<WaylandSurfaceRenderElement<DummyRenderer>> =
                render_elements_from_surface_tree(
                    &mut DummyRenderer,
                    &root,
                    (0, 0),
                    1.0,
                    1.0,
                    smithay::backend::renderer::element::Kind::Unspecified,
                );
            assert_eq!(drawn.len(), 2, "the premise: two surfaces drawn");

            drop((sub, child, surface, toplevel, qh, queue, client, conn));
            desk.display
                .dispatch_clients(&mut desk.state)
                .expect("dispatching the disconnect");

            let crate::pane::Remains::Picture(picture) = &left_of(&desk.state, pane).remains else {
                panic!("a window with a picture fades out from it");
            };
            assert_eq!(
                picture.dummy_sizes(),
                vec![(16, 16), (64, 64)],
                "the subsurface that went first is not in the picture"
            );
        }

        /// **#126's second review: and one whose `wl_subsurface` is older
        /// than its window's surface, and than its own.** That object is
        /// destroyed first, and smithay's destructor for it unlinks the
        /// subsurface from its window and resets where it was, telling
        /// the compositor nothing -- so by the time any surface of the
        /// window was heard going, the page or the video was no longer in
        /// it. The window is now taken at that destructor, for a client
        /// that has gone, while all of it is there.
        ///
        /// The `wl_subsurface` is given an older id by recycling one: a
        /// spare surface made first, before the subsurface's own surface
        /// and the window's, and destroyed before the subsurface is made.
        #[test]
        fn a_client_that_disconnects_keeps_a_subsurface_whose_wl_subsurface_is_older() {
            use smithay::backend::renderer::element::surface::{
                WaylandSurfaceRenderElement, render_elements_from_surface_tree,
            };
            use smithay::backend::renderer::{Renderer as _, test::DummyRenderer};

            let mut desk = Desk::new();
            desk.install(RECORDER);
            desk.state.textures = Some(crate::remains::Textures::Dummy(DummyRenderer.context_id()));
            let (conn, mut queue, mut client) = another_client(&mut desk);
            let qh = queue.handle();
            let compositor = client.compositor.clone().expect("wl_compositor bound");
            let spare = compositor.create_surface(&qh, ());
            let child = compositor.create_surface(&qh, ());
            let (window, toplevel, surface) =
                open_surface(&mut desk.display, &mut desk.state, &conn, &client, &qh);
            desk.state.sync_panes();
            let pane = desk.state.panes.id_of(&window).expect("a pane");
            spare.destroy();
            pump(
                &mut desk.display,
                &mut desk.state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            let subcompositor = client
                .subcompositor
                .clone()
                .expect("wl_subcompositor bound");
            let sub = subcompositor.get_subsurface(&child, &surface, &qh, ());
            sub.set_desync();
            commit_buffer(&client, &qh, &child, 16, 16);
            surface.commit();
            pump(
                &mut desk.display,
                &mut desk.state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            let sub_id = wayland_client::Proxy::id(&sub).protocol_id();
            let child_id = wayland_client::Proxy::id(&child).protocol_id();
            let root_id = wayland_client::Proxy::id(&surface).protocol_id();
            assert!(
                sub_id < child_id && sub_id < root_id,
                "the premise: the wl_subsurface ({sub_id}) is older than the subsurface's \
                 surface ({child_id}) and the window's ({root_id})"
            );
            let root = window
                .wl_surface()
                .map(std::borrow::Cow::into_owned)
                .expect("the window's surface");
            let drawn: Vec<WaylandSurfaceRenderElement<DummyRenderer>> =
                render_elements_from_surface_tree(
                    &mut DummyRenderer,
                    &root,
                    (0, 0),
                    1.0,
                    1.0,
                    smithay::backend::renderer::element::Kind::Unspecified,
                );
            assert_eq!(drawn.len(), 2, "the premise: two surfaces drawn");

            drop((sub, child, surface, toplevel, qh, queue, client, conn));
            desk.display
                .dispatch_clients(&mut desk.state)
                .expect("dispatching the disconnect");

            let crate::pane::Remains::Picture(picture) = &left_of(&desk.state, pane).remains else {
                panic!("a window with a picture fades out from it");
            };
            assert_eq!(
                picture.dummy_sizes(),
                vec![(16, 16), (64, 64)],
                "the subsurface whose wl_subsurface went first is not in the picture"
            );
            let id = pane.get();
            assert_eq!(
                desk.events(),
                format!("open {id},close {id}*"),
                "told gone once, at the wl_subsurface, and not again at its surfaces"
            );
        }

        /// **#126's review: a client that takes its subsurfaces down
        /// before its window, and stays connected, fades from what is left
        /// when the window goes.** Pinned rather than fixed: the
        /// subsurface is destroyed while the window is still an ordinary
        /// one, and keeping it would mean keeping a picture of every
        /// subsurface any live window lets go of, on the chance that the
        /// window follows. The window's own surface still fades, so what
        /// is missing is the part the subsurface covered.
        #[test]
        fn a_window_whose_subsurface_goes_first_fades_without_it() {
            use smithay::backend::renderer::element::surface::{
                WaylandSurfaceRenderElement, render_elements_from_surface_tree,
            };
            use smithay::backend::renderer::{Renderer as _, test::DummyRenderer};

            let mut desk = Desk::new();
            desk.state.textures = Some(crate::remains::Textures::Dummy(DummyRenderer.context_id()));
            let opened = desk.open_surface();
            let compositor = desk.client.compositor.clone().expect("wl_compositor bound");
            let subcompositor = desk
                .client
                .subcompositor
                .clone()
                .expect("wl_subcompositor bound");
            let child = compositor.create_surface(&desk.qh, ());
            let sub = subcompositor.get_subsurface(&child, &opened.surface, &desk.qh, ());
            sub.set_desync();
            commit_buffer(&desk.client, &desk.qh, &child, 16, 16);
            opened.surface.commit();
            desk.pump();
            let root = desk
                .state
                .panes
                .get(opened.pane)
                .and_then(Pane::client)
                .and_then(|window| window.wl_surface().map(std::borrow::Cow::into_owned))
                .expect("the window's surface");
            drop(render_elements_from_surface_tree::<
                DummyRenderer,
                WaylandSurfaceRenderElement<DummyRenderer>,
            >(
                &mut DummyRenderer,
                &root,
                (0, 0),
                1.0,
                1.0,
                smithay::backend::renderer::element::Kind::Unspecified,
            ));

            sub.destroy();
            child.destroy();
            opened.toplevel.destroy();
            desk.pump();

            let crate::pane::Remains::Picture(picture) = &left_of(&desk.state, opened.pane).remains
            else {
                panic!("a window with a picture fades out from it");
            };
            assert_eq!(
                picture.dummy_sizes(),
                vec![(64, 64)],
                "the window's own surface, and nothing of the subsurface that went first"
            );
        }

        /// **#126's second review: a live client that destroys a
        /// subsurface's `wl_surface` before its `wl_subsurface` keeps its
        /// window.** Legal, and no shipped toolkit is known to do it: the
        /// subsurface simply goes inert. `CompositorHandler::destroyed`
        /// told a disconnect from this by asking the dying surface for its
        /// client, which libwayland answers `None` for every surface being
        /// destroyed, alive client or not. So it walked up to the window
        /// and ended it: `close` went out, the space let go of a live
        /// toplevel, and nothing mapped it again while its application
        /// went on running.
        ///
        /// And when that client does go, its window fades out as any
        /// window of a client that disconnects does.
        #[test]
        fn a_live_client_destroying_a_subsurfaces_surface_first_keeps_its_window() {
            let mut desk = Desk::new();
            desk.install(RECORDER);
            let (conn, mut queue, mut client) = another_client(&mut desk);
            let qh = queue.handle();
            let (window, toplevel, surface) =
                open_surface(&mut desk.display, &mut desk.state, &conn, &client, &qh);
            desk.state.sync_panes();
            let pane = desk.state.panes.id_of(&window).expect("a pane");
            let compositor = client.compositor.clone().expect("wl_compositor bound");
            let subcompositor = client
                .subcompositor
                .clone()
                .expect("wl_subcompositor bound");
            let child = compositor.create_surface(&qh, ());
            let sub = subcompositor.get_subsurface(&child, &surface, &qh, ());
            sub.set_desync();
            commit_buffer(&client, &qh, &child, 16, 16);
            surface.commit();
            pump(
                &mut desk.display,
                &mut desk.state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            land(&mut desk);
            let id = pane.get();
            assert_eq!(
                desk.events(),
                format!("open {id}"),
                "the premise: the window opened, and nothing else"
            );

            child.destroy();
            pump(
                &mut desk.display,
                &mut desk.state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            // And the window goes on drawing, as its application does.
            commit_buffer(&client, &qh, &surface, 64, 64);
            pump(
                &mut desk.display,
                &mut desk.state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            land(&mut desk);

            let held = |desk: &Desk| desk.state.panes.get(pane).and_then(Pane::client).cloned();
            assert_eq!(
                held(&desk),
                Some(window.clone()),
                "a live window left its pane"
            );
            assert!(
                desk.state.space.elements().any(|each| *each == window),
                "the space let go of a live window"
            );
            assert_eq!(
                desk.events(),
                format!("open {id}"),
                "scripts were told a live window closed"
            );

            sub.destroy();
            pump(
                &mut desk.display,
                &mut desk.state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            land(&mut desk);
            assert_eq!(
                held(&desk),
                Some(window.clone()),
                "the inert subsurface's own object going took the window with it"
            );

            drop((sub, child, surface, toplevel, qh, queue, client, conn));
            desk.display
                .dispatch_clients(&mut desk.state)
                .expect("dispatching the disconnect");
            assert!(
                desk.state.panes.get(pane).is_some_and(Pane::ghost),
                "when its client goes, the window fades out"
            );
            assert_eq!(
                desk.events(),
                format!("open {id},close {id}*"),
                "and is told gone, once"
            );
        }

        /// **#126: a window that closed itself keeps the buffer it is drawn
        /// from until its fade is over**, and then the client that closed
        /// it and stayed is told it may have it back.
        ///
        /// Drawn once through `DummyRenderer` first, so that there is a
        /// picture to draw the fade from and so a buffer worth keeping.
        #[test]
        fn a_window_that_closed_itself_keeps_its_last_buffer_until_its_fade_is_over() {
            use smithay::backend::renderer::element::surface::{
                WaylandSurfaceRenderElement, render_elements_from_surface_tree,
            };
            use smithay::backend::renderer::{Renderer as _, test::DummyRenderer};

            let mut desk = Desk::new();
            desk.state.textures = Some(crate::remains::Textures::Dummy(DummyRenderer.context_id()));
            let opened = desk.open_surface();
            let last = commit_buffer(&desk.client, &desk.qh, &opened.surface, 80, 80);
            desk.pump();
            land(&mut desk);
            desk.pump();
            let root = desk
                .state
                .panes
                .get(opened.pane)
                .and_then(Pane::client)
                .and_then(|window| window.wl_surface().map(std::borrow::Cow::into_owned))
                .expect("the window's surface");
            // Dropped at once, as a frame's elements are: each holds the
            // buffer it was built from, and one kept here would hold it
            // for this test rather than for the fade.
            drop(render_elements_from_surface_tree::<
                DummyRenderer,
                WaylandSurfaceRenderElement<DummyRenderer>,
            >(
                &mut DummyRenderer,
                &root,
                (0, 0),
                1.0,
                1.0,
                smithay::backend::renderer::element::Kind::Unspecified,
            ));
            let last = wayland_client::Proxy::id(&last);
            assert!(
                !desk.client.released.contains(&last),
                "the premise: the buffer on screen is not released"
            );

            opened.toplevel.destroy();
            opened.xdg.destroy();
            opened.surface.destroy();
            desk.pump();
            assert!(
                !desk.client.released.contains(&last),
                "a buffer the fade is drawn from was handed back while it was drawn"
            );

            let since = left_of(&desk.state, opened.pane).since;
            desk.state
                .settle(since + crate::pane::LEAVING + Duration::from_millis(16));
            desk.pump();
            assert!(
                desk.client.released.contains(&last),
                "and handed back once the fade is over"
            );
        }

        /// **#126: a window whose client has gone is drawn, and is nothing
        /// else.**
        ///
        /// Two windows overlapping and no layout, the upper one closing
        /// itself. While it fades:
        ///
        /// * it is in no window list a script sees;
        /// * the pointer, a press and a resize border under it all reach
        ///   the window underneath -- a pane with no client that answered
        ///   "covered" would be the input black hole the loading pane is
        ///   on purpose, and a titlebar or a border it offered would be a
        ///   press nothing takes;
        /// * the keyboard goes to the window underneath;
        /// * `sol.place` does not move it, and `close_pane` does nothing,
        ///   neither `closing` nor a second `close`.
        #[test]
        fn a_window_that_left_is_nobodys_to_find() {
            let mut desk = Desk::new();
            let lower = desk.open_surface();
            let upper = desk.open_surface();
            for (opened, at) in [(&lower, (100, 100)), (&upper, (200, 200))] {
                commit_buffer(&desk.client, &desk.qh, &opened.surface, 400, 300);
                desk.pump();
                let window = desk
                    .state
                    .panes
                    .get(opened.pane)
                    .and_then(Pane::client)
                    .cloned()
                    .expect("a window");
                desk.state.map_stacked(window, at, false);
            }
            land(&mut desk);
            let (ghost, below) = (upper.pane.get(), lower.pane.get());
            desk.install(&format!(
                "{RECORDER}\nsol.bind(\"super+x\", function() \
                 sol.place({ghost}, {{ x = 0, y = 0, w = 50, h = 50 }}) end)"
            ));
            let lower_window = desk
                .state
                .panes
                .get(lower.pane)
                .and_then(Pane::client)
                .cloned()
                .expect("the lower window");
            let lower_surface = lower_window
                .wl_surface()
                .map(std::borrow::Cow::into_owned)
                .expect("its surface");
            // Inside both, and on the lower window's right-hand border.
            let point: Point<f64, Logical> = (499.0, 300.0).into();
            upper.toplevel.set_title("the window that went".to_owned());
            desk.pump();
            assert!(
                desk.state.looks_focused(upper.pane),
                "the premise: the window going is the focused one"
            );

            upper.toplevel.destroy();
            desk.pump();
            let stood = left_of(&desk.state, upper.pane).outer;
            assert!(
                stood.to_f64().contains(point) && desk.state.drawn(upper.pane, stood).covers(point),
                "the premise: the window that went is drawn over the point"
            );

            assert!(
                desk.state
                    .snapshot()
                    .windows
                    .iter()
                    .all(|row| row.id != ghost),
                "a window that went is in a script's window list"
            );
            assert_eq!(
                desk.state.window_under(point).map(|(window, _)| window),
                Some(lower_window),
                "the pointer stopped at a window that went"
            );
            assert_eq!(
                desk.state.surface_under(point).map(|(surface, _)| surface),
                Some(lower_surface),
                "a press went to a window that went"
            );
            assert_eq!(
                desk.state
                    .chrome_under(point)
                    .map(|under| (under.pane, under.chrome)),
                Some((lower.pane, Chrome::Resize(ResizeEdge::Right))),
                "the border under a window that went is not the lower window's to drag"
            );

            desk.state.sync_panes();
            assert_eq!(
                desk.state
                    .focused_window()
                    .and_then(|window| desk.state.panes.id_of(&window)),
                Some(lower.pane),
                "the keyboard goes to the window that is still here"
            );
            assert!(
                desk.state.looks_focused(upper.pane)
                    && desk.state.pane_title(upper.pane) == "the window that went",
                "and the frame fading out is drawn as it stood: focused, and titled"
            );
            point_at(&mut desk.state, point);
            assert!(
                desk.state.pointer_over(lower.pane) && !desk.state.pointer_over(upper.pane),
                "a frame fading out lights up for the pointer over it, as if a press there \
                 would do something"
            );

            let tile = desk.state.panes.get(upper.pane).and_then(Pane::placed);
            assert!(
                desk.state.trigger("super+x"),
                "the premise: the binding ran"
            );
            assert_eq!(
                desk.state.panes.get(upper.pane).and_then(Pane::placed),
                tile,
                "a script placing it moved the tile its picture is cut to"
            );
            assert_eq!(
                left_of(&desk.state, upper.pane).outer,
                stood,
                "a script placing it moved it"
            );
            assert_eq!(
                desk.state.pane_outer_of(upper.pane),
                Some(stood),
                "a script placing it moved where it is drawn"
            );
            desk.state.close_pane(upper.pane);
            assert!(
                desk.state
                    .panes
                    .get(upper.pane)
                    .is_some_and(|pane| pane.closing_at().is_none()),
                "closing it by hand started a close"
            );
            assert_eq!(
                desk.events(),
                format!("close {ghost}*"),
                "and nothing was told anything but `close`, once: {below} stayed"
            );
        }

        /// **#126: a layout reflows at `close` and the window fades where it
        /// stood while its neighbour grows in** -- the picture #128 gives a
        /// close the compositor asks for, with no `closing`, because nobody
        /// asked for this one. The window is in no arrangement afterwards:
        /// an `adopt`, on a `monitors` event, finds nothing to put back.
        ///
        /// It fades in front of every window the layout moved, and
        /// everywhere else keeps its place in the stack (#126's review): in
        /// scrolling the column on the far side does not move, was over it,
        /// and stays over it. See `crate::pane::Left::over`.
        #[test]
        fn a_window_that_closes_itself_hands_its_space_over_as_it_fades() {
            for layout in LAYOUTS {
                let mut desk = Desk::new();
                let (closed, others) = closing_scene(&mut desk, layout);
                let before: Vec<Rectangle<i32, Logical>> =
                    others.iter().map(|other| desk.placed(other.pane)).collect();
                let stood = desk
                    .state
                    .pane_outer_of(closed.pane)
                    .expect("a mapped pane");
                let stacked_over: Vec<bool> = others
                    .iter()
                    .map(|other| above(&mut desk.state, closed.pane, other.pane))
                    .collect();

                closed.toplevel.destroy();
                desk.pump();

                let id = closed.pane.get();
                assert_eq!(
                    desk.events()
                        .split(',')
                        .filter(|event| !event.starts_with("open"))
                        .collect::<Vec<_>>(),
                    vec![format!("close {id}*")],
                    "{layout}: `close`, once, and nothing else"
                );
                assert!(
                    others
                        .iter()
                        .zip(&before)
                        .any(|(other, was)| desk.placed(other.pane) != *was),
                    "{layout}: the layout closed up at `close`"
                );
                let since = left_of(&desk.state, closed.pane).since;
                let half = desk
                    .state
                    .drawn_id_at(closed.pane, stood, since + present::CLOSING / 2);
                let (cx, cy) = (
                    f64::from(stood.loc.x) + f64::from(stood.size.w) / 2.0,
                    f64::from(stood.loc.y) + f64::from(stood.size.h) / 2.0,
                );
                assert!(
                    half.shows()
                        && (half.rect.loc.x + half.rect.size.w / 2.0 - cx).abs() < 1.0
                        && (half.rect.loc.y + half.rect.size.h / 2.0 - cy).abs() < 1.0,
                    "{layout}: half way through, it is fading where it stood: {half:?} \
                     against {stood:?}"
                );
                let crate::pane::Left { outer, .. } = left_of(&desk.state, closed.pane);
                assert_eq!(*outer, stood, "{layout}: and the layout did not move it");
                for ((other, was), over) in others.iter().zip(&before).zip(&stacked_over) {
                    let moved = desk.placed(other.pane) != *was;
                    assert_eq!(
                        above(&mut desk.state, closed.pane, other.pane),
                        moved || *over,
                        "{layout}: it fades in front of a window moving in, as a close \
                         the compositor asked for does, and otherwise where it was in the \
                         stack (moved: {moved}, over it before: {over})"
                    );
                }

                // `adopt` puts back whatever an arrangement is missing,
                // from `sol.windows()`, and runs on `monitors`.
                let grown: Vec<Rectangle<i32, Logical>> =
                    others.iter().map(|other| desk.placed(other.pane)).collect();
                desk.state.trigger_monitors_changed();
                desk.state.space.refresh();
                desk.state.sync_panes();
                assert!(
                    desk.state.panes.get(closed.pane).is_some_and(Pane::ghost),
                    "{layout}: the premise: it is still fading"
                );
                let now = since + crate::pane::LEAVING + Duration::from_millis(16);
                desk.state.settle(now);
                desk.state.trigger_relayout();
                assert_eq!(
                    others
                        .iter()
                        .map(|other| desk.placed(other.pane))
                        .collect::<Vec<_>>(),
                    grown,
                    "{layout}: adopted and laid out again, the arrangement made room \
                     for the window that went"
                );
                assert_eq!(
                    desk.events()
                        .split(',')
                        .filter(|event| !event.starts_with("open"))
                        .count(),
                    1,
                    "{layout}: and it was never told of again"
                );
            }
        }

        /// **#126: a client that quits part of the way through a close the
        /// compositor asked for leaves on that close**: from its transform,
        /// with nothing restarted, and gone when that close's fade is over.
        /// A client that exits the moment it is asked -- inside `CLOSING`,
        /// before the request even goes out, as one closing on a second
        /// press of its own shortcut does -- is exactly this.
        #[test]
        fn a_client_that_quits_during_a_close_leaves_on_that_close() {
            let mut desk = Desk::new();
            desk.install(RECORDER);
            let opened = desk.open_surface();
            let pane = opened.pane;
            land(&mut desk);
            let stood = desk.state.pane_outer_of(pane).expect("a mapped pane");

            desk.state.close_pane(pane);
            let due = desk
                .state
                .panes
                .get(pane)
                .and_then(Pane::closing_at)
                .expect("the premise: a close is playing");
            let began = due - present::CLOSING;
            let fading = desk
                .state
                .drawn_id_at(pane, stood, began + present::CLOSING / 2);

            desk.state.clock.advance(Duration::from_millis(60));
            opened.toplevel.destroy();
            desk.pump();

            let left = left_of(&desk.state, pane);
            assert_eq!(left.since, began, "its fade began at the press");
            assert_eq!(
                desk.state
                    .drawn_id_at(pane, stood, began + present::CLOSING / 2),
                fading,
                "and it goes on with that fade rather than starting another"
            );
            let id = pane.get();
            assert_eq!(
                desk.events(),
                format!("open {id},closing {id}*,close {id}*")
            );
            desk.state
                .settle(began + crate::pane::LEAVING + Duration::from_millis(1));
            assert!(
                desk.state.panes.get(pane).is_none(),
                "gone when that close's fade is over"
            );
        }

        /// **#126: a window that goes keeps the shift its selections had**,
        /// however a script rebuilds them after it. `workspaces.lua`
        /// rebuilds a desk's membership from `sol.windows()` on every
        /// `layout` event, and a window that has gone is not in it -- so a
        /// window on a desk carried a screen away would lose its desk's
        /// shift on the next one and fade over the desk on screen.
        #[test]
        fn a_window_that_left_keeps_the_shift_its_desk_had() {
            let mut desk = Desk::new();
            let opened = desk.open_surface();
            let pane = opened.pane;
            let id = pane.get();
            desk.install(&format!(
                "sol.bind(\"super+g\", function() \
                    sol.group(\"desk\", {{ windows = {{ {id} }} }}) \
                    sol.present_group(\"desk\", {{ x = -500 }}, {{ duration = 1 }}) end)\n\
                 sol.bind(\"super+h\", function() \
                    sol.group(\"desk\", {{ windows = {{}} }}) end)"
            ));
            assert!(desk.state.trigger("super+g"), "the premise: the desk moved");
            land(&mut desk);
            let stood = desk.state.pane_outer_of(pane).expect("a mapped pane");
            let carried = desk.state.drawn(pane, stood).rect;
            assert!(
                (carried.loc.x - f64::from(stood.loc.x - 500)).abs() < 1.0,
                "the premise: the desk carries it 500 to the left: {carried:?}"
            );

            opened.toplevel.destroy();
            desk.pump();
            assert!(
                desk.state.trigger("super+h"),
                "the premise: the desk was rebuilt"
            );
            let since = left_of(&desk.state, pane).since;
            let drawn = desk.state.drawn_id_at(pane, stood, since).rect;
            assert!(
                (drawn.loc.x - carried.loc.x).abs() < 1.0,
                "a desk rebuilt without the window that went moved it back: \
                 {drawn:?}, where the desk had it at {carried:?}"
            );
        }

        /// **#126's review: a window that goes moves with its desk for as
        /// long as it fades**, rather than stopping where the desk had it
        /// when it went. The desk is rebuilt without it and carried back,
        /// the way `workspaces.lua` switches desks: a window closed with
        /// `Ctrl+D` just before a switch used to stay where it was and fade
        /// over the desk sliding in, and one that went during a slide
        /// stopped half way while the rest of its desk carried on.
        ///
        /// Measured against a window staying on the same desk, whose only
        /// transform is the desk's, and at the centre, which a close
        /// shrinks about.
        #[test]
        fn a_window_that_left_moves_with_its_desk() {
            let mut desk = Desk::new();
            let staying = desk.open_surface();
            let going = desk.open_surface();
            let (stays, goes) = (staying.pane.get(), going.pane.get());
            desk.install(&format!(
                "sol.bind(\"super+g\", function() \
                    sol.group(\"desk\", {{ windows = {{ {stays}, {goes} }} }}) \
                    sol.present_group(\"desk\", {{ x = -500 }}, {{ duration = 1 }}) end)\n\
                 sol.bind(\"super+h\", function() \
                    sol.group(\"desk\", {{ windows = {{ {stays} }} }}) \
                    sol.present_group(\"desk\", {{ x = 0 }}, {{ duration = 100 }}) end)"
            ));
            assert!(desk.state.trigger("super+g"), "the premise: the desk moved");
            land(&mut desk);
            let stood = desk.state.pane_outer_of(going.pane).expect("a mapped pane");
            let beside = desk
                .state
                .pane_outer_of(staying.pane)
                .expect("a mapped pane");

            going.toplevel.destroy();
            desk.pump();
            assert!(left_of(&desk.state, going.pane).since > Duration::ZERO);
            let slid = desk.state.clock.now();
            assert!(
                desk.state.trigger("super+h"),
                "the premise: the desk went back"
            );
            let centre = |rect: Rectangle<f64, Logical>| rect.loc.x + rect.size.w / 2.0;
            let at = slid + Duration::from_millis(50);
            let carried = desk.state.drawn_id_at(staying.pane, beside, at).rect.loc.x
                - f64::from(beside.loc.x);
            assert!(
                carried < -1.0 && carried > -499.0,
                "the premise: half way through, the desk is on its way back: {carried}"
            );
            let ghost =
                centre(desk.state.drawn_id_at(going.pane, stood, at).rect) - centre(stood.to_f64());
            assert!(
                (ghost - carried).abs() < 1.0,
                "the window that went is carried {ghost} while its desk is carried {carried}"
            );
        }

        /// The fills `render::remains_elements` draws for a pane fading out,
        /// as premultiplied colours, at the start of its fade.
        fn fills(state: &Solium, pane: crate::pane::PaneId) -> Vec<[f32; 4]> {
            let left = left_of(state, pane);
            let frame = state.drawn_id_at(pane, left.outer, left.since);
            crate::render::remains_elements(state, pane, &frame, left.outer.size, 1.0)
                .into_iter()
                .filter_map(|element| match element {
                    crate::render::Element::Solid(solid) => Some(solid.color().components()),
                    _ => None,
                })
                .collect()
        }

        /// **#126's review: a window that left no picture is filled with a
        /// translucent grey, not an opaque white.**
        ///
        /// Common rather than rare: smithay clears a surface's textures on
        /// every new buffer and imports only when it draws, so a client
        /// that paints and exits inside a frame leaves nothing. The fill
        /// was the shipped `Theme.surface`, white, at full opacity -- a
        /// white rectangle flashed over a dark terminal as it went.
        #[test]
        fn a_window_that_left_no_picture_is_filled_with_a_translucent_grey() {
            let mut desk = Desk::new();
            let opened = desk.open_surface();
            land(&mut desk);
            opened.toplevel.destroy();
            desk.pump();
            assert!(
                matches!(
                    left_of(&desk.state, opened.pane).remains,
                    crate::pane::Remains::Lost
                ),
                "the premise: no renderer, so nothing was imported"
            );
            let fills = fills(&desk.state, opened.pane);
            let [[r, g, b, a]] = fills.as_slice() else {
                panic!("one fill where the client was: {fills:?}");
            };
            assert!(
                *a <= 0.5 + f32::EPSILON,
                "the fill starts at full opacity, over whatever the client showed: {a}"
            );
            assert!(
                (r - g).abs() < f32::EPSILON
                    && (g - b).abs() < f32::EPSILON
                    && *r <= a / 2.0 + f32::EPSILON,
                "the fill is a light colour rather than a mid grey: {:?}",
                [r, g, b, a]
            );
        }

        /// **#126's review: a surface that left no picture is filled where
        /// it was**, so what did survive never stands in a hole. Asked of
        /// the picture as a whole, one surface with pixels meant no fill at
        /// all: a root that went unimported left its subsurface -- a page,
        /// a video -- standing in the window's frame round a hole, and a
        /// subsurface that went unimported left a hole in the root.
        ///
        /// Each case is one surface committing a last buffer after the last
        /// frame that drew it, which is what empties smithay's textures for
        /// it: Firefox, closed nested, committed its page subsurface 6 ms
        /// before destroying its toplevel. `DummyRenderer` draws the frame
        /// before that commit, and imports every surface of the tree.
        #[test]
        fn a_surface_that_left_no_picture_is_filled_where_it_was() {
            use smithay::backend::renderer::element::surface::{
                WaylandSurfaceRenderElement, render_elements_from_surface_tree,
            };
            use smithay::backend::renderer::{Renderer as _, test::DummyRenderer};

            for late in ["root", "subsurface"] {
                let mut desk = Desk::new();
                desk.state.textures =
                    Some(crate::remains::Textures::Dummy(DummyRenderer.context_id()));
                let opened = desk.open_surface();
                let compositor = desk.client.compositor.clone().expect("wl_compositor bound");
                let subcompositor = desk
                    .client
                    .subcompositor
                    .clone()
                    .expect("wl_subcompositor bound");
                let child = compositor.create_surface(&desk.qh, ());
                let sub = subcompositor.get_subsurface(&child, &opened.surface, &desk.qh, ());
                sub.set_position(8, 8);
                sub.set_desync();
                commit_buffer(&desk.client, &desk.qh, &child, 16, 16);
                opened.surface.commit();
                desk.pump();
                let root = desk
                    .state
                    .panes
                    .get(opened.pane)
                    .and_then(Pane::client)
                    .and_then(|window| window.wl_surface().map(std::borrow::Cow::into_owned))
                    .expect("the window's surface");
                drop(render_elements_from_surface_tree::<
                    DummyRenderer,
                    WaylandSurfaceRenderElement<DummyRenderer>,
                >(
                    &mut DummyRenderer,
                    &root,
                    (0, 0),
                    1.0,
                    1.0,
                    smithay::backend::renderer::element::Kind::Unspecified,
                ));
                let (surface, side, hole) = if late == "root" {
                    (
                        &opened.surface,
                        64,
                        Rectangle::new((0, 0).into(), (64, 64).into()),
                    )
                } else {
                    (&child, 16, Rectangle::new((8, 8).into(), (16, 16).into()))
                };
                commit_buffer(&desk.client, &desk.qh, surface, side, side);
                desk.pump();

                opened.toplevel.destroy();
                desk.pump();
                let crate::pane::Remains::Picture(picture) =
                    &left_of(&desk.state, opened.pane).remains
                else {
                    panic!("{late}: a window with a picture fades out from it");
                };
                assert_eq!(
                    picture.dummy_sizes().len(),
                    1,
                    "{late}: the premise: one surface has pixels and the other has none"
                );
                let fills: Vec<Rectangle<i32, smithay::utils::Physical>> = picture
                    .elements((0, 0).into(), 1.0, 1.0)
                    .filter(crate::remains::Surface::filled)
                    .map(|surface| {
                        smithay::backend::renderer::element::Element::geometry(
                            &surface,
                            smithay::utils::Scale::from(1.0),
                        )
                    })
                    .collect();
                assert_eq!(
                    fills,
                    vec![hole],
                    "{late}: the surface that left no pixels is a hole in what fades"
                );
            }
        }

        /// **#126's review: a window whose client keeps its surface and
        /// gives it a new buffer stops fading there and then.** smithay's
        /// GLES renderer uploads a surface's next shared-memory buffer of
        /// the same size into the texture it already has for that surface
        /// -- the one the fade is drawn from -- so a client hiding a window
        /// and showing it again inside the fade by mapping a new role on
        /// the same `wl_surface` would have had the fade show the new
        /// window's pixels. No client was seen to; see
        /// `crate::remains::Picture::holds`. That upload needs a GPU and is
        /// read, not driven; what is pinned here is that the fade has ended
        /// by the time anything could draw the new buffer.
        #[test]
        fn a_window_that_went_ends_its_fade_when_its_surface_is_given_a_new_buffer() {
            use smithay::backend::renderer::element::surface::{
                WaylandSurfaceRenderElement, render_elements_from_surface_tree,
            };
            use smithay::backend::renderer::{Renderer as _, test::DummyRenderer};

            let mut desk = Desk::new();
            desk.state.textures = Some(crate::remains::Textures::Dummy(DummyRenderer.context_id()));
            let opened = desk.open_surface();
            let root = desk
                .state
                .panes
                .get(opened.pane)
                .and_then(Pane::client)
                .and_then(|window| window.wl_surface().map(std::borrow::Cow::into_owned))
                .expect("the window's surface");
            drop(render_elements_from_surface_tree::<
                DummyRenderer,
                WaylandSurfaceRenderElement<DummyRenderer>,
            >(
                &mut DummyRenderer,
                &root,
                (0, 0),
                1.0,
                1.0,
                smithay::backend::renderer::element::Kind::Unspecified,
            ));

            opened.toplevel.destroy();
            opened.xdg.destroy();
            desk.pump();
            assert!(
                matches!(
                    left_of(&desk.state, opened.pane).remains,
                    crate::pane::Remains::Picture(_)
                ),
                "the premise: the window is fading out from its picture"
            );

            // The same surface, a new window: what a hide and a show is.
            let wm_base = desk.client.wm_base.clone().expect("xdg_wm_base bound");
            let xdg = wm_base.get_xdg_surface(&opened.surface, &desk.qh, ());
            let _toplevel = xdg.get_toplevel(&desk.qh, ());
            opened.surface.commit();
            desk.pump();
            assert!(
                desk.state.panes.get(opened.pane).is_some_and(Pane::ghost),
                "a commit with no buffer in it touches no texture, and the fade goes on"
            );
            commit_buffer(&desk.client, &desk.qh, &opened.surface, 64, 64);
            desk.pump();
            assert!(
                desk.state.panes.get(opened.pane).is_none(),
                "the window that went is still fading from the texture its surface's new \
                 buffer is about to be uploaded into"
            );
        }

        /// Three windows of 400x300, mapped overlapping at `(100, 100)`,
        /// `(200, 200)` and `(300, 300)`, bottom to top, with no layout.
        fn overlapping(desk: &mut Desk) -> [Opened; 3] {
            let opened = [
                desk.open_surface(),
                desk.open_surface(),
                desk.open_surface(),
            ];
            for (each, at) in opened.iter().zip([(100, 100), (200, 200), (300, 300)]) {
                commit_buffer(&desk.client, &desk.qh, &each.surface, 400, 300);
                desk.pump();
                let window = desk
                    .state
                    .panes
                    .get(each.pane)
                    .and_then(Pane::client)
                    .cloned()
                    .expect("a window");
                desk.state.map_stacked(window, at, false);
            }
            land(desk);
            opened
        }

        /// Every pane, topmost first, after the frame's `sync_panes`.
        fn stacked(state: &mut Solium) -> Vec<crate::pane::PaneId> {
            state.space.refresh();
            state.sync_panes();
            state
                .on_screen()
                .into_iter()
                .map(|(pane, _)| pane)
                .collect()
        }

        /// **#126's review: a window that closes itself behind another one
        /// fades behind it**, where it was in the stack, and not in front
        /// of the window that was covering it.
        ///
        /// No layout, which is the default, so nothing grows into its
        /// space: a terminal behind a browser whose shell exits, a dialog
        /// behind its window finishing. #126 put every window that went on
        /// top of the stack to fade, so its last picture jumped in front
        /// of the window over it and faded there, where before it had
        /// vanished unseen.
        #[test]
        fn a_window_that_closes_itself_behind_another_fades_behind_it() {
            let mut desk = Desk::new();
            let [bottom, middle, top] = overlapping(&mut desk);
            assert_eq!(
                stacked(&mut desk.state),
                vec![top.pane, middle.pane, bottom.pane],
                "the premise: three windows, stacked as they were mapped"
            );

            middle.toplevel.destroy();
            desk.pump();
            assert!(
                desk.state.panes.get(middle.pane).is_some_and(Pane::ghost),
                "the premise: the middle window is fading out"
            );
            for sweep in 0..2 {
                assert_eq!(
                    stacked(&mut desk.state),
                    vec![top.pane, middle.pane, bottom.pane],
                    "sweep {sweep}: the window that went fades where it was in the stack"
                );
            }
        }

        /// **#126's review, the other half: a window that goes from the
        /// top stays over the window the keyboard moves to.** Focusing that
        /// window raises it, which is why a window fading out is kept over
        /// the panes it was over rather than at a place in the list: kept
        /// at a place, it would go behind the first window focused after
        /// it, and a terminal closed with `Ctrl+D` over another window
        /// would vanish where the two overlap, as it did before #126.
        #[test]
        fn a_window_that_goes_from_the_top_stays_over_the_window_the_keyboard_moves_to() {
            let mut desk = Desk::new();
            let [bottom, middle, top] = overlapping(&mut desk);
            let window = desk
                .state
                .panes
                .get(top.pane)
                .and_then(Pane::client)
                .cloned()
                .expect("a window");
            desk.state
                .focus_window(&window, smithay::utils::SERIAL_COUNTER.next_serial());
            // Somewhere the pointer is over the bottom window alone, so the
            // keyboard goes to the one furthest down, and has the most to
            // be raised past.
            point_at(&mut desk.state, (150.0, 150.0).into());

            top.toplevel.destroy();
            desk.pump();
            let order = stacked(&mut desk.state);
            assert_eq!(
                desk.state
                    .focused_window()
                    .and_then(|window| desk.state.panes.id_of(&window)),
                Some(bottom.pane),
                "the premise: the keyboard went to the window under the pointer"
            );
            assert_eq!(
                stacked(&mut desk.state),
                vec![top.pane, bottom.pane, middle.pane],
                "the window focused is raised, and the window that went stays over it: \
                 the sweep before this one had {order:?}"
            );
        }

        /// **#126: nothing is kept of a window that is not leaving.**
        ///
        /// The picture is taken at the moment a client goes, and never
        /// before: a window that stays holds nothing extra, however many
        /// frames it draws. `remains::taken` counts pictures taken on this
        /// thread, which is the test's own.
        #[test]
        fn nothing_is_kept_of_a_window_that_stays() {
            use smithay::backend::renderer::{Renderer as _, test::DummyRenderer};

            let mut desk = Desk::new();
            desk.state.textures = Some(crate::remains::Textures::Dummy(DummyRenderer.context_id()));
            let staying = desk.open_surface();
            let going = desk.open_surface();
            let taken = crate::remains::taken();
            for size in 60..90 {
                for opened in [&staying, &going] {
                    commit_buffer(&desk.client, &desk.qh, &opened.surface, size, size);
                }
                desk.pump();
                let now = desk.state.clock.now();
                desk.state.settle(now);
                desk.state.space.refresh();
                desk.state.sync_panes();
            }
            assert_eq!(
                crate::remains::taken(),
                taken,
                "a picture was taken of a window nobody closed"
            );
            assert!(desk.state.panes.iter().all(|pane| pane.left().is_none()));

            going.toplevel.destroy();
            desk.pump();
            assert_eq!(
                crate::remains::taken(),
                taken + 1,
                "one, of the window that went"
            );
            assert!(
                desk.state.panes.get(going.pane).is_some_and(Pane::ghost)
                    && desk
                        .state
                        .panes
                        .get(staying.pane)
                        .is_some_and(|pane| pane.left().is_none()),
                "and only the window that went keeps anything"
            );
        }

        /// **#126: a window whose application never arrived is told gone
        /// while its pane is still here**, as every window is: its row is
        /// in `close`'s own snapshot, leaving. It used to be removed first,
        /// so `close` was the one event about a window that was not in it.
        ///
        /// A pane with no scene -- building one needs Qt, which a test
        /// holding a Wayland client cannot have -- so there is nothing of
        /// it on screen to fade, and it goes at once.
        #[test]
        fn a_window_whose_application_never_came_is_told_gone_while_it_is_here() {
            let mut desk = Desk::new();
            desk.install(RECORDER);
            let area = Rectangle::new((100, 100).into(), (400, 300).into());
            let now = desk.state.clock.now();
            let pane = desk.state.panes.open(Pane::loading(
                "never",
                None,
                area,
                std::path::PathBuf::new(),
                None,
                now,
            ));
            desk.state.trigger_open(pane);
            let patience = desk.state.loading.patience;
            desk.state.settle(now + patience);
            let id = pane.get();
            assert_eq!(desk.events(), format!("open {id},close {id}*"));
            assert!(
                desk.state.panes.get(pane).is_none(),
                "with nothing on screen to fade, it goes at once"
            );
        }

        /// **#134 review: a window opened onto a workspace nobody is looking
        /// at took the keyboard with it.**
        ///
        /// Inside `reflow_on_close` for [`Desk`], the one fixture with the
        /// shipped scripts and a real client, and this needs both: the
        /// scripts to decide where a window goes, and the client to say
        /// where a key lands.
        ///
        /// With `follow_overflow = false` a window with no room is placed on
        /// the next empty workspace, a screen away, and the view stays.
        /// `new_toplevel` gave every new toplevel the keyboard -- before any
        /// layout had said where it goes, or, for a window launched with
        /// `sol.spawn`, after the layout had already put it there -- and
        /// `scrolling.lua`, while not in charge, focused it again. Every
        /// key typed after that went to a window the user could not see,
        /// until they clicked. That is #127's fault by another door.
        ///
        /// **The second round: every way a new window gets the keyboard.**
        /// Closing those two doors closed a third that nobody knew was one:
        /// the strip's stray focus was the only way an X11 window ever got
        /// the keyboard as it opened. And a fourth stayed open: a launched
        /// application activating with the token `sol.spawn` gave it, which
        /// the first round's client never sent. So the client here sends it,
        /// and each way in is pinned in each mode it can happen in.
        mod keyboard_at_open {
            use super::*;

            /// The shipped layouts, as `init.lua` requires them, at a
            /// minimum one window fills: the desk's one tile at the shipped
            /// gap is 1896x1056, and neither half of it -- 942 across, 522
            /// down -- is 1000x600. With [`SHIPPED_HEARING_FOCUS`]'s
            /// recorder, which is those same layouts.
            fn one_window_fills_a_workspace(follow: bool) -> String {
                format!(
                    "local config = require(\"config\")\n\
                     config.tiling.minimum = {{ w = 1000, h = 600 }}\n\
                     config.tiling.follow_overflow = {follow}\n\
                     {SHIPPED_HEARING_FOCUS}"
                )
            }

            /// Tiling in charge and one window open, with the keyboard,
            /// typing reaching it, and every animation landed.
            fn working_in_one(follow: bool) -> (Desk, Window) {
                working_in(&one_window_fills_a_workspace(follow), Some("super+t"))
            }

            /// [`working_in_one`] for any scripts, in the mode `key`
            /// switches on -- floating, the startup default, for `None`.
            fn working_in(scripts: &str, key: Option<&str>) -> (Desk, Window) {
                let mut desk = Desk::new();
                desk.install(scripts);
                if let Some(key) = key {
                    assert!(desk.state.trigger(key), "{key} was not handled");
                }
                let (working, _, _) = desk.open();
                // Twice more, for the reason `typing_after_a_close...`
                // gives: the keyboard is a request the client makes in
                // answer to the seat's capabilities.
                desk.pump();
                desk.pump();
                assert!(
                    desk.client.keyboard.is_some(),
                    "the client bound a keyboard; without one this test cannot \
                     observe anything"
                );
                desk.state.clock.advance(Duration::from_secs(1));
                let now = desk.state.clock.now();
                desk.state.settle(now);
                assert_eq!(
                    desk.state.focused_window(),
                    Some(working.clone()),
                    "the premise: the window the user is working in has the keyboard"
                );
                typed_into(&mut desk, &working, "the premise: typing reaches it");
                (desk, working)
            }

            /// One key, and where it landed, as the client saw it.
            fn typed_into(desk: &mut Desk, window: &Window, says: &str) {
                types(&mut desk.state, KEY_A);
                desk.pump();
                assert_eq!(
                    desk.client.typed.last(),
                    Some(&(Some(surface_id(window)), KEY_A)),
                    "{says}"
                );
            }

            /// Whether a pane is headed somewhere the user can see it, as
            /// the focus rules ask.
            fn headed_on_stage(state: &Solium, pane: crate::pane::PaneId) -> bool {
                state.pane_on_stage(pane)
            }

            /// What the scripts say, for a premise.
            fn says(desk: &Desk, chunk: &str) -> String {
                desk.state
                    .scripts
                    .as_ref()
                    .map(|scripts| scripts.evaluate(chunk))
                    .unwrap_or_default()
            }

            fn workspace_of(desk: &Desk, pane: crate::pane::PaneId) -> String {
                says(
                    desk,
                    &format!(
                        "return tostring(require(\"workspaces\").of[{}])",
                        pane.get()
                    ),
                )
            }

            fn showing(desk: &Desk) -> String {
                says(
                    desk,
                    "return tostring(require(\"workspaces\").on(\"reflow-test\"))",
                )
            }

            /// What `sol.spawn` does before it forks, for a program whose
            /// process is this one -- so the client this fixture connects
            /// is the application it asked for. `open_loading` rather than
            /// `begin_loading`, which would build the loading scene in Qt:
            /// see `Solium::open_loading`.
            fn asked_for(desk: &mut Desk) -> crate::pane::PaneId {
                let source = crate::pane::loading_source(None);
                desk.state
                    .open_loading("app", Some(std::process::id()), source, None)
            }

            /// The application asked for arrives: a toplevel and its first
            /// frame, in one dispatch.
            fn arrives(desk: &mut Desk) -> Window {
                let (window, _toplevel, _surface, _xdg) = open_xdg(
                    &mut desk.display,
                    &mut desk.state,
                    &desk.conn,
                    &desk.client,
                    &desk.qh,
                );
                desk.state.sync_panes();
                desk.pump();
                window
            }

            /// **A window opened by its application, with nowhere on this
            /// workspace to go and `follow_overflow` off, leaves the
            /// keyboard where it was.**
            #[test]
            fn a_window_that_overflows_to_a_hidden_workspace_does_not_take_the_keyboard() {
                let (mut desk, working) = working_in_one(false);
                let (_parked, _, pane) = desk.open();
                assert_eq!(
                    workspace_of(&desk, pane),
                    "2",
                    "the premise: the new window was sent to workspace 2"
                );
                assert_eq!(showing(&desk), "1", "the premise: the view stayed");
                assert!(
                    !headed_on_stage(&desk.state, pane),
                    "the premise: it is parked a screen away"
                );

                assert_eq!(
                    desk.state.focused_window(),
                    Some(working.clone()),
                    "the keyboard went with the window to a workspace nobody is looking at"
                );
                typed_into(
                    &mut desk,
                    &working,
                    "a key typed after the window opened went somewhere else",
                );
            }

            /// **The same for a window launched with `sol.spawn`**, whose
            /// place is decided before its application exists: it is on
            /// workspace 2 already when the application arrives, and the
            /// application arriving is not a reason to take the keyboard
            /// there.
            #[test]
            fn a_launched_window_parked_on_a_hidden_workspace_does_not_take_the_keyboard_when_it_arrives()
             {
                let (mut desk, working) = working_in_one(false);
                let pane = asked_for(&mut desk);
                assert_eq!(
                    workspace_of(&desk, pane),
                    "2",
                    "the premise: the launched window was sent to workspace 2"
                );
                assert!(
                    !headed_on_stage(&desk.state, pane),
                    "the premise: it is parked a screen away"
                );

                let arrived = arrives(&mut desk);
                assert_eq!(
                    desk.state.panes.id_of(&arrived),
                    Some(pane),
                    "the premise: the application arrived in the window opened for it"
                );
                assert_eq!(
                    desk.state.focused_window(),
                    Some(working.clone()),
                    "the application arriving took the keyboard to a workspace nobody is \
                     looking at"
                );
                typed_into(
                    &mut desk,
                    &working,
                    "a key typed after the application arrived went somewhere else",
                );
            }

            /// **With `follow_overflow` on, a launched window that
            /// overflows takes the keyboard when its application arrives.**
            ///
            /// The view goes to workspace 2 at the launch and `tiling.lua`
            /// focuses the window by name, but `open` for a launched window
            /// fires before its application exists and a window with no
            /// client cannot be given the keyboard (`window_by_id`), so that
            /// request does nothing. Until the application arrives the
            /// keyboard stays on the window it was on, now a screen away --
            /// where `super+2` onto an empty workspace leaves it as well. That
            /// gap is asserted below as it stands, because `tiling.lua` and
            /// `config.lua` describe it; closing it is a rule about a desk
            /// carried away taking the keyboard with it, which is `go`'s case
            /// as much as this one's.
            ///
            /// The half this pins is the arrival. The keyboard used to be
            /// given in `new_toplevel` and is given at the window's first
            /// frame now, when it is known to be on screen; that move is
            /// what could have lost it.
            #[test]
            fn a_launched_window_that_overflows_with_the_view_takes_the_keyboard_when_it_arrives() {
                let (mut desk, working) = working_in_one(true);
                let pane = asked_for(&mut desk);
                assert_eq!(
                    workspace_of(&desk, pane),
                    "2",
                    "the premise: the launched window was sent to workspace 2"
                );
                assert_eq!(showing(&desk), "2", "the premise: the view went with it");
                assert!(
                    headed_on_stage(&desk.state, pane),
                    "the premise: it is headed on screen"
                );
                let left = desk
                    .state
                    .panes
                    .id_of(&working)
                    .expect("an open window has a pane");
                assert!(
                    desk.state.focused_window() == Some(working.clone())
                        && !headed_on_stage(&desk.state, left),
                    "the gap as it stands: until the application arrives the keyboard is \
                     on the window the view has just left"
                );

                let arrived = arrives(&mut desk);
                assert_eq!(desk.state.panes.id_of(&arrived), Some(pane));
                assert_eq!(
                    desk.state.focused_window(),
                    Some(arrived.clone()),
                    "the application arrived in a window on screen and was not given the \
                     keyboard"
                );
                typed_into(
                    &mut desk,
                    &arrived,
                    "a key typed after the application arrived did not reach it",
                );
            }

            /// **A script that moves the keyboard as a window opens has the
            /// last word.** While `new_toplevel` gave the keyboard, it did so
            /// before `open` and a script's `sol.focus` there came after it.
            /// The keyboard is given after `open` now, and must not take a
            /// script's choice back: this one keeps it on the window that was
            /// already open.
            #[test]
            fn a_script_that_moves_the_keyboard_at_open_has_the_last_word() {
                let mut desk = Desk::new();
                desk.install(
                    "sol.on(\"open\", function(id)\n\
                         for _, window in ipairs(sol.windows()) do\n\
                             if window.id ~= id then\n\
                                 sol.focus(window.id)\n\
                                 return\n\
                             end\n\
                         end\n\
                     end)",
                );
                let (first, _, _) = desk.open();
                desk.pump();
                desk.pump();
                assert_eq!(
                    desk.state.focused_window(),
                    Some(first.clone()),
                    "the premise: with nothing else open and no script moving it, the new \
                     window has the keyboard"
                );
                let _second = desk.open();
                assert_eq!(
                    desk.state.focused_window(),
                    Some(first.clone()),
                    "the script put the keyboard back on the first window and the compositor \
                     took it away again"
                );
                typed_into(&mut desk, &first, "the key went to the new window");
            }

            /// The shipped layouts as `init.lua` requires them, with every
            /// `focus` the scripts hear written down in `heard`.
            const SHIPPED_HEARING_FOCUS: &str = "require(\"modes\")\n\
                 require(\"workspaces\")\n\
                 require(\"tiling\")\n\
                 require(\"scrolling\")\n\
                 heard = {}\n\
                 sol.on(\"focus\", function(id) heard[#heard + 1] = id end)";

            /// The `focus` events the scripts have heard, oldest first.
            fn heard(desk: &Desk) -> String {
                says(desk, "return table.concat(heard, \",\")")
            }

            /// The window in a pane.
            fn window_of(desk: &Desk, pane: crate::pane::PaneId) -> Window {
                desk.state
                    .panes
                    .get(pane)
                    .and_then(Pane::client)
                    .cloned()
                    .expect("the pane has its client")
            }

            /// A second window opens in the mode `key` switches on, and
            /// takes the keyboard as a focus: typing reaches it, and the
            /// scripts hear `focus` for it. A bare grant gives the first two
            /// and not the third.
            fn opening_takes_the_keyboard_as_a_focus(key: Option<&str>, mode: &str) {
                let (mut desk, _working) = working_in(SHIPPED_HEARING_FOCUS, key);
                let (arrived, _, pane) = desk.open();
                assert_eq!(
                    desk.state.focused_window(),
                    Some(arrived.clone()),
                    "{mode}: the new window was not given the keyboard"
                );
                typed_into(
                    &mut desk,
                    &arrived,
                    &format!("{mode}: a key typed after the window opened went somewhere else"),
                );
                let heard = heard(&desk);
                assert_eq!(
                    heard.rsplit(',').next(),
                    Some(pane.get().to_string().as_str()),
                    "{mode}: the new window took the keyboard and the scripts never heard \
                     `focus` for it -- a bare grant rather than a focus. Heard: [{heard}]"
                );
            }

            /// **Floating, the startup default.** Nothing in the scripts
            /// focuses a new window here, so this is `offer_keyboard`'s own
            /// path. Before #134's second review it was a bare
            /// `give_keyboard`: the keyboard, and no `focus`.
            #[test]
            fn a_window_opening_while_floating_takes_the_keyboard_as_a_focus() {
                opening_takes_the_keyboard_as_a_focus(None, "floating");
            }

            /// **Tiling**, which focuses a new window only when the view
            /// goes with it to another workspace -- otherwise the same path
            /// as floating, and the same bare grant before.
            #[test]
            fn a_window_opening_while_tiling_takes_the_keyboard_as_a_focus() {
                opening_takes_the_keyboard_as_a_focus(Some("super+t"), "tiling");
            }

            /// **Scrolling**, whose strip focuses its new column itself, so
            /// `offer_keyboard` stands aside (`Opened::focused`). This held
            /// before as well; it is here so that the three modes are held
            /// to the same answer.
            #[test]
            fn a_window_opening_while_scrolling_takes_the_keyboard_as_a_focus() {
                opening_takes_the_keyboard_as_a_focus(Some("super+s"), "scrolling");
            }

            /// **With `follow_overflow` on, a window its application opened
            /// with no room here takes the keyboard on the workspace the view
            /// went to.** `tiling.lua` focuses it by name; this is the half
            /// of `follow_overflow` its launched twin,
            /// `a_launched_window_that_overflows_with_the_view_takes_the_keyboard_when_it_arrives`,
            /// does not cover.
            #[test]
            fn a_window_that_overflows_with_the_view_takes_the_keyboard() {
                let (mut desk, _working) = working_in_one(true);
                let (arrived, _, pane) = desk.open();
                assert_eq!(
                    workspace_of(&desk, pane),
                    "2",
                    "the premise: the new window was sent to workspace 2"
                );
                assert_eq!(showing(&desk), "2", "the premise: the view went with it");
                assert_eq!(
                    desk.state.focused_window(),
                    Some(arrived.clone()),
                    "the view went with the new window and the keyboard did not"
                );
                typed_into(
                    &mut desk,
                    &arrived,
                    "a key typed after the window opened did not reach it",
                );
            }

            /// **A managed X11 window reaching `offer_keyboard` is focused
            /// exactly as an xdg one is.**
            ///
            /// At the rule, because an `X11Surface` needs a live XWayland and
            /// this binary cannot start one. What the rule's `Focus` leads to
            /// is one line of `offer_keyboard` for both kinds,
            /// `focus_window`, and the xdg tests above are what see that line
            /// taken: a bare grant would give the keyboard without the
            /// `focus` they listen for. Before #134's second review
            /// `offer_keyboard` returned early for any window without an
            /// `xdg_toplevel` -- `ClientKind::X11 => Stay`, in this rule's
            /// terms -- and nothing else gave an X11 window the keyboard as
            /// it opened, once `scrolling.lua` stopped doing it by accident.
            /// That an XWayland application really comes up focused, and
            /// draws itself so, is for a person to check by hand.
            ///
            /// **And the two halves the rule does not see** (#134's third
            /// review): which kind a window with only an X11 role is, through
            /// the same [`ClientKind::from_roles`] that `ClientKind::of`
            /// answers with; and that `offer_keyboard` has no early return
            /// of its own. The second is asked of the source text, for the
            /// reason `render.rs`'s drag-icon test gives: the behaviour
            /// needs an X11 window, and restoring the old `return` passed
            /// every other test. It pins only that `offer_keyboard` hands
            /// the window's kind straight to this rule and returns nowhere
            /// on the way.
            #[test]
            fn a_managed_x11_window_is_offered_the_keyboard_as_an_xdg_one_is() {
                assert_eq!(
                    [
                        ClientKind::from_roles(true, false),
                        ClientKind::from_roles(false, true),
                        ClientKind::from_roles(false, false),
                    ],
                    [Some(ClientKind::Xdg), Some(ClientKind::X11), None],
                    "a window's roles name the wrong kind: an X11 window is one with only an \
                     X11 surface"
                );
                for kind in [ClientKind::Xdg, ClientKind::X11].map(Some) {
                    assert_eq!(
                        first_focus(kind, true, true),
                        FirstFocus::Focus,
                        "{kind:?}: a managed window headed on screen was not focused"
                    );
                    assert_eq!(
                        first_focus(kind, true, false),
                        FirstFocus::Stay,
                        "{kind:?}: a window headed where nobody can see it took the keyboard"
                    );
                    assert_eq!(
                        first_focus(kind, false, true),
                        FirstFocus::Stay,
                        "{kind:?}: a menu, a tooltip or a splash took the keyboard"
                    );
                }
                assert_eq!(
                    first_focus(None, true, true),
                    FirstFocus::Stay,
                    "a window of no kind anybody knows took the keyboard"
                );

                let source = include_str!("open.rs");
                let start = source
                    .find("    fn offer_keyboard(&mut self, window: &Window, pane: crate::pane::PaneId) {")
                    .expect("`offer_keyboard` is still here");
                let body = source[start..]
                    .find("\n    }\n")
                    .map(|end| &source[start..start + end])
                    .expect("and still ends");
                assert!(
                    body.contains("first_focus(")
                        && body.contains("ClientKind::of(window)")
                        && !body.contains("return")
                        && !body.contains("toplevel()"),
                    "`offer_keyboard` decides something before `first_focus` does -- an early \
                     return, or a question about the toplevel -- so an X11 window can be \
                     refused where no test can see it:\n{body}"
                );
            }

            /// When an application sends its activation token, against its
            /// first frame.
            #[derive(Clone, Copy, Debug)]
            enum Sends {
                /// With the toplevel, before it has drawn: what winit does,
                /// and so alacritty.
                BeforeItsFirstFrame,
                /// Once it has drawn.
                AfterItsFirstFrame,
            }

            const BOTH: [Sends; 2] = [Sends::BeforeItsFirstFrame, Sends::AfterItsFirstFrame];

            /// Let a frame go by: the panes reconciled, and a round trip.
            fn frame(desk: &mut Desk) {
                desk.state.sync_panes();
                desk.pump();
            }

            /// A window arrives, and asks to be brought forward with
            /// `token` when `sends` says -- as an application launched with
            /// one does as it opens.
            fn arrives_activating(desk: &mut Desk, token: &str, sends: Sends) -> Window {
                let compositor = desk.client.compositor.clone().expect("wl_compositor bound");
                let wm_base = desk.client.wm_base.clone().expect("xdg_wm_base bound");
                let activation = desk
                    .client
                    .activation
                    .clone()
                    .expect("xdg_activation_v1 bound");
                let before: Vec<Window> = desk.state.space.elements().cloned().collect();
                let surface = compositor.create_surface(&desk.qh, ());
                let xdg = wm_base.get_xdg_surface(&surface, &desk.qh, ());
                let _toplevel = xdg.get_toplevel(&desk.qh, ());
                if matches!(sends, Sends::BeforeItsFirstFrame) {
                    activation.activate(token.to_owned(), &surface);
                }
                commit_buffer(&desk.client, &desk.qh, &surface, 64, 64);
                desk.pump();
                let window = desk
                    .state
                    .space
                    .elements()
                    .find(|window| !before.contains(window))
                    .cloned()
                    .expect("new_toplevel mapped a window");
                frame(desk);
                if matches!(sends, Sends::AfterItsFirstFrame) {
                    activation.activate(token.to_owned(), &surface);
                    frame(desk);
                }
                window
            }

            /// A token nothing was launched with: what a notification, or
            /// another application, hands a window it wants brought forward.
            fn genuine_token(desk: &mut Desk) -> String {
                let (token, _) = desk
                    .state
                    .activation_state
                    .create_external_token(XdgActivationTokenData::default());
                token.as_str().to_owned()
            }

            /// `surface` asks to be brought forward with `token`.
            fn activates(desk: &mut Desk, surface: &wl_surface::WlSurface, token: &str) {
                desk.client
                    .activation
                    .clone()
                    .expect("xdg_activation_v1 bound")
                    .activate(token.to_owned(), surface);
                frame(desk);
            }

            /// **A launched window parked on a hidden workspace does not take
            /// the keyboard by its own token**, sent before its first frame
            /// or after.
            ///
            /// The application is the process `sol.spawn` started, so
            /// `new_toplevel` adopts it by its pid and its window is no
            /// longer loading by the time the token comes back.
            /// `claim_into` asked "still loading?" first, said no, and
            /// `request_activation` took the launch's own token for an
            /// ordinary request and focused the window, on a desk nobody is
            /// looking at. `a_launched_window_parked_on_a_hidden_workspace_does_not_take_the_keyboard_when_it_arrives`
            /// could not see it: its client sends no token.
            #[test]
            fn a_launched_window_parked_on_a_hidden_workspace_does_not_take_the_keyboard_by_its_own_token()
             {
                for sends in BOTH {
                    let (mut desk, working) = working_in_one(false);
                    let pane = asked_for(&mut desk);
                    let token = desk.state.launch_token(pane);
                    assert!(
                        !headed_on_stage(&desk.state, pane),
                        "{sends:?}: the premise: it is parked a screen away"
                    );
                    let arrived = arrives_activating(&mut desk, &token, sends);
                    assert_eq!(
                        desk.state.panes.id_of(&arrived),
                        Some(pane),
                        "{sends:?}: the premise: the application arrived in the window \
                         opened for it"
                    );
                    assert_eq!(
                        desk.state.focused_window(),
                        Some(working.clone()),
                        "{sends:?}: the launch's own token took the keyboard to a workspace \
                         nobody is looking at"
                    );
                    typed_into(
                        &mut desk,
                        &working,
                        &format!("{sends:?}: a key typed after the token went somewhere else"),
                    );
                    // Not even for a moment. Focused and then handed back,
                    // the window would have been told it had the keyboard
                    // and the clipboard, and the scripts that it was
                    // focused, over a token that asks for none of it.
                    let (heard, parked) = (heard(&desk), pane.get().to_string());
                    assert!(
                        !heard.split(',').any(|id| id == parked),
                        "{sends:?}: the launch's own token focused the parked window, if only \
                         until something took the keyboard back. Heard: [{heard}]"
                    );
                }
            }

            /// **The other half: a launched window on screen that activates
            /// with its own token has the keyboard**, whichever side of its
            /// first frame the token comes. The token decides nothing about
            /// the keyboard any more; `offer_keyboard` does, at the first
            /// frame, and this is it saying yes.
            #[test]
            fn a_launched_window_on_screen_takes_the_keyboard_when_it_activates_with_its_own_token()
            {
                for sends in BOTH {
                    let (mut desk, _working) = working_in_one(true);
                    let pane = asked_for(&mut desk);
                    let token = desk.state.launch_token(pane);
                    assert!(
                        headed_on_stage(&desk.state, pane),
                        "{sends:?}: the premise: the view went with it"
                    );
                    let arrived = arrives_activating(&mut desk, &token, sends);
                    assert_eq!(desk.state.panes.id_of(&arrived), Some(pane));
                    assert_eq!(
                        desk.state.focused_window(),
                        Some(arrived.clone()),
                        "{sends:?}: the application arrived on screen and was not given the \
                         keyboard"
                    );
                    let heard = heard(&desk);
                    assert_eq!(
                        heard.rsplit(',').next(),
                        Some(pane.get().to_string().as_str()),
                        "{sends:?}: the application took the keyboard and the scripts never \
                         heard `focus` for it. Heard: [{heard}]"
                    );
                    typed_into(
                        &mut desk,
                        &arrived,
                        &format!("{sends:?}: a key typed after it arrived did not reach it"),
                    );
                }
            }

            /// **A genuine activation of a window on screen takes the
            /// keyboard**, as the protocol is for: the check that follows it
            /// in `request_activation` leaves it alone.
            #[test]
            fn a_genuine_activation_of_a_window_on_screen_takes_the_keyboard() {
                let (mut desk, _working) = working_in(SHIPPED_HEARING_FOCUS, None);
                let asking = desk.open_surface();
                let (other, _, _) = desk.open();
                assert_eq!(
                    desk.state.focused_window(),
                    Some(other),
                    "the premise: the window opened last has the keyboard"
                );
                let token = genuine_token(&mut desk);
                activates(&mut desk, &asking.surface, &token);
                let asking = window_of(&desk, asking.pane);
                assert_eq!(
                    desk.state.focused_window(),
                    Some(asking.clone()),
                    "a window on screen asked to be brought forward and was refused"
                );
                typed_into(&mut desk, &asking, "a key typed after it did not reach it");
            }

            /// **A genuine activation of a window being closed does not
            /// keep the keyboard.** Its own frame is fading to nothing, and
            /// no selection is what keeps it off stage. Until #134's fifth
            /// review it was focused like any other window and then, headed
            /// nowhere the user can see, gave the keyboard back to what is
            /// on screen; it is refused before it is focused now, which
            /// with one other window on screen ends in the same place --
            /// `a_genuine_activation_of_a_window_being_closed_on_the_desk_in_view_leaves_the_keyboard_exactly_where_it_was`
            /// is the one that tells the two apart.
            #[test]
            fn a_genuine_activation_of_a_window_being_closed_does_not_keep_the_keyboard() {
                let (mut desk, working) = working_in(SHIPPED_HEARING_FOCUS, None);
                let closing = desk.open_surface();
                desk.state
                    .focus_window(&working, SERIAL_COUNTER.next_serial());
                typed_into(
                    &mut desk,
                    &working,
                    "the premise: typing reaches the window before",
                );
                desk.state.close_pane(closing.pane);
                assert!(
                    !headed_on_stage(&desk.state, closing.pane)
                        && !desk.state.carried_by_a_selection(closing.pane),
                    "the premise: the window being closed is headed off stage, and not by a \
                     selection"
                );

                let token = genuine_token(&mut desk);
                activates(&mut desk, &closing.surface, &token);
                assert_eq!(
                    desk.state.focused_window(),
                    Some(working.clone()),
                    "a window being closed asked to be brought forward and kept the keyboard"
                );
                typed_into(
                    &mut desk,
                    &working,
                    "a key typed after it went somewhere else",
                );
            }

            /// **A genuine activation of a window its own frame hides on the
            /// desk in view is focused, and does not keep the keyboard.**
            /// No selection hides it and it is not being closed, so nothing
            /// refuses it first: focusing a window can bring it back, as
            /// the strip brings back a column
            /// (`a_genuine_activation_of_a_column_scrolled_off_screen_brings_it_back_with_the_keyboard`).
            /// The check after the focus in `request_activation` is what
            /// gives the keyboard back to what is on screen when nothing
            /// did.
            #[test]
            fn a_genuine_activation_of_a_window_its_own_frame_hides_does_not_keep_the_keyboard() {
                let (mut desk, working) = working_in(SHIPPED_HEARING_FOCUS, None);
                let hidden = desk.open_surface();
                desk.state
                    .focus_window(&working, SERIAL_COUNTER.next_serial());
                typed_into(
                    &mut desk,
                    &working,
                    "the premise: typing reaches the window before",
                );
                // Held at nothing where it is, as `sol.present(id, { opacity
                // = 0 })` holds a window.
                let now = desk.state.clock.now();
                if let Some(pane) = desk.state.panes.get(hidden.pane) {
                    let outer = desk.state.pane_outer(pane);
                    present::present(
                        pane,
                        outer,
                        Frame {
                            opacity: 0.0,
                            ..Frame::real(outer)
                        },
                        now,
                        Duration::ZERO,
                        solium_animation::Curve::Linear,
                    );
                }
                assert!(
                    !headed_on_stage(&desk.state, hidden.pane)
                        && !desk.state.carried_by_a_selection(hidden.pane)
                        && !desk.state.panes.get(hidden.pane).is_some_and(Pane::leaving),
                    "the premise: the window is held invisible by its own frame, and by \\
                         nothing else"
                );

                let before = heard(&desk);
                let token = genuine_token(&mut desk);
                activates(&mut desk, &hidden.surface, &token);
                let after = heard(&desk);
                let since = after.get(before.len()..).unwrap_or_default();
                assert!(
                    since
                        .split(',')
                        .any(|id| id == hidden.pane.get().to_string()),
                    "the premise: it was focused, which is what the check after the focus is \\
                         for. Heard since: [{since}]"
                );
                assert_eq!(
                    desk.state.focused_window(),
                    Some(working.clone()),
                    "a window its own frame hides asked to be brought forward and kept the \\
                         keyboard"
                );
                typed_into(
                    &mut desk,
                    &working,
                    "a key typed after it went somewhere else",
                );
            }

            /// **Any client can mint itself a token, and take the keyboard
            /// with it.** Pinned as it stands, not as it ought to be: this
            /// compositor does not override `token_created`, smithay's
            /// default keeps every token a client asks for -- with no input
            /// serial and no surface, as here -- and `request_activation`
            /// honours whatever token comes back. There is no focus-stealing
            /// prevention yet, and the comment there said a client could not
            /// do this until #134's third review. When there is, this test
            /// is the one that has to change.
            #[test]
            fn a_client_can_mint_itself_an_activation_token_and_take_the_keyboard_with_it() {
                let (mut desk, working) = working_in(SHIPPED_HEARING_FOCUS, None);
                let asking = desk.open_surface();
                desk.state
                    .focus_window(&working, SERIAL_COUNTER.next_serial());
                typed_into(
                    &mut desk,
                    &working,
                    "the premise: typing reaches the window before",
                );

                let activation = desk
                    .client
                    .activation
                    .clone()
                    .expect("xdg_activation_v1 bound");
                // Nothing set on it: no serial from any input event, and no
                // surface asking.
                activation.get_activation_token(&desk.qh, ()).commit();
                frame(&mut desk);
                let token = desk
                    .client
                    .tokens
                    .last()
                    .cloned()
                    .expect("the compositor handed the client the token it asked for");
                activates(&mut desk, &asking.surface, &token);

                let asking = window_of(&desk, asking.pane);
                assert_eq!(
                    desk.state.focused_window(),
                    Some(asking.clone()),
                    "a token a client minted for itself was refused: focus-stealing \
                     prevention exists now, and `request_activation`'s comment and this test \
                     both say otherwise"
                );
                typed_into(&mut desk, &asking, "a key typed after it did not reach it");
            }

            /// **A genuine activation of a window on a workspace nobody is
            /// looking at leaves the keyboard on screen.** Before #134's
            /// second review `focus_window` gave it the keyboard there and
            /// nothing took it back; with `follow_overflow = false` that
            /// window is an ordinary one.
            #[test]
            fn a_genuine_activation_of_a_window_on_a_hidden_workspace_leaves_the_keyboard_on_screen()
             {
                let (mut desk, working) = working_in_one(false);
                let parked = desk.open_surface();
                assert_eq!(
                    workspace_of(&desk, parked.pane),
                    "2",
                    "the premise: the new window was sent to workspace 2"
                );
                assert!(
                    !headed_on_stage(&desk.state, parked.pane),
                    "the premise: it is parked a screen away"
                );
                let token = genuine_token(&mut desk);
                activates(&mut desk, &parked.surface, &token);
                assert_eq!(
                    desk.state.focused_window(),
                    Some(working.clone()),
                    "an activation took the keyboard to a workspace nobody is looking at"
                );
                typed_into(
                    &mut desk,
                    &working,
                    "a key typed after the activation went somewhere else",
                );
            }

            /// **A genuine activation of a column scrolled off the screen
            /// brings it back, keyboard and all** -- which is why
            /// `request_activation` asks whether the window is on screen
            /// after focusing it and not before: the strip scrolls a focused
            /// column into view, so the column is off screen only until the
            /// focus it asked for. Asking first refused it.
            #[test]
            fn a_genuine_activation_of_a_column_scrolled_off_screen_brings_it_back_with_the_keyboard()
             {
                let (mut desk, _working) = working_in(SHIPPED_HEARING_FOCUS, Some("super+s"));
                let first = desk.open_surface();
                for _ in 0..4 {
                    let _ = desk.open();
                }
                desk.state.clock.advance(Duration::from_secs(1));
                let now = desk.state.clock.now();
                desk.state.settle(now);
                assert!(
                    !headed_on_stage(&desk.state, first.pane),
                    "the premise: the strip scrolled the column off screen"
                );
                let token = genuine_token(&mut desk);
                activates(&mut desk, &first.surface, &token);
                let first_window = window_of(&desk, first.pane);
                assert_eq!(
                    desk.state.focused_window(),
                    Some(first_window.clone()),
                    "a column off the edge asked to be brought forward and was refused"
                );
                assert!(
                    headed_on_stage(&desk.state, first.pane),
                    "the strip did not bring the column it focused back into view"
                );
                typed_into(
                    &mut desk,
                    &first_window,
                    "a key typed after the activation did not reach the column",
                );
            }

            /// Scrolling, one monitor: five windows on workspace 1, so the
            /// strip has scrolled `off` -- the second of them -- off the
            /// screen; then workspace 2, with two columns side by side on it,
            /// the keyboard on `left` and the pointer resting on `right`.
            /// `shut` is another of workspace 1's. Every animation landed.
            ///
            /// The pointer is on the other tile so that `settle_focus` --
            /// which a window focused and handed back ends in -- picks a
            /// window other than the one that had the keyboard.
            struct LeftBehind {
                desk: Desk,
                off: Opened,
                shut: Opened,
                left: Window,
            }

            fn a_strip_left_behind_on_workspace_1() -> LeftBehind {
                let (mut desk, _working) = working_in(SHIPPED_HEARING_FOCUS, Some("super+s"));
                let off = desk.open_surface();
                let shut = desk.open_surface();
                for _ in 0..3 {
                    let _ = desk.open();
                }
                desk.state.clock.advance(Duration::from_secs(1));
                let now = desk.state.clock.now();
                desk.state.settle(now);
                assert!(
                    !headed_on_stage(&desk.state, off.pane),
                    "the premise: the strip scrolled the column off screen"
                );

                assert!(desk.state.trigger("super+2"), "super+2 was not handled");
                desk.state.clock.advance(Duration::from_secs(1));
                let now = desk.state.clock.now();
                desk.state.settle(now);
                frame(&mut desk);
                let left = desk.open_surface();
                let right = desk.open_surface();
                desk.answer(&left);
                desk.answer(&right);
                desk.state.clock.advance(Duration::from_secs(1));
                let now = desk.state.clock.now();
                desk.state.settle(now);
                frame(&mut desk);
                assert_eq!(
                    (
                        showing(&desk),
                        workspace_of(&desk, off.pane),
                        workspace_of(&desk, shut.pane),
                        workspace_of(&desk, left.pane),
                        workspace_of(&desk, right.pane),
                    ),
                    (
                        "2".to_owned(),
                        "1".to_owned(),
                        "1".to_owned(),
                        "2".to_owned(),
                        "2".to_owned()
                    ),
                    "the premise: the view is on workspace 2, `off` and `shut` on 1, the two \
                     columns on 2"
                );
                assert!(
                    headed_on_stage(&desk.state, left.pane)
                        && headed_on_stage(&desk.state, right.pane)
                        && !headed_on_stage(&desk.state, off.pane),
                    "the premise: both of workspace 2's columns are on screen, and `off` is not"
                );

                let over_right = desk.placed(right.pane);
                point_at(
                    &mut desk.state,
                    (
                        f64::from(over_right.loc.x + over_right.size.w / 2),
                        f64::from(over_right.loc.y + over_right.size.h / 2),
                    ),
                );
                let left = window_of(&desk, left.pane);
                desk.state.focus_window(&left, SERIAL_COUNTER.next_serial());
                typed_into(&mut desk, &left, "the premise: typing reaches `left`");
                LeftBehind {
                    desk,
                    off,
                    shut,
                    left,
                }
            }

            /// The keyboard is on `left`, typing reaches it, and since
            /// `before` the scripts have heard no focus for the window that
            /// asked, `asked`.
            fn still_on_the_left(
                desk: &mut Desk,
                left: &Window,
                before: &str,
                asked: u64,
                route: &str,
            ) {
                assert_eq!(
                    desk.state.focused_window(),
                    Some(left.clone()),
                    "{route}: the keyboard moved off the window that had it"
                );
                typed_into(
                    desk,
                    left,
                    &format!("{route}: a key typed afterwards went somewhere else"),
                );
                let after = heard(desk);
                let since = after.get(before.len()..).unwrap_or_default();
                let asked = asked.to_string();
                assert!(
                    !since.split(',').any(|id| id == asked),
                    "{route}: and the window that asked was focused on the way, if only until \
                     the keyboard was handed on. Heard since: [{since}]"
                );
            }

            /// **#134's fourth review, finding 2: a column scrolled off a
            /// hidden workspace is not focused by a genuine activation.**
            /// Its own frame is off stage already, so asking whether a
            /// selection carries that frame off -- as `carried_off_stage`
            /// did -- said no; it was focused, the strip in view does not
            /// hold it and brought nothing back, and `hand_off_keyboard`
            /// gave the keyboard to whatever `settle_focus` picked: the
            /// column under the pointer.
            #[test]
            fn a_genuine_activation_of_a_column_scrolled_off_a_hidden_workspace_leaves_the_keyboard_exactly_where_it_was()
             {
                let LeftBehind {
                    mut desk,
                    off,
                    left,
                    ..
                } = a_strip_left_behind_on_workspace_1();
                let before = heard(&desk);
                let token = genuine_token(&mut desk);
                activates(&mut desk, &off.surface, &token);
                still_on_the_left(
                    &mut desk,
                    &left,
                    &before,
                    off.pane.get(),
                    "a column scrolled off workspace 1 asked to be brought forward",
                );
            }

            /// **The same for a window being closed on a hidden workspace**,
            /// whose own frame is fading to nothing and so was off stage
            /// before any selection was asked about it either.
            #[test]
            fn a_genuine_activation_of_a_window_being_closed_on_a_hidden_workspace_leaves_the_keyboard_exactly_where_it_was()
             {
                let LeftBehind {
                    mut desk,
                    shut,
                    left,
                    ..
                } = a_strip_left_behind_on_workspace_1();
                desk.state.close_pane(shut.pane);
                assert!(
                    desk.state.panes.get(shut.pane).is_some_and(Pane::leaving),
                    "the premise: the window on workspace 1 is being closed"
                );
                typed_into(
                    &mut desk,
                    &left,
                    "the premise: the close left the keyboard on `left`",
                );
                let before = heard(&desk);
                let token = genuine_token(&mut desk);
                activates(&mut desk, &shut.surface, &token);
                still_on_the_left(
                    &mut desk,
                    &left,
                    &before,
                    shut.pane.get(),
                    "a window being closed on workspace 1 asked to be brought forward",
                );
            }

            /// Tiling, one monitor, the desk in view: `left` with the
            /// keyboard, and `right` and `third` tiled beside it, every
            /// animation landed. See [`keyboard_left_pointer_right`] for
            /// where the pointer goes.
            struct ThreeTiles {
                desk: Desk,
                left: Window,
                right: Opened,
                third: Opened,
            }

            fn three_tiles() -> ThreeTiles {
                let (mut desk, left) = working_in(SHIPPED_HEARING_FOCUS, Some("super+t"));
                let right = desk.open_surface();
                let third = desk.open_surface();
                desk.answer(&right);
                desk.answer(&third);
                desk.state.clock.advance(Duration::from_secs(1));
                let now = desk.state.clock.now();
                desk.state.settle(now);
                frame(&mut desk);
                let left_pane = desk
                    .state
                    .panes
                    .id_of(&left)
                    .expect("a client in the space has a pane");
                assert!(
                    [left_pane, right.pane, third.pane]
                        .iter()
                        .all(|pane| headed_on_stage(&desk.state, *pane)
                            && !desk.state.carried_by_a_selection(*pane)),
                    "the premise: three tiles on the desk in view"
                );
                desk.state.focus_window(&left, SERIAL_COUNTER.next_serial());
                ThreeTiles {
                    desk,
                    left,
                    right,
                    third,
                }
            }

            /// The pointer comes to rest on `right`'s tile as the layout
            /// has it now, and the keyboard goes back to `left`.
            ///
            /// On the other tile so that `settle_focus` -- which a window
            /// focused and handed back ends in -- picks a window other than
            /// the one that had the keyboard: asserted, since that is the
            /// whole of what tells "handed off" from "stayed put" here.
            fn keyboard_left_pointer_right(tiles: &mut ThreeTiles) {
                let ThreeTiles {
                    desk, left, right, ..
                } = tiles;
                let over_right = desk.placed(right.pane);
                let at = Point::<f64, Logical>::from((
                    f64::from(over_right.loc.x + over_right.size.w / 2),
                    f64::from(over_right.loc.y + over_right.size.h / 2),
                ));
                point_at(&mut desk.state, (at.x, at.y));
                desk.state.focus_window(left, SERIAL_COUNTER.next_serial());
                typed_into(desk, left, "the premise: typing reaches `left`");
                let right = window_of(desk, right.pane);
                assert_eq!(
                    desk.state
                        .window_under_at(at, desk.state.settling())
                        .map(|(window, _)| window),
                    Some(right),
                    "the premise: `settle_focus`'s pointer arm would pick `right`"
                );
            }

            /// **#134's fifth review, finding 1: a genuine activation of a
            /// window being closed on the desk in view leaves the keyboard
            /// exactly where it was.** Refused before it is focused, as one
            /// on a hidden desk is. Until then it was focused, found to be
            /// headed nowhere, and handed off -- and `settle_focus` gave the
            /// keyboard to the tile under the pointer, which is not the one
            /// that had it.
            #[test]
            fn a_genuine_activation_of_a_window_being_closed_on_the_desk_in_view_leaves_the_keyboard_exactly_where_it_was()
             {
                let mut tiles = three_tiles();
                tiles.desk.state.close_pane(tiles.third.pane);
                assert!(
                    tiles
                        .desk
                        .state
                        .panes
                        .get(tiles.third.pane)
                        .is_some_and(Pane::leaving)
                        && !headed_on_stage(&tiles.desk.state, tiles.third.pane)
                        && !tiles.desk.state.carried_by_a_selection(tiles.third.pane),
                    "the premise: `third` is being closed on the desk in view"
                );
                keyboard_left_pointer_right(&mut tiles);
                let ThreeTiles {
                    mut desk,
                    left,
                    third,
                    ..
                } = tiles;
                let before = heard(&desk);
                let token = genuine_token(&mut desk);
                activates(&mut desk, &third.surface, &token);
                still_on_the_left(
                    &mut desk,
                    &left,
                    &before,
                    third.pane.get(),
                    "a window being closed on the desk in view asked to be brought forward",
                );
            }

            /// A selection of `pane` alone, carried by `to` from now, at
            /// once: what `sol.group` and `sol.present_group` hold for one.
            fn carry(desk: &mut Desk, pane: crate::pane::PaneId, to: crate::group::Shift) {
                let now = desk.state.clock.now();
                let selection = crate::group::Selection {
                    members: vec![crate::group::Member::Window(pane.get())],
                    on: None,
                };
                let _ = desk.state.groups.declare("under-test", selection, now);
                assert!(
                    desk.state.groups.present(
                        "under-test",
                        to,
                        now,
                        Duration::ZERO,
                        solium_animation::Curve::Linear,
                    ),
                    "the selection was declared"
                );
                frame(desk);
            }

            /// **#134's fifth review: a window whose selection fades it to
            /// nothing where it is -- moved nowhere -- is refused before it
            /// is focused**, as a desk carried a screen away is. Until then
            /// only a displacement counted: it was focused, found to be
            /// headed nowhere, and handed off to the tile under the pointer.
            #[test]
            fn a_genuine_activation_of_a_window_a_selection_fades_to_nothing_leaves_the_keyboard_exactly_where_it_was()
             {
                let mut tiles = three_tiles();
                carry(
                    &mut tiles.desk,
                    tiles.third.pane,
                    crate::group::Shift {
                        opacity: 0.0,
                        ..crate::group::Shift::NONE
                    },
                );
                assert!(
                    !headed_on_stage(&tiles.desk.state, tiles.third.pane)
                        && !tiles
                            .desk
                            .state
                            .panes
                            .get(tiles.third.pane)
                            .is_some_and(Pane::leaving),
                    "the premise: `third` is faded to nothing, and not being closed"
                );
                keyboard_left_pointer_right(&mut tiles);
                let ThreeTiles {
                    mut desk,
                    left,
                    third,
                    ..
                } = tiles;
                let before = heard(&desk);
                let token = genuine_token(&mut desk);
                activates(&mut desk, &third.surface, &token);
                still_on_the_left(
                    &mut desk,
                    &left,
                    &before,
                    third.pane.get(),
                    "a window a selection fades to nothing asked to be brought forward",
                );
            }

            /// **#134's fifth review, finding 2: on a spring, a genuine
            /// activation mid-slide of a window on the desk being switched
            /// to takes the keyboard.** The desk is cleared from a screen
            /// away, and a spring's progress at its end is within its
            /// epsilon of 1 and not 1 -- so asked where the desk is at
            /// `settling`, it was still a pixel or two off, a hidden
            /// workspace, until the travel was retired: every window on the
            /// desk being switched to was refused for the whole slide.
            #[test]
            fn on_a_spring_a_genuine_activation_mid_slide_on_the_desk_being_switched_to_takes_the_keyboard()
             {
                let (mut desk, _working) = working_in(
                    &format!(
                        "require(\"config\").workspaces.motion = \
                         {{ duration = 300, easing = \"spring\" }}\n\
                         {SHIPPED_HEARING_FOCUS}"
                    ),
                    Some("super+t"),
                );
                let switch = |desk: &mut Desk, key: &str, showing_now: &str| {
                    assert!(desk.state.trigger(key), "{key} was not handled");
                    desk.state.clock.advance(Duration::from_secs(1));
                    let now = desk.state.clock.now();
                    desk.state.settle(now);
                    frame(desk);
                    assert_eq!(showing(desk), showing_now, "the premise: {key} switched");
                };
                switch(&mut desk, "super+2", "2");
                let one = desk.open_surface();
                let two = desk.open_surface();
                desk.answer(&one);
                desk.answer(&two);
                switch(&mut desk, "super+1", "1");
                assert!(
                    workspace_of(&desk, one.pane) == "2"
                        && workspace_of(&desk, two.pane) == "2"
                        && !headed_on_stage(&desk.state, one.pane),
                    "the premise: two windows parked on workspace 2"
                );

                // And back to it, a tenth of a second into the slide.
                assert!(desk.state.trigger("super+2"), "super+2 was not handled");
                desk.state.clock.advance(Duration::from_millis(100));
                frame(&mut desk);
                let had = desk.focused();
                let asking = if had == one.pane { &two } else { &one };
                let late = desk
                    .state
                    .panes
                    .get(asking.pane)
                    .map(|pane| {
                        let slot = desk.state.pane_outer(pane);
                        desk.state
                            .carried_at(pane, slot, desk.state.settling())
                            .offset()
                    })
                    .unwrap_or_default();
                assert!(
                    late != (0.0, 0.0) && late.0.abs() < 3.0 && late.1 == 0.0,
                    "the premise, and the situation under test: the spring leaves workspace 2 \
                     a pixel or two off at `settling`, and here it is {late:?}"
                );
                assert!(
                    headed_on_stage(&desk.state, asking.pane),
                    "the premise: the window asking is headed on stage"
                );

                let token = genuine_token(&mut desk);
                activates(&mut desk, &asking.surface, &token);
                let asking = window_of(&desk, asking.pane);
                assert_eq!(
                    desk.state.focused_window(),
                    Some(asking.clone()),
                    "a window on the desk being switched to asked to be brought forward mid-slide \
                     and was refused"
                );
                typed_into(&mut desk, &asking, "a key typed after it did not reach it");
            }

            /// How thick the panel across the slide is in
            /// [`under_a_panel`]: more than the tiling gap, so a tile on
            /// the desk carried towards it shows under it.
            const PANEL: i32 = 64;

            /// A panel [`PANEL`] deep across the slide -- along the right
            /// edge of the monitor for a horizontal arrangement, the top
            /// for a vertical one -- holding it as its exclusive zone, the
            /// way [`bar`] makes a top bar.
            fn a_panel_across(
                desk: &mut Desk,
                vertical: bool,
            ) -> (
                wl_surface::WlSurface,
                zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
            ) {
                use zwlr_layer_surface_v1::Anchor;
                let compositor = desk.client.compositor.clone().expect("wl_compositor bound");
                let shell = desk
                    .client
                    .layer_shell
                    .clone()
                    .expect("zwlr_layer_shell_v1 bound");
                let surface = compositor.create_surface(&desk.qh, ());
                let layer = shell.get_layer_surface(
                    &surface,
                    None,
                    zwlr_layer_shell_v1::Layer::Top,
                    "activation-test-panel".to_string(),
                    &desk.qh,
                    (),
                );
                if vertical {
                    layer.set_anchor(Anchor::Top | Anchor::Left | Anchor::Right);
                    layer.set_size(0, PANEL.unsigned_abs());
                } else {
                    layer.set_anchor(Anchor::Right | Anchor::Top | Anchor::Bottom);
                    layer.set_size(PANEL.unsigned_abs(), 0);
                }
                layer.set_exclusive_zone(PANEL);
                surface.commit();
                desk.pump();
                (surface, layer)
            }

            /// Where a pane's selections are headed, as
            /// `carried_by_a_selection` asks.
            fn bound_for(desk: &Desk, pane: crate::pane::PaneId) -> (f64, f64) {
                let monitor = desk.state.groups.names_monitors().then_some("reflow-test");
                desk.state
                    .groups
                    .bound_for_window(pane.get(), monitor)
                    .offset()
            }

            /// Tiling, one monitor, the shipped workspaces at `spread = 1.0`
            /// in a row or a column of three, and a panel across the
            /// slide. `before` is on workspace 1, carried left or up, with
            /// the window the fixture started in; `after` and
            /// `beside_after` are on workspace 3, carried right or down;
            /// and workspace 2 is in view, with the keyboard on `left` and
            /// the pointer resting on `right`, for the reason
            /// [`a_strip_left_behind_on_workspace_1`] gives. Every animation
            /// landed.
            ///
            /// Of the three windows beside, one at least shows under the
            /// panel and one is off the screen, which are the two ways the
            /// regression went.
            struct UnderAPanel {
                desk: Desk,
                before: Opened,
                after: Opened,
                beside_after: Opened,
                left: Window,
                _panel: (
                    wl_surface::WlSurface,
                    zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
                ),
            }

            fn under_a_panel(vertical: bool) -> UnderAPanel {
                let arrangement = if vertical { "vertical" } else { "horizontal" };
                let (mut desk, _working) = working_in(
                    &format!(
                        "local config = require(\"config\")\n\
                         config.workspaces.arrangement = \"{arrangement}\"\n\
                         config.workspaces.columns = 3\n\
                         config.workspaces.rows = 3\n\
                         config.workspaces.spread = 1.0\n\
                         {SHIPPED_HEARING_FOCUS}"
                    ),
                    Some("super+t"),
                );
                let panel = a_panel_across(&mut desk, vertical);
                let output = desk
                    .state
                    .space
                    .outputs()
                    .next()
                    .cloned()
                    .expect("the fixture has a monitor");
                let wanted = if vertical {
                    Rectangle::new((0, PANEL).into(), (1920, 1080 - PANEL).into())
                } else {
                    Rectangle::new((0, 0).into(), (1920 - PANEL, 1080).into())
                };
                assert_eq!(
                    desk.state.work_area_on(&output),
                    Some(wanted),
                    "{arrangement}: the premise: the panel takes its thickness off the work \
                     area, across the slide"
                );

                let settle = |desk: &mut Desk| {
                    desk.state.clock.advance(Duration::from_secs(1));
                    let now = desk.state.clock.now();
                    desk.state.settle(now);
                    frame(desk);
                };
                let before = desk.open_surface();
                desk.answer(&before);
                settle(&mut desk);
                assert!(desk.state.trigger("super+3"), "super+3 was not handled");
                settle(&mut desk);
                let after = desk.open_surface();
                let beside_after = desk.open_surface();
                desk.answer(&after);
                desk.answer(&beside_after);
                settle(&mut desk);
                assert!(desk.state.trigger("super+2"), "super+2 was not handled");
                settle(&mut desk);
                let left = desk.open_surface();
                let right = desk.open_surface();
                desk.answer(&left);
                desk.answer(&right);
                settle(&mut desk);
                assert_eq!(
                    (
                        showing(&desk),
                        workspace_of(&desk, before.pane),
                        workspace_of(&desk, after.pane),
                        workspace_of(&desk, beside_after.pane),
                        workspace_of(&desk, left.pane),
                        workspace_of(&desk, right.pane),
                    ),
                    (
                        "2".to_owned(),
                        "1".to_owned(),
                        "3".to_owned(),
                        "3".to_owned(),
                        "2".to_owned(),
                        "2".to_owned()
                    ),
                    "{arrangement}: the premise: the view on workspace 2, `before` on 1, \
                     `after` and `beside_after` on 3"
                );

                // Parked by the work area, which is the situation under
                // test: the whole screen carried that far still overlaps
                // the screen by the panel's thickness.
                let across = if vertical {
                    (0.0, f64::from(1080 - PANEL))
                } else {
                    (f64::from(1920 - PANEL), 0.0)
                };
                assert_eq!(
                    (bound_for(&desk, before.pane), bound_for(&desk, after.pane)),
                    ((-across.0, -across.1), across),
                    "{arrangement}: the premise: each desk beside is parked a work area away"
                );
                let shows = [&before, &after, &beside_after]
                    .map(|beside| headed_on_stage(&desk.state, beside.pane));
                assert!(
                    shows.contains(&true) && shows.contains(&false),
                    "{arrangement}: the premise: of `before`, `after` and `beside_after`, one \
                     shows under the panel and one is off the screen, and here they are \
                     {shows:?}, living at {:?}",
                    [&before, &after, &beside_after].map(|beside| desk.placed(beside.pane)),
                );
                assert!(
                    headed_on_stage(&desk.state, left.pane)
                        && headed_on_stage(&desk.state, right.pane),
                    "{arrangement}: the premise: both of workspace 2's tiles are on screen"
                );

                let over_right = desk.placed(right.pane);
                point_at(
                    &mut desk.state,
                    (
                        f64::from(over_right.loc.x + over_right.size.w / 2),
                        f64::from(over_right.loc.y + over_right.size.h / 2),
                    ),
                );
                let left = window_of(&desk, left.pane);
                desk.state.focus_window(&left, SERIAL_COUNTER.next_serial());
                typed_into(&mut desk, &left, "the premise: typing reaches `left`");
                UnderAPanel {
                    desk,
                    before,
                    after,
                    beside_after,
                    left,
                    _panel: panel,
                }
            }

            /// A genuine activation of each window beside in turn: each
            /// refused, and the keyboard exactly where it was.
            fn neither_desk_beside_takes_the_keyboard(vertical: bool) {
                let arrangement = if vertical { "vertical" } else { "horizontal" };
                let UnderAPanel {
                    mut desk,
                    before,
                    after,
                    beside_after,
                    left,
                    ..
                } = under_a_panel(vertical);
                for (asking, which) in [
                    (&before, "workspace 1"),
                    (&after, "workspace 3"),
                    (&beside_after, "workspace 3"),
                ] {
                    let heard_before = heard(&desk);
                    let token = genuine_token(&mut desk);
                    activates(&mut desk, &asking.surface, &token);
                    still_on_the_left(
                        &mut desk,
                        &left,
                        &heard_before,
                        asking.pane.get(),
                        &format!(
                            "{arrangement}: a window on {which}, parked a work area away, \
                             asked to be brought forward"
                        ),
                    );
                }
            }

            /// **#134's sixth review: a desk parked a work area away is a
            /// hidden workspace, whatever a panel across the slide leaves
            /// of its screen.** Until then `put_away` measured the desk
            /// against its whole screen, and at `spread = 1.0` that still
            /// overlapped the screen by the panel: a window on the desk to
            /// the right showed under the panel and kept the keyboard, and
            /// one on the desk to the left was focused, found off screen,
            /// and handed off to the tile under the pointer.
            #[test]
            fn with_a_panel_across_a_horizontal_slide_a_genuine_activation_on_a_hidden_workspace_leaves_the_keyboard_exactly_where_it_was()
             {
                neither_desk_beside_takes_the_keyboard(false);
            }

            /// **The same in a column of workspaces, under a top bar**: the
            /// desk above shows under the bar, and the one below is off the
            /// bottom of the screen.
            #[test]
            fn with_a_bar_across_a_vertical_slide_a_genuine_activation_on_a_hidden_workspace_leaves_the_keyboard_exactly_where_it_was()
             {
                neither_desk_beside_takes_the_keyboard(true);
            }

            /// **And under the same panel, a genuine activation mid-slide
            /// on the desk being switched to takes the keyboard**, in a row
            /// and in a column: that desk is headed for nothing, however
            /// far away it still is.
            #[test]
            fn with_a_panel_across_the_slide_a_genuine_activation_mid_slide_on_the_desk_being_switched_to_takes_the_keyboard()
             {
                for vertical in [false, true] {
                    let arrangement = if vertical { "vertical" } else { "horizontal" };
                    let UnderAPanel {
                        mut desk,
                        after,
                        beside_after,
                        ..
                    } = under_a_panel(vertical);
                    assert!(desk.state.trigger("super+3"), "super+3 was not handled");
                    desk.state.clock.advance(Duration::from_millis(100));
                    frame(&mut desk);
                    let had = desk.focused();
                    assert_eq!(
                        workspace_of(&desk, had),
                        "3",
                        "{arrangement}: the premise: the switch gave the keyboard to \
                         workspace 3"
                    );
                    let asking = if had == after.pane {
                        &beside_after
                    } else {
                        &after
                    };
                    let now = desk.state.clock.now();
                    let mid = desk
                        .state
                        .panes
                        .get(asking.pane)
                        .map(|pane| {
                            let slot = desk.state.pane_outer(pane);
                            desk.state.carried_at(pane, slot, now).offset()
                        })
                        .unwrap_or_default();
                    assert!(
                        mid != (0.0, 0.0) && bound_for(&desk, asking.pane) == (0.0, 0.0),
                        "{arrangement}: the premise: workspace 3 is mid-slide, {mid:?} away, \
                         and headed for nothing"
                    );

                    let token = genuine_token(&mut desk);
                    activates(&mut desk, &asking.surface, &token);
                    let asking = window_of(&desk, asking.pane);
                    assert_eq!(
                        desk.state.focused_window(),
                        Some(asking.clone()),
                        "{arrangement}: a window on the desk being switched to asked to be \
                         brought forward mid-slide and was refused"
                    );
                    typed_into(
                        &mut desk,
                        &asking,
                        &format!("{arrangement}: a key typed after it did not reach it"),
                    );
                }
            }

            /// **With `follow_overflow = false`, no route hands the keyboard
            /// to a window parked on a hidden workspace**: one opened by its
            /// application, one launched that arrives with no token, with
            /// its own token before its first frame and after, and a genuine
            /// activation of the first of them. Each is parked on a
            /// workspace of its own -- six of them, where the shipped four
            /// would run out and let the fifth in beside the first -- and
            /// the keyboard stays in the window the user is working in
            /// throughout.
            #[test]
            fn with_follow_overflow_off_no_route_hands_the_keyboard_to_a_parked_window() {
                fn still_working(desk: &mut Desk, working: &Window, route: &str) {
                    assert_eq!(
                        desk.state.focused_window(),
                        Some(working.clone()),
                        "{route}: the keyboard went to a workspace nobody is looking at"
                    );
                    typed_into(
                        desk,
                        working,
                        &format!("{route}: a key typed afterwards went somewhere else"),
                    );
                }
                fn parked(desk: &Desk, pane: crate::pane::PaneId, route: &str) {
                    assert!(
                        !headed_on_stage(&desk.state, pane),
                        "{route}: the premise: the window is parked a screen away, and it is \
                         on workspace {} with the view on {}",
                        workspace_of(desk, pane),
                        showing(desk)
                    );
                }

                let (mut desk, working) = working_in(
                    &format!(
                        "require(\"config\").workspaces.columns = 6\n{}",
                        one_window_fills_a_workspace(false)
                    ),
                    Some("super+t"),
                );

                let opened = desk.open_surface();
                parked(&desk, opened.pane, "opened by its application");
                still_working(&mut desk, &working, "opened by its application");

                let pane = asked_for(&mut desk);
                parked(&desk, pane, "launched");
                let _ = arrives(&mut desk);
                still_working(&mut desk, &working, "launched, with no token");

                for sends in BOTH {
                    let route = format!("launched, with its own token {sends:?}");
                    let pane = asked_for(&mut desk);
                    parked(&desk, pane, &route);
                    let token = desk.state.launch_token(pane);
                    let _ = arrives_activating(&mut desk, &token, sends);
                    still_working(&mut desk, &working, &route);
                }

                let token = genuine_token(&mut desk);
                activates(&mut desk, &opened.surface, &token);
                still_working(&mut desk, &working, "a genuine activation");
            }

            /// **#134 third review, finding 2: a genuine activation of a
            /// window on a hidden workspace focused it first, and then gave
            /// the keyboard away** -- to whatever `settle_focus` picked,
            /// which with two tiles on screen need not be the one that had
            /// it.
            ///
            /// Two tiles side by side, at a minimum two fit and three do
            /// not. The keyboard is on the left one; the pointer rests on
            /// the right one, which is what `settle_focus` asks first. The
            /// window asking to be brought forward overflowed to workspace
            /// 2. Focused and then handed off, the keyboard ended on the
            /// tile under the pointer; and the parked window had been told
            /// it had the keyboard, and the scripts that it was focused, on
            /// the way.
            #[test]
            fn a_genuine_activation_of_a_window_on_a_hidden_workspace_leaves_the_keyboard_exactly_where_it_was()
             {
                let (mut desk, _first) = working_in(
                    &format!(
                        "local config = require(\"config\")\n\
                         config.tiling.minimum = {{ w = 900, h = 600 }}\n\
                         config.tiling.follow_overflow = false\n\
                         {SHIPPED_HEARING_FOCUS}"
                    ),
                    Some("super+t"),
                );
                let left = desk.focused();
                let right = desk.open_surface();
                desk.answer(&right);
                let left_window = window_of(&desk, left);
                let right_window = window_of(&desk, right.pane);
                desk.state.clock.advance(Duration::from_secs(1));
                let now = desk.state.clock.now();
                desk.state.settle(now);
                assert!(
                    headed_on_stage(&desk.state, left) && headed_on_stage(&desk.state, right.pane),
                    "the premise: two tiles on screen"
                );

                // The pointer on the right tile, the keyboard on the left.
                let over_right = desk.placed(right.pane);
                point_at(
                    &mut desk.state,
                    (
                        f64::from(over_right.loc.x + over_right.size.w / 2),
                        f64::from(over_right.loc.y + over_right.size.h / 2),
                    ),
                );
                desk.state
                    .focus_window(&left_window, SERIAL_COUNTER.next_serial());
                typed_into(
                    &mut desk,
                    &left_window,
                    "the premise: typing reaches the left tile",
                );

                let parked = desk.open_surface();
                assert_eq!(
                    workspace_of(&desk, parked.pane),
                    "2",
                    "the premise: the third window had no room and went to workspace 2"
                );
                assert!(
                    !headed_on_stage(&desk.state, parked.pane),
                    "the premise: it is parked a screen away"
                );
                assert_eq!(
                    desk.state.focused_window(),
                    Some(left_window.clone()),
                    "the premise: it opened without the keyboard"
                );

                let token = genuine_token(&mut desk);
                activates(&mut desk, &parked.surface, &token);
                assert_eq!(
                    desk.state.focused_window(),
                    Some(left_window.clone()),
                    "an activation of a window nobody can see moved the keyboard off the \
                     left tile -- to the right one (surface {}) if it went where the pointer is",
                    surface_id(&right_window)
                );
                typed_into(
                    &mut desk,
                    &left_window,
                    "a key typed after the activation went somewhere else",
                );
                let (heard, parked) = (heard(&desk), parked.pane.get().to_string());
                assert!(
                    !heard.split(',').any(|id| id == parked),
                    "the parked window was focused on the way, if only until the keyboard \
                     was handed back. Heard: [{heard}]"
                );
            }

            /// **#134 third review, finding 3: a window claimed by its
            /// launch's token into a pane parked on a hidden workspace kept
            /// the keyboard it had.**
            ///
            /// A launcher that forks and exits breaks the pid chain, so its
            /// application opens a window of its own -- on screen, and
            /// given the keyboard there, as any new window is -- and then
            /// activates with the token `sol.spawn` handed it.
            /// `claim_into` moves it into the window that was opened for the
            /// launch, and that one is on workspace 2. It moved, and the
            /// keyboard went with it.
            #[test]
            fn a_focused_window_claimed_into_a_pane_on_a_hidden_workspace_gives_the_keyboard_up() {
                let mut desk = Desk::new();
                desk.install(&one_window_fills_a_workspace(false));
                assert!(desk.state.trigger("super+t"));
                let (_in_the_way, toplevel, _) = desk.open();
                desk.pump();
                desk.pump();
                assert!(
                    desk.client.keyboard.is_some(),
                    "the client bound a keyboard"
                );

                // No pid, so nothing adopts the application by its
                // process: only the token can.
                let source = crate::pane::loading_source(None);
                let launched = desk.state.open_loading("app", None, source, None);
                assert_eq!(
                    workspace_of(&desk, launched),
                    "2",
                    "the premise: the launch had no room and went to workspace 2"
                );
                // And the window that filled workspace 1 goes, so there is
                // room there for the application's own window.
                toplevel.destroy();
                desk.pump();
                frame(&mut desk);

                let own = desk.open_surface();
                let window = window_of(&desk, own.pane);
                assert_ne!(
                    own.pane, launched,
                    "the premise: it opened a window of its own"
                );
                assert_eq!(
                    desk.state.focused_window(),
                    Some(window.clone()),
                    "the premise: that window opened on screen and has the keyboard"
                );

                let token = desk.state.launch_token(launched);
                activates(&mut desk, &own.surface, &token);
                assert_eq!(
                    desk.state.panes.id_of(&window),
                    Some(launched),
                    "the premise: the token moved it into the window opened for the launch"
                );
                assert!(
                    !headed_on_stage(&desk.state, launched),
                    "the premise: and that window is parked a screen away"
                );
                assert_ne!(
                    desk.state.focused_window(),
                    Some(window.clone()),
                    "the keyboard went with the window to a workspace nobody is looking at"
                );
                let before = desk.client.typed.len();
                types(&mut desk.state, KEY_A);
                desk.pump();
                assert!(
                    desk.client.typed[before..]
                        .iter()
                        .all(|(on, _)| *on != Some(surface_id(&window))),
                    "a key typed after the token reached the window nobody can see"
                );
            }

            /// **Two monitors side by side, #134's third review, finding
            /// 1.** Everything above uses one monitor, where a workspace
            /// parked a screen away is parked on no screen at all. With the
            /// shipped `per_monitor`, `horizontal` and `spread = 1.06`, the
            /// left monitor's workspace 2 is carried 2035 pixels right --
            /// onto the right monitor, which the renderer never draws it on,
            /// because a pane is drawn only on the monitors its slot is on.
            /// Every "is this on screen" question answered against every
            /// screen instead, and said yes.
            ///
            /// The desk: tiling, one window fills a workspace, overflow does
            /// not follow. `working` on the left monitor with the keyboard,
            /// `right` on the right monitor, and `parked` opened after both
            /// with no room on the left, so it is on the left monitor's
            /// workspace 2 and drawn over `right`. Every client has answered
            /// its tile, so each draws the whole of it, and the animations
            /// have landed.
            struct SideBySide {
                desk: Desk,
                working: Opened,
                right: Opened,
                parked: Opened,
            }

            /// The right monitor's rectangle.
            fn right_screen() -> Rectangle<i32, Logical> {
                Rectangle::new((1920, 0).into(), (1920, 1080).into())
            }

            fn side_by_side_with_a_parked_window() -> SideBySide {
                side_by_side_with_a_parked_window_and("")
            }

            /// [`side_by_side_with_a_parked_window`], with `extra` loaded
            /// after the shipped layouts -- `overview`, which `init.lua`
            /// requires and the recorder here does not.
            fn side_by_side_with_a_parked_window_and(extra: &str) -> SideBySide {
                let mut desk = Desk::side_by_side();
                desk.install(&format!("{}\n{extra}", one_window_fills_a_workspace(false)));
                // Floating first, to put a window on the right monitor by
                // hand: every new window maps at the origin, which is the
                // left one, and tiling then adopts it where it is.
                let right = desk.open_surface();
                desk.state
                    .map_stacked(window_of(&desk, right.pane), (2020, 100), false);
                frame(&mut desk);
                assert!(desk.state.trigger("super+t"), "super+t was not handled");
                let working = desk.open_surface();
                desk.pump();
                desk.pump();
                assert!(
                    desk.client.keyboard.is_some(),
                    "the client bound a keyboard"
                );
                desk.answer(&right);
                desk.answer(&working);
                let parked = desk.open_surface();
                desk.answer(&parked);
                desk.state.clock.advance(Duration::from_secs(1));
                let now = desk.state.clock.now();
                desk.state.settle(now);
                frame(&mut desk);

                let screen = right_screen();
                assert!(
                    screen.contains_rect(desk.placed(right.pane))
                        && !screen.overlaps(desk.placed(working.pane)),
                    "the premise: `right` is tiled on the right monitor and `working` on \
                     the left, at {:?} and {:?}",
                    desk.placed(right.pane),
                    desk.placed(working.pane)
                );
                assert_eq!(
                    (
                        workspace_of(&desk, parked.pane),
                        showing(&desk),
                        says(
                            &desk,
                            &format!(
                                "return tostring(require(\"workspaces\").on({RIGHT_SCREEN:?}))"
                            )
                        )
                    ),
                    ("2".to_owned(), "1".to_owned(), "1".to_owned()),
                    "the premise: `parked` is on workspace 2 and both monitors show 1"
                );
                let drawn = drawn_now(&desk.state, parked.pane, desk.state.settling()).rect;
                assert!(
                    !screen.overlaps(desk.placed(parked.pane)) && screen.to_f64().overlaps(drawn),
                    "the premise, and the situation under test: `parked` lives on the left \
                     monitor at {:?} and is carried onto the right one, to {drawn:?}",
                    desk.placed(parked.pane)
                );
                SideBySide {
                    desk,
                    working,
                    right,
                    parked,
                }
            }

            /// The pointer, moved to `at` as a device would move it.
            fn point_at(state: &mut Solium, at: (f64, f64)) {
                let pointer = state.seat.get_pointer().expect(
                    "the fixture's seat has a pointer; without one there is no pointer \
                     arm to test",
                );
                pointer.motion(
                    state,
                    None,
                    &smithay::input::pointer::MotionEvent {
                        location: at.into(),
                        serial: SERIAL_COUNTER.next_serial(),
                        time: 0,
                    },
                );
                pointer.frame(state);
            }

            /// The user goes to the left monitor's workspace 2, works in
            /// the parked window there, and comes back to workspace 1 --
            /// which leaves the parked window stacked above `right`, as the
            /// window last raised on that monitor before the switch away.
            /// What a walk topmost-first meets before the window the right
            /// monitor draws.
            fn visit_the_parked_window(side: &mut SideBySide) {
                let desk = &mut side.desk;
                for (key, showing_now) in [("super+2", "2"), ("super+1", "1")] {
                    assert!(desk.state.trigger(key), "{key} was not handled");
                    desk.state.clock.advance(Duration::from_secs(1));
                    let now = desk.state.clock.now();
                    desk.state.settle(now);
                    frame(desk);
                    assert_eq!(showing(desk), showing_now, "the premise: {key} switched");
                }
                let working = window_of(desk, side.working.pane);
                assert!(
                    heard(desk)
                        .split(',')
                        .any(|id| id == side.parked.pane.get().to_string())
                        && desk.state.focused_window() == Some(working),
                    "the premise: the parked window was worked in on workspace 2, and the \
                     keyboard came back to `working` with the view"
                );
                assert!(
                    above(&mut desk.state, side.parked.pane, side.right.pane),
                    "the premise: the parked window is stacked above the right one, so a \
                     walk topmost-first meets it first"
                );
            }

            /// The first thing any question about visibility has to get
            /// right: `parked` is not on stage, and the windows that are
            /// drawn are. And idle inhibition, which asks the same question
            /// of the present: a video on the left monitor's hidden
            /// workspace does not hold the machine awake from the right.
            #[test]
            fn on_two_monitors_a_window_on_the_left_monitors_hidden_workspace_is_not_on_stage() {
                let SideBySide {
                    desk,
                    working,
                    right,
                    parked,
                } = side_by_side_with_a_parked_window();
                let surface = |pane| {
                    window_of(&desk, pane)
                        .wl_surface()
                        .map(std::borrow::Cow::into_owned)
                        .expect("a mapped client has a surface")
                };
                // Every answer asked before any is asserted, so a failure
                // says which question is wrong rather than only the first.
                let on_stage = |pane| headed_on_stage(&desk.state, pane);
                let visible = |pane| desk.state.surface_is_visible(&surface(pane));
                assert_eq!(
                    (
                        on_stage(parked.pane),
                        on_stage(working.pane),
                        on_stage(right.pane),
                        visible(parked.pane),
                        visible(right.pane),
                    ),
                    (false, true, true, false, true),
                    "(parked on stage, working on stage, right on stage, parked visible to \
                     idle inhibition, right visible to it): a window on the left monitor's \
                     hidden workspace counted as on screen because it is carried over the \
                     right monitor, which never draws it -- or a window that is drawn did not"
                );
            }

            /// **`offer_keyboard`**: the window that opened onto the left
            /// monitor's hidden workspace did not take the keyboard.
            #[test]
            fn on_two_monitors_a_window_opening_onto_the_left_monitors_hidden_workspace_does_not_take_the_keyboard()
             {
                let SideBySide {
                    mut desk,
                    working,
                    parked,
                    ..
                } = side_by_side_with_a_parked_window();
                let working = window_of(&desk, working.pane);
                assert_eq!(
                    desk.state.focused_window(),
                    Some(working.clone()),
                    "the window that opened onto the left monitor's hidden workspace took \
                     the keyboard"
                );
                typed_into(
                    &mut desk,
                    &working,
                    "a key typed after it opened went somewhere else",
                );
                let (heard, parked) = (heard(&desk), parked.pane.get().to_string());
                assert!(
                    !heard.split(',').any(|id| id == parked),
                    "and the scripts heard it focused. Heard: [{heard}]"
                );
            }

            /// **`settle_focus`'s topmost arm**: the last window on the
            /// left monitor closes, with the pointer over nothing, and the
            /// keyboard goes to the window on the right monitor -- not to
            /// the parked one, which is stacked above it. Live on `stage`
            /// before #134, whenever the last window on a monitor closed.
            #[test]
            fn on_two_monitors_closing_the_last_window_on_the_left_hands_the_keyboard_to_the_right_monitor()
             {
                let mut side = side_by_side_with_a_parked_window();
                visit_the_parked_window(&mut side);
                let SideBySide {
                    mut desk,
                    working,
                    right,
                    parked,
                } = side;
                let right_window = window_of(&desk, right.pane);
                desk.state.close_pane(working.pane);
                desk.ask();
                assert_eq!(
                    desk.state.focused_window(),
                    Some(right_window.clone()),
                    "closing the last window on the left monitor handed the keyboard to the \
                     left monitor's hidden workspace (surface {}) rather than to the window \
                     on the right monitor",
                    surface_id(&window_of(&desk, parked.pane))
                );
                typed_into(
                    &mut desk,
                    &right_window,
                    "a key typed after the close went somewhere else",
                );
            }

            /// **`settle_focus`'s pointer arm**: a pointer resting on the
            /// right monitor, at a pixel the parked window is carried over,
            /// hands the keyboard to the window the right monitor draws
            /// there.
            #[test]
            fn on_two_monitors_a_pointer_on_the_right_monitor_does_not_settle_the_keyboard_on_the_left_monitors_hidden_workspace()
             {
                let mut side = side_by_side_with_a_parked_window();
                visit_the_parked_window(&mut side);
                let SideBySide {
                    mut desk, right, ..
                } = side;
                let right_window = window_of(&desk, right.pane);
                point_at(&mut desk.state, (2500.0, 500.0));
                desk.state.give_keyboard(None, SERIAL_COUNTER.next_serial());
                desk.state.settle_focus();
                assert_eq!(
                    desk.state.focused_window(),
                    Some(right_window.clone()),
                    "a pointer resting on the right monitor handed the keyboard to the \
                     window carried over it from the left monitor's hidden workspace"
                );
                typed_into(
                    &mut desk,
                    &right_window,
                    "a key typed afterwards went somewhere else",
                );
            }

            /// **The hit tests**: a press on the right monitor, at a pixel
            /// the parked window is carried over, reaches the window the
            /// right monitor draws there -- `window_under` for
            /// click-to-focus, `surface_under` for the event itself, and
            /// `chrome_under` for the parked window's edge, which is there
            /// only in arithmetic. Otherwise it is an input black hole: a
            /// press lands in a window nobody can see, and focus follows it.
            #[test]
            fn on_two_monitors_a_press_on_the_right_monitor_reaches_what_it_draws() {
                let mut side = side_by_side_with_a_parked_window();
                visit_the_parked_window(&mut side);
                let SideBySide {
                    desk,
                    right,
                    parked,
                    ..
                } = side;
                let right_window = window_of(&desk, right.pane);
                let now = desk.state.clock.now();
                let drawn = drawn_now(&desk.state, parked.pane, now).rect;
                let point = Point::<f64, Logical>::from((2500.0, 500.0));
                assert!(
                    drawn.contains(point),
                    "the premise: the parked window is carried over {point:?}, to {drawn:?}"
                );
                // The parked window's carried resize border, twice: just
                // inside its left edge, over the middle of the right window;
                // and just outside its top edge, over the bare gap above
                // both -- where a border that is only a claim, a `Halo`,
                // would win for want of anything drawn beneath it.
                let edge = Point::<f64, Logical>::from((drawn.loc.x + 2.0, 500.0));
                let halo = Point::<f64, Logical>::from((2500.0, drawn.loc.y - 4.0));

                // Everything asked before anything is asserted, so a
                // failure says which walk is wrong rather than only the
                // first.
                let clicked = desk.state.window_under(point).map(|(window, _)| window);
                let delivered = desk.state.surface_under(point).map(|(surface, _)| surface);
                let parked_edge = |at| {
                    desk.state
                        .chrome_under(at)
                        .is_some_and(|under| under.pane == parked.pane)
                };
                assert_eq!(
                    (
                        clicked.as_ref().map(surface_id),
                        delivered.map(|surface| surface.id().protocol_id()),
                        parked_edge(edge),
                        parked_edge(halo),
                    ),
                    (
                        Some(surface_id(&right_window)),
                        Some(surface_id(&right_window)),
                        false,
                        false
                    ),
                    "(click-to-focus, the pointer event, the parked window's edge over the \
                     right window, and over bare desktop): a press on the right monitor \
                     reached the window carried over it from the left monitor's hidden \
                     workspace, surface {}",
                    surface_id(&window_of(&desk, parked.pane))
                );
            }

            /// **An activation**: of the parked window, it leaves the
            /// keyboard where it was; of the window on the right monitor, it
            /// takes the keyboard, as it should.
            #[test]
            fn on_two_monitors_an_activation_of_the_left_monitors_hidden_workspace_leaves_the_keyboard_where_it_was()
             {
                let SideBySide {
                    mut desk,
                    working,
                    right,
                    parked,
                } = side_by_side_with_a_parked_window();
                let working = window_of(&desk, working.pane);
                let token = genuine_token(&mut desk);
                activates(&mut desk, &parked.surface, &token);
                assert_eq!(
                    desk.state.focused_window(),
                    Some(working.clone()),
                    "an activation took the keyboard to the left monitor's hidden workspace"
                );
                typed_into(
                    &mut desk,
                    &working,
                    "a key typed after the activation went somewhere else",
                );
                let (heard, id) = (heard(&desk), parked.pane.get().to_string());
                assert!(
                    !heard.split(',').any(|each| each == id),
                    "and the parked window was focused on the way. Heard: [{heard}]"
                );

                let token = genuine_token(&mut desk);
                activates(&mut desk, &right.surface, &token);
                let right_window = window_of(&desk, right.pane);
                assert_eq!(
                    desk.state.focused_window(),
                    Some(right_window.clone()),
                    "the window on the right monitor asked to be brought forward and was refused"
                );
                typed_into(
                    &mut desk,
                    &right_window,
                    "a key typed after it did not reach it",
                );
            }

            /// **`sol.window_at`, #134's fourth review, finding 1: asked
            /// from Lua, it answers what the Rust hit test answers.** At a
            /// pixel on the right monitor the parked window is carried over,
            /// the window the right monitor draws there; and past the right
            /// window's tile, where the parked window is carried and the
            /// right monitor draws nothing, nothing. It matched the drawn
            /// rectangle against the point and nothing else, so it found the
            /// parked window at both -- stacked above the right one, and the
            /// only window whose rectangle reaches the second.
            #[test]
            fn on_two_monitors_sol_window_at_answers_what_the_right_monitor_draws() {
                let mut side = side_by_side_with_a_parked_window();
                visit_the_parked_window(&mut side);
                let SideBySide {
                    desk,
                    right,
                    parked,
                    ..
                } = side;
                let now = desk.state.clock.now();
                let carried = drawn_now(&desk.state, parked.pane, now).rect;
                let tile = desk.placed(right.pane);
                let screen = right_screen();
                let over = Point::<f64, Logical>::from((2500.0, 500.0));
                // Between the right window's tile and the monitor's edge:
                // the gap the layout leaves there.
                let bare = Point::<f64, Logical>::from((
                    f64::from(tile.loc.x + tile.size.w + screen.loc.x + screen.size.w) / 2.0,
                    500.0,
                ));
                assert!(
                    carried.contains(over)
                        && carried.contains(bare)
                        && tile.to_f64().contains(over)
                        && !tile.to_f64().contains(bare)
                        && screen.to_f64().contains(bare),
                    "the premise: the parked window is carried over {over:?}, which is on the \
                     right window's tile at {tile:?}, and over {bare:?}, which is on the right \
                     monitor and on no tile; it is carried to {carried:?}"
                );

                let from_lua = (
                    window_at_from_lua(&desk, over),
                    window_at_from_lua(&desk, bare),
                );
                let from_rust = (
                    desk.state
                        .window_under(over)
                        .and_then(|(window, _)| desk.state.panes.id_of(&window))
                        .map(|pane| pane.get().to_string()),
                    desk.state
                        .window_under(bare)
                        .and_then(|(window, _)| desk.state.panes.id_of(&window))
                        .map(|pane| pane.get().to_string()),
                );
                assert_eq!(
                    from_lua,
                    (right.pane.get().to_string(), "nil".to_owned()),
                    "(over the right window, over the bare gap beside it): `sol.window_at` \
                     found the window carried over the right monitor from the left one's \
                     hidden workspace, window {}",
                    parked.pane.get()
                );
                assert_eq!(
                    from_rust,
                    (Some(right.pane.get().to_string()), None),
                    "and the Rust hit test gives another answer: the two are not one rule"
                );
            }

            /// **The overview, as shipped: a click on empty space on the
            /// right monitor leaves it, and the keyboard where it was.**
            /// The reviewer's own route to finding 1. Overview shrinks the
            /// windows in view into a grid and leaves the parked one where
            /// its desk carries it, over the right monitor; a click where no
            /// thumbnail is drawn is how it is left, and `overview.lua` hands
            /// whatever `sol.window_at` finds there to `sol.focus`. That was
            /// the parked window, which took the keyboard on a desk nobody
            /// can see.
            #[test]
            fn on_two_monitors_an_overview_click_on_the_right_monitor_leaves_the_keyboard_on_screen()
             {
                let mut side = side_by_side_with_a_parked_window_and("require(\"overview\")");
                visit_the_parked_window(&mut side);
                let SideBySide {
                    mut desk,
                    working,
                    right,
                    parked,
                } = side;
                let working = window_of(&desk, working.pane);
                assert!(
                    desk.state.trigger("super+space"),
                    "super+space was not handled"
                );
                desk.state.clock.advance(Duration::from_secs(1));
                let now = desk.state.clock.now();
                desk.state.settle(now);
                frame(&mut desk);
                assert!(desk.state.script_grab, "the premise: overview owns input");

                // A pixel of the right monitor the parked window is carried
                // over and the right window's thumbnail is not.
                let now = desk.state.clock.now();
                let carried = drawn_now(&desk.state, parked.pane, now).rect;
                let thumbnail = drawn_now(&desk.state, right.pane, now).rect;
                let screen = right_screen();
                let empty = (screen.loc.y..screen.loc.y + screen.size.h)
                    .step_by(4)
                    .flat_map(|y| {
                        (screen.loc.x..screen.loc.x + screen.size.w)
                            .step_by(4)
                            .map(move |x| Point::<f64, Logical>::from((f64::from(x), f64::from(y))))
                    })
                    .find(|point| carried.contains(*point) && !thumbnail.contains(*point))
                    .expect(
                        "the premise: somewhere on the right monitor the parked window is \
                         carried over and the right window's thumbnail is not",
                    );

                let before = heard(&desk);
                assert!(
                    desk.state.trigger_click(empty.x, empty.y),
                    "overview did not take the click"
                );
                desk.state.clock.advance(Duration::from_secs(1));
                let now = desk.state.clock.now();
                desk.state.settle(now);
                frame(&mut desk);
                assert!(
                    !desk.state.script_grab,
                    "the premise: the click left overview"
                );
                assert_eq!(
                    desk.state.focused_window(),
                    Some(working.clone()),
                    "a click on empty space on the right monitor, at {empty:?}, handed the \
                     keyboard to the window carried there from the left monitor's hidden \
                     workspace, surface {}",
                    surface_id(&window_of(&desk, parked.pane))
                );
                typed_into(
                    &mut desk,
                    &working,
                    "a key typed after the click went somewhere else",
                );
                let after = heard(&desk);
                let since = after.get(before.len()..).unwrap_or_default();
                let parked = parked.pane.get().to_string();
                assert!(
                    !since.split(',').any(|id| id == parked),
                    "and the scripts heard the parked window focused. Heard since the \
                     click: [{since}]"
                );
            }

            /// Every answer `sol.window_at` gives a layout, oldest first --
            /// the target a split or a drop was handed, which the tree then
            /// looks past when it holds no such window, so no arrangement
            /// shows it. Installed ahead of the layouts, which look `sol.window_at`
            /// up each time they call it.
            /// `on_two_monitors_tiling_splits_and_both_layouts_drop_onto_what_the_left_monitor_draws`.
            const RECORDS_WINDOW_AT: &str = "asked = {}\n\
                 local window_at = sol.window_at\n\
                 sol.window_at = function(x, y, skip)\n\
                     local id = window_at(x, y, skip)\n\
                     asked[#asked + 1] = tostring(id)\n\
                     return id\n\
                 end";

            /// The last answer [`RECORDS_WINDOW_AT`] heard, and forget them all.
            fn last_asked(desk: &Desk) -> String {
                says(
                    desk,
                    "local last = asked[#asked]; asked = {}; return tostring(last)",
                )
            }

            /// Two monitors side by side under the layout `key` switches on,
            /// at the shipped settings: `a` and `b` side by side on the left
            /// monitor, `right` on the right one -- and then the right
            /// monitor switched to its workspace 2, so `right` is on a
            /// hidden desk carried a screen and a bit to the left, over
            /// `a`, and stacked above it: it had the keyboard last, and no
            /// layout has placed anything since. Every animation landed.
            ///
            /// A fixture of its own rather than
            /// [`side_by_side_with_a_parked_window`], which is tiling's:
            /// only tiling's overflow parks a window on a hidden workspace,
            /// and scrolling answers a focus with a sweep that places -- and
            /// so raises -- every window in view back over a hidden one.
            /// Here the last thing to happen is a switch, which places
            /// nothing.
            /// `on_two_monitors_tiling_splits_and_both_layouts_drop_onto_what_the_left_monitor_draws`.
            struct Carried {
                desk: Desk,
                a: Opened,
                b: Opened,
                right: Opened,
                /// A point on `a`, and inside where `right` is carried.
                over_a: Point<f64, Logical>,
            }

            fn right_monitor_switched_away(key: &str) -> Carried {
                let mut desk = Desk::side_by_side();
                desk.install(&format!("{RECORDS_WINDOW_AT}\n{SHIPPED_HEARING_FOCUS}"));
                let right = desk.open_surface();
                desk.state
                    .map_stacked(window_of(&desk, right.pane), (2020, 100), false);
                frame(&mut desk);
                assert!(desk.state.trigger(key), "{key} was not handled");
                let a = desk.open_surface();
                let b = desk.open_surface();
                for opened in [&right, &a, &b] {
                    desk.answer(opened);
                }
                desk.state.clock.advance(Duration::from_secs(1));
                let now = desk.state.clock.now();
                desk.state.settle(now);
                frame(&mut desk);
                let screen = right_screen();
                assert!(
                    screen.contains_rect(desk.placed(right.pane))
                        && !screen.overlaps(desk.placed(a.pane))
                        && !screen.overlaps(desk.placed(b.pane))
                        && !desk.placed(a.pane).overlaps(desk.placed(b.pane)),
                    "{key}: the premise: `right` on the right monitor, `a` and `b` side by \
                     side on the left, at {:?}, {:?} and {:?}",
                    desk.placed(right.pane),
                    desk.placed(a.pane),
                    desk.placed(b.pane)
                );

                point_at(&mut desk.state, (2500.0, 500.0));
                desk.state
                    .focus_window(&window_of(&desk, right.pane), SERIAL_COUNTER.next_serial());
                assert!(desk.state.trigger("super+2"), "super+2 was not handled");
                desk.state.clock.advance(Duration::from_secs(1));
                let now = desk.state.clock.now();
                desk.state.settle(now);
                frame(&mut desk);
                assert_eq!(
                    (
                        showing(&desk),
                        says(
                            &desk,
                            &format!(
                                "return tostring(require(\"workspaces\").on({RIGHT_SCREEN:?}))"
                            )
                        ),
                        workspace_of(&desk, right.pane),
                    ),
                    ("1".to_owned(), "2".to_owned(), "1".to_owned()),
                    "{key}: the premise: the right monitor shows workspace 2, the left one 1, \
                     and `right` is on 1"
                );
                let carried = drawn_now(&desk.state, right.pane, now).rect;
                let on_a = desk.placed(a.pane).to_f64().intersection(carried).expect(
                    "the premise: `right` is carried over `a`, which is what the left \
                     monitor draws there",
                );
                let over_a = Point::<f64, Logical>::from((
                    on_a.loc.x + on_a.size.w / 2.0,
                    on_a.loc.y + on_a.size.h / 2.0,
                ));
                assert!(
                    above(&mut desk.state, right.pane, a.pane),
                    "{key}: the premise: `right` is stacked above `a`, so a walk topmost \
                     first meets it first"
                );
                let _ = last_asked(&desk);
                Carried {
                    desk,
                    a,
                    b,
                    right,
                    over_a,
                }
            }

            /// **Tiling's split at open and both layouts' drops, #134's
            /// fourth review, finding 1: each is handed the window the
            /// monitor under the point draws**, and not a window on a
            /// hidden desk carried over it. The split and tiling's drop
            /// hand the target to a tree that holds no such window, which
            /// then falls back on its own hit test, so only the target
            /// itself shows the fault; scrolling's drop checks its strip
            /// holds the target, and so dropped onto `a` went nowhere.
            #[test]
            fn on_two_monitors_tiling_splits_and_both_layouts_drop_onto_what_the_left_monitor_draws()
             {
                let Carried {
                    mut desk,
                    a,
                    right,
                    over_a,
                    ..
                } = right_monitor_switched_away("super+t");
                point_at(&mut desk.state, (over_a.x, over_a.y));
                let _ = desk.open_surface();
                let at_open = last_asked(&desk);

                let Carried {
                    mut desk,
                    a: tiled_a,
                    b,
                    over_a,
                    ..
                } = right_monitor_switched_away("super+t");
                desk.state
                    .trigger_drop(&window_of(&desk, b.pane), over_a.x, over_a.y);
                let tiling_drop = last_asked(&desk);

                let Carried {
                    mut desk,
                    a: strip_a,
                    b: strip_b,
                    over_a,
                    ..
                } = right_monitor_switched_away("super+s");
                desk.state
                    .trigger_drop(&window_of(&desk, strip_b.pane), over_a.x, over_a.y);
                let scrolling_drop = last_asked(&desk);
                desk.state.clock.advance(Duration::from_secs(1));
                let now = desk.state.clock.now();
                desk.state.settle(now);
                frame(&mut desk);
                let joined = desk.placed(strip_b.pane).loc.x == desk.placed(strip_a.pane).loc.x;

                assert_eq!(
                    (at_open, tiling_drop, scrolling_drop, joined),
                    (
                        a.pane.get().to_string(),
                        tiled_a.pane.get().to_string(),
                        strip_a.pane.get().to_string(),
                        true
                    ),
                    "(tiling's split target at open, tiling's drop target, scrolling's drop \
                     target, whether the window dropped in scrolling joined `a`'s column): a \
                     layout was handed the window carried over the left monitor from the \
                     right one's hidden workspace, window {}",
                    right.pane.get()
                );
            }
        }

        /// **#141 and #142: what is drawn over what, above and below the
        /// windows, and whether the one on top is the one the pointer
        /// reaches.**
        ///
        /// The picture is asked of [`crate::render::stacked`], which is
        /// everything `render::elements` draws below the drag icon, in its
        /// order -- `elements` needs a GPU and only turns each entry into
        /// elements. The pointer is asked of the hit tests themselves.
        mod stacking {
            use super::*;
            use crate::render::Stacked;
            use crate::scripted::Layer as Scripted;
            use zwlr_layer_shell_v1::Layer as Client;

            /// Something on the monitor, by a number both ends of the
            /// fixture agree on.
            #[derive(Clone, Copy, Debug, PartialEq, Eq)]
            enum Seen {
                /// A client's layer surface, by its `wl_surface`'s id.
                Layer(u32),
                /// A script's surface.
                Script(crate::scripted::SurfaceId),
                /// A pane.
                Pane(crate::pane::PaneId),
            }

            /// The desk's monitor.
            fn screen() -> Rectangle<i32, Logical> {
                Rectangle::new((0, 0).into(), (1920, 1080).into())
            }

            /// What the desk's monitor draws, topmost first.
            fn drawn(desk: &Desk) -> Vec<Seen> {
                crate::render::stacked(&desk.state, screen(), desk.state.clock.now())
                    .into_iter()
                    .flat_map(|each| match each {
                        Stacked::Layer(surface, _) => {
                            vec![Seen::Layer(surface.wl_surface().id().protocol_id())]
                        }
                        Stacked::Surface(id, _, _) => vec![Seen::Script(id)],
                        Stacked::Panes(nodes) => nodes
                            .into_iter()
                            .map(|((pane, ..), _)| Seen::Pane(pane))
                            .collect(),
                    })
                    .collect()
            }

            /// Whether `top` is drawn over `bottom`, both being drawn.
            fn drawn_over(desk: &Desk, top: Seen, bottom: Seen) -> bool {
                let drawn = drawn(desk);
                let at = |seen| drawn.iter().position(|each| *each == seen);
                matches!((at(top), at(bottom)), (Some(top), Some(bottom)) if top < bottom)
            }

            /// A client's layer surface at `layer`, anchored to the top of
            /// the monitor: across its whole width, or `width` wide from its
            /// left edge. With a buffer of that size, so it is drawn and can
            /// be pointed at.
            fn layer_surface(
                desk: &mut Desk,
                layer: Client,
                width: Option<i32>,
                height: i32,
                exclusive: i32,
            ) -> (
                wl_surface::WlSurface,
                zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
            ) {
                use zwlr_layer_surface_v1::Anchor;
                let compositor = desk.client.compositor.clone().expect("wl_compositor bound");
                let shell = desk
                    .client
                    .layer_shell
                    .clone()
                    .expect("zwlr_layer_shell_v1 bound");
                let surface = compositor.create_surface(&desk.qh, ());
                let layered = shell.get_layer_surface(
                    &surface,
                    None,
                    layer,
                    "stacking-test".to_string(),
                    &desk.qh,
                    (),
                );
                match width {
                    Some(width) => {
                        layered.set_anchor(Anchor::Top | Anchor::Left);
                        layered.set_size(width.unsigned_abs(), height.unsigned_abs());
                    }
                    None => {
                        layered.set_anchor(Anchor::Top | Anchor::Left | Anchor::Right);
                        layered.set_size(0, height.unsigned_abs());
                    }
                }
                layered.set_exclusive_zone(exclusive);
                surface.commit();
                desk.pump();
                commit_buffer(
                    &desk.client,
                    &desk.qh,
                    &surface,
                    width.unwrap_or(screen().size.w),
                    height,
                );
                desk.pump();
                (surface, layered)
            }

            fn id(surface: &wl_surface::WlSurface) -> u32 {
                wayland_client::Proxy::id(surface).protocol_id()
            }

            /// Where the pointer is delivered at a point: the surface, by id.
            fn delivered(desk: &Desk, x: f64, y: f64) -> Option<u32> {
                desk.state
                    .surface_under((x, y).into())
                    .map(|(surface, _)| surface.id().protocol_id())
            }

            /// A script's interactive surface at `layer`, over `rect`.
            fn scripted(
                desk: &mut Desk,
                name: &str,
                layer: Scripted,
                rect: Rectangle<i32, Logical>,
            ) -> crate::scripted::SurfaceId {
                desk.state.declare_surface(crate::scripted::Declaration {
                    name: name.to_owned(),
                    scene: std::path::PathBuf::from("/nonexistent/stacking-test.qml"),
                    layer,
                    on: crate::scripted::On::Rect(rect),
                    properties: "{}".to_owned(),
                    interactive: true,
                });
                desk.state
                    .surfaces
                    .named(name)
                    .expect("the surface was declared")
            }

            /// The script's surface a press at a point is offered to.
            fn claimed(desk: &Desk, x: f64, y: f64) -> Option<crate::scripted::SurfaceId> {
                desk.state
                    .surface_claiming(true, (x, y).into())
                    .map(|(_, id, _)| id)
            }

            fn window_of(desk: &Desk, pane: crate::pane::PaneId) -> Window {
                desk.state
                    .panes
                    .get(pane)
                    .and_then(Pane::client)
                    .cloned()
                    .expect("the pane has its client")
            }

            /// A window at `at`, sent fullscreen, answering with a buffer
            /// the size of the monitor as a client does.
            fn fullscreen(desk: &mut Desk, at: (i32, i32)) -> (Opened, Window) {
                let opened = desk.open_surface();
                let window = window_of(desk, opened.pane);
                desk.state.space.map_element(window.clone(), at, false);
                desk.state.space.refresh();
                opened.toplevel.set_fullscreen(None);
                desk.pump();
                commit_buffer(
                    &desk.client,
                    &desk.qh,
                    &opened.surface,
                    screen().size.w,
                    screen().size.h,
                );
                desk.pump();
                landed(desk);
                assert_eq!(
                    desk.state.real_geometry(&window),
                    Some(screen()),
                    "the premise: the window covers the monitor, bar and all"
                );
                (opened, window)
            }

            /// Let every animation land: a window opening is drawn fading in,
            /// and one drawn at nothing covers nothing.
            fn landed(desk: &mut Desk) {
                desk.state.clock.advance(Duration::from_secs(1));
                let now = desk.state.clock.now();
                desk.state.settle(now);
                desk.state.sync_panes();
                desk.pump();
            }

            fn strip(x: i32, width: i32) -> Rectangle<i32, Logical> {
                Rectangle::new((x, 0).into(), (width, 30).into())
            }

            /// **An overlay surface mapped before a bar is drawn over it,
            /// and takes the press there -- before the edge of the window
            /// under both.** The bar was drawn on top because it was mapped
            /// last, while the press went to the overlay; and neither beat
            /// the window's resize border, which `pointer_button` asked
            /// before any client's surface.
            #[test]
            fn an_overlay_mapped_before_a_bar_is_drawn_over_it_and_takes_the_press() {
                let mut desk = Desk::new();
                let opened = desk.open_surface();
                let window = window_of(&desk, opened.pane);
                desk.state.space.map_element(window, (500, 10), false);
                desk.state.space.refresh();
                landed(&mut desk);
                let edge = Point::<f64, Logical>::from((532.0, 12.0));
                assert_eq!(
                    desk.state.claim_under(edge),
                    Claim::Chrome(Chrome::Resize(ResizeEdge::Top)),
                    "the premise: with nothing over it, a press there drags the window's edge"
                );

                let (overlay, _overlay) = layer_surface(&mut desk, Client::Overlay, None, 30, -1);
                let (bar, _bar) = layer_surface(&mut desk, Client::Top, None, 30, 30);

                assert!(
                    drawn_over(&desk, Seen::Layer(id(&overlay)), Seen::Layer(id(&bar)))
                        && drawn_over(&desk, Seen::Layer(id(&bar)), Seen::Pane(opened.pane)),
                    "the overlay, the bar, then the window: {:?}",
                    drawn(&desk)
                );
                assert_eq!(
                    (
                        delivered(&desk, edge.x, edge.y),
                        desk.state.claim_under(edge)
                    ),
                    (Some(id(&overlay)), Claim::Nothing),
                    "(the surface the pointer reaches, what the compositor makes of a press): \
                     the overlay is on top and the press is its"
                );
            }

            /// **The same, for a script's surfaces**: one declared at
            /// `overlay` before one at `top` is drawn over it and is the one
            /// offered the press.
            #[test]
            fn a_scripted_overlay_is_drawn_over_a_scripted_bar_and_takes_the_press() {
                let mut desk = Desk::new();
                let overlay = scripted(&mut desk, "overlay", Scripted::Overlay, strip(0, 1920));
                let bar = scripted(&mut desk, "bar", Scripted::Top, strip(0, 1920));
                assert!(
                    drawn_over(&desk, Seen::Script(overlay), Seen::Script(bar)),
                    "the scripted overlay over the scripted bar: {:?}",
                    drawn(&desk)
                );
                assert_eq!(
                    (
                        claimed(&desk, 100.0, 15.0),
                        desk.state.claim_under((100.0, 15.0).into())
                    ),
                    (Some(overlay), Claim::Surface)
                );
            }

            /// **Below the windows, by layer too**: a client's background
            /// mapped after its bottom surface stays under it, and at each
            /// layer the client's surface is over the script's -- and the
            /// script's dock has the press only where nothing is drawn over
            /// it.
            #[test]
            fn a_background_mapped_after_a_bottom_surface_stays_under_it() {
                let mut desk = Desk::new();
                let opened = desk.open_surface();
                let window = window_of(&desk, opened.pane);
                desk.state.space.map_element(window, (600, 500), false);
                desk.state.space.refresh();
                landed(&mut desk);
                let script_bottom = scripted(&mut desk, "dock", Scripted::Bottom, strip(0, 1920));
                let script_background =
                    scripted(&mut desk, "wallpaper", Scripted::Background, screen());
                let (bottom, _bottom) = layer_surface(&mut desk, Client::Bottom, Some(200), 30, 0);
                let (background, _background) =
                    layer_surface(&mut desk, Client::Background, None, 30, 0);
                let below: Vec<Seen> = drawn(&desk)
                    .into_iter()
                    .skip_while(|seen| *seen != Seen::Pane(opened.pane))
                    .skip(1)
                    .collect();
                assert_eq!(
                    below,
                    vec![
                        Seen::Layer(id(&bottom)),
                        Seen::Script(script_bottom),
                        Seen::Layer(id(&background)),
                        Seen::Script(script_background),
                    ],
                    "under the window: the client's bottom surface, the script's, the \
                     client's background, the script's"
                );
                // A client's surfaces below the windows are offered no
                // pointer, and never were -- but the client's bottom surface
                // is drawn over the script's dock, so the dock does not have
                // the press there either. Beside it, over only the client's
                // background, it does.
                assert_eq!(
                    (
                        delivered(&desk, 100.0, 15.0),
                        claimed_below(&desk, 100.0, 15.0),
                        delivered(&desk, 1000.0, 15.0),
                        claimed_below(&desk, 1000.0, 15.0),
                    ),
                    (None, None, None, Some(script_bottom)),
                    "(on the client's bottom surface: the surface the pointer reaches, the \
                     script's surface offered the press; the same beside it)"
                );
            }

            /// The script's surface below the windows a press at a point is
            /// offered to.
            fn claimed_below(desk: &Desk, x: f64, y: f64) -> Option<crate::scripted::SurfaceId> {
                desk.state
                    .surface_claiming(false, (x, y).into())
                    .map(|(_, id, _)| id)
            }

            /// **A window over a script's dock keeps the press, and so do
            /// its menu reaching past it and a client's bar.** The dock was
            /// offered every press below the windows whatever was drawn over
            /// it: click-to-focus focused the window, and the press went to
            /// the dock.
            #[test]
            fn a_window_over_a_scripted_dock_keeps_the_press() {
                let mut desk = Desk::new();
                let dock = scripted(&mut desk, "dock", Scripted::Bottom, screen());
                let (bar, _bar) = layer_surface(&mut desk, Client::Top, Some(200), 30, 0);
                let opened = desk.open_surface();
                commit_buffer(&desk.client, &desk.qh, &opened.surface, 400, 300);
                desk.pump();
                let window = window_of(&desk, opened.pane);
                desk.state
                    .space
                    .map_element(window.clone(), (100, 100), false);
                desk.state.space.refresh();
                landed(&mut desk);
                let menu = drawn_popup(
                    &mut desk.display,
                    &mut desk.state,
                    &desk.conn,
                    &desk.qh,
                    &mut desk.queue,
                    &mut desk.client,
                    &opened.xdg,
                    (390, 50),
                    (120, 80),
                );
                let on_the_menu = Point::<f64, Logical>::from((560.0, 190.0));
                assert!(
                    desk.state
                        .real_geometry(&window)
                        .is_some_and(|real| !real.to_f64().contains(on_the_menu))
                        && delivered(&desk, on_the_menu.x, on_the_menu.y) == Some(id(&menu)),
                    "the premise: the menu reaches past its window, and has the pointer there"
                );

                assert_eq!(
                    (
                        desk.state
                            .window_under((300.0, 250.0).into())
                            .map(|(under, _)| under),
                        claimed_below(&desk, 300.0, 250.0),
                        claimed_below(&desk, on_the_menu.x, on_the_menu.y),
                        (
                            delivered(&desk, 100.0, 15.0),
                            claimed_below(&desk, 100.0, 15.0)
                        ),
                        claimed_below(&desk, 1000.0, 700.0),
                    ),
                    (Some(window), None, None, (Some(id(&bar)), None), Some(dock)),
                    "(the window a press on it focuses, the dock offered that press, the dock \
                     offered one on the menu, the surface the pointer reaches on the bar and \
                     the dock offered a press there, the dock offered one where nothing is \
                     over it)"
                );
            }

            /// **Within one layer, the order each kind already had**: the
            /// client's surface mapped last is on top and has the pointer,
            /// and the script's declared first is on top and has the press.
            #[test]
            fn within_a_layer_the_order_is_the_one_each_kind_had() {
                let mut desk = Desk::new();
                let (first, _first) = layer_surface(&mut desk, Client::Top, None, 30, 0);
                let (second, _second) = layer_surface(&mut desk, Client::Top, None, 30, 0);
                let below_the_bars = Rectangle::new((0, 100).into(), (1920, 30).into());
                let one = scripted(&mut desk, "one", Scripted::Top, below_the_bars);
                let two = scripted(&mut desk, "two", Scripted::Top, below_the_bars);
                assert!(
                    drawn_over(&desk, Seen::Layer(id(&second)), Seen::Layer(id(&first)))
                        && drawn_over(&desk, Seen::Script(one), Seen::Script(two)),
                    "{:?}",
                    drawn(&desk)
                );
                assert_eq!(
                    (delivered(&desk, 100.0, 15.0), claimed(&desk, 100.0, 115.0)),
                    (Some(id(&second)), Some(one))
                );
            }

            /// **A fullscreen window covers the bar, the client's and the
            /// script's, and takes the press where the bar was.**
            #[test]
            fn a_fullscreen_window_covers_a_bar_and_takes_the_press_where_it_was() {
                let mut desk = Desk::new();
                let (bar, _bar) = layer_surface(&mut desk, Client::Top, None, 30, 30);
                let shell = scripted(&mut desk, "shell", Scripted::Top, strip(0, 1920));
                let (opened, window) = fullscreen(&mut desk, (10, 40));

                assert!(
                    drawn_over(&desk, Seen::Pane(opened.pane), Seen::Layer(id(&bar)))
                        && drawn_over(&desk, Seen::Pane(opened.pane), Seen::Script(shell)),
                    "the fullscreen window over both bars: {:?}",
                    drawn(&desk)
                );
                let at = Point::<f64, Logical>::from((100.0, 15.0));
                assert_eq!(
                    (
                        delivered(&desk, at.x, at.y),
                        claimed(&desk, at.x, at.y),
                        desk.state.window_under(at).map(|(under, _)| under),
                    ),
                    (Some(surface_id(&window)), None, Some(window.clone())),
                    "(the surface the pointer reaches, the scripted bar being offered the \
                     press, the window a press focuses)"
                );
            }

            /// **An overlay surface stays over a fullscreen window and keeps
            /// its presses**, the client's and the script's -- and the bar,
            /// mapped after the client's overlay, stays under both.
            #[test]
            fn an_overlay_stays_over_a_fullscreen_window_and_keeps_its_presses() {
                let mut desk = Desk::new();
                let (overlay, _overlay) =
                    layer_surface(&mut desk, Client::Overlay, Some(200), 30, -1);
                let (bar, _bar) = layer_surface(&mut desk, Client::Top, None, 30, 30);
                let osd = scripted(&mut desk, "osd", Scripted::Overlay, strip(300, 200));
                let (opened, window) = fullscreen(&mut desk, (10, 40));

                let pane = Seen::Pane(opened.pane);
                assert!(
                    drawn_over(&desk, Seen::Layer(id(&overlay)), pane)
                        && drawn_over(&desk, Seen::Script(osd), pane)
                        && drawn_over(&desk, pane, Seen::Layer(id(&bar))),
                    "both overlays, the fullscreen window, the bar: {:?}",
                    drawn(&desk)
                );
                assert_eq!(
                    (
                        delivered(&desk, 100.0, 15.0),
                        delivered(&desk, 1000.0, 15.0),
                        claimed(&desk, 400.0, 15.0),
                        desk.state.claim_under((400.0, 15.0).into()),
                    ),
                    (
                        Some(id(&overlay)),
                        Some(surface_id(&window)),
                        Some(osd),
                        Claim::Surface
                    ),
                    "(on the client's overlay, on the bar beside it, on the script's \
                     overlay, what a press there is)"
                );
            }

            /// **Leaving fullscreen puts the bar back on top.**
            #[test]
            fn leaving_fullscreen_puts_the_bar_back_on_top() {
                let mut desk = Desk::new();
                let (bar, _bar) = layer_surface(&mut desk, Client::Top, None, 30, 30);
                let (opened, window) = fullscreen(&mut desk, (10, 5));
                let pane = Seen::Pane(opened.pane);
                assert!(
                    drawn_over(&desk, pane, Seen::Layer(id(&bar)))
                        && delivered(&desk, 20.0, 15.0) == Some(surface_id(&window)),
                    "the premise: fullscreen, the window covers the bar"
                );

                opened.toplevel.unset_fullscreen();
                desk.pump();
                commit_buffer(&desk.client, &desk.qh, &opened.surface, 64, 64);
                desk.pump();
                landed(&mut desk);
                assert_eq!(
                    desk.state.real_geometry(&window),
                    Some(Rectangle::new((10, 5).into(), (64, 64).into())),
                    "the premise: back where it was, under the bar's strip"
                );
                assert!(
                    drawn_over(&desk, Seen::Layer(id(&bar)), pane),
                    "the bar is back over the window: {:?}",
                    drawn(&desk)
                );
                assert_eq!(delivered(&desk, 20.0, 15.0), Some(id(&bar)));
            }

            /// **A fullscreen window on a workspace that is not shown does
            /// not hide the bar.**
            #[test]
            fn a_fullscreen_window_on_a_workspace_not_shown_leaves_the_bar_on_top() {
                let mut desk = Desk::new();
                desk.install(
                    "require(\"modes\")\n\
                     require(\"workspaces\")\n\
                     require(\"tiling\")\n\
                     require(\"scrolling\")",
                );
                let (bar, _bar) = layer_surface(&mut desk, Client::Top, None, 30, 30);
                let (opened, window) = fullscreen(&mut desk, (10, 40));
                let says = |desk: &Desk, chunk: &str| {
                    desk.state
                        .scripts
                        .as_ref()
                        .map(|scripts| scripts.evaluate(chunk))
                        .unwrap_or_default()
                };
                let showing = "return tostring(require(\"workspaces\").on(\"reflow-test\"))";
                let pane = Seen::Pane(opened.pane);
                assert!(
                    says(&desk, showing) == "1"
                        && drawn_over(&desk, pane, Seen::Layer(id(&bar)))
                        && delivered(&desk, 100.0, 15.0) == Some(surface_id(&window)),
                    "the premise: on the workspace in view, the window covers the bar"
                );

                assert!(desk.state.trigger("super+2"), "super+2 was not handled");
                landed(&mut desk);
                assert_eq!(
                    (
                        says(&desk, showing),
                        says(
                            &desk,
                            &format!(
                                "return tostring(require(\"workspaces\").of[{}])",
                                opened.pane.get()
                            )
                        )
                    ),
                    ("2".to_owned(), "1".to_owned()),
                    "the premise: workspace 2 in view, the window on 1"
                );
                assert!(
                    drawn_over(&desk, Seen::Layer(id(&bar)), pane),
                    "the bar is over the window on the workspace not shown: {:?}",
                    drawn(&desk)
                );
                assert_eq!(delivered(&desk, 100.0, 15.0), Some(id(&bar)));
            }

            /// **`fullscreen.covers = "none"` keeps the bar over a
            /// fullscreen window**, set the way `init.lua` sets it.
            #[test]
            fn with_fullscreen_covers_none_the_bar_stays_over_a_fullscreen_window() {
                let mut desk = Desk::new();
                let (bar, _bar) = layer_surface(&mut desk, Client::Top, None, 30, 30);
                let (opened, window) = fullscreen(&mut desk, (10, 40));
                let pane = Seen::Pane(opened.pane);
                assert!(
                    drawn_over(&desk, pane, Seen::Layer(id(&bar)))
                        && delivered(&desk, 100.0, 15.0) == Some(surface_id(&window)),
                    "the premise: by default the window covers the bar"
                );

                desk.install(
                    "local config = require(\"config\")\n\
                     config.fullscreen.covers = \"none\"\n\
                     sol.fullscreen(config.fullscreen)",
                );
                let scripts = desk.state.scripts.take();
                desk.state.start_scripts(scripts);
                landed(&mut desk);
                assert!(
                    drawn_over(&desk, Seen::Layer(id(&bar)), pane),
                    "the bar is over the fullscreen window: {:?}",
                    drawn(&desk)
                );
                assert_eq!(delivered(&desk, 100.0, 15.0), Some(id(&bar)));
            }

        }
    }

    /// A window opened, mapped at a known place, and known to the panes.
    ///
    /// The three closing tests below all start here, and all three need a
    /// *real* client rather than a loading pane: `settle_closing` removes a
    /// pane with no client outright — "the one case where closing is
    /// entirely ours to decide" — so a fixture without one never reaches
    /// `send_close`, never stamps `asked_at`, and so cannot see either of
    /// the two faults that live after the request goes out.
    fn opened_at(
        display: &mut Display<Solium>,
        state: &mut Solium,
        conn: &Connection,
        client: &Client,
        qh: &QueueHandle<Client>,
        place: (i32, i32),
    ) -> (Window, crate::pane::PaneId) {
        let (window, _toplevel) = open_window(display, state, conn, client, qh);
        state.map_stacked(window.clone(), place, false);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&window)
            .expect("a client in the space has a pane");
        (window, pane)
    }

    /// How this pane is drawn at `now`, through the same call the renderer
    /// makes.
    fn drawn_now(state: &Solium, pane: crate::pane::PaneId, now: Duration) -> Frame {
        let outer = state
            .pane_outer_of(pane)
            .expect("a mapped pane has a rectangle");
        state.drawn_id_at(pane, outer, now)
    }

    /// **Issue #127, fault 1: a layout sweep during a close put the dying
    /// window back at full opacity, and it then vanished with no
    /// animation.**
    ///
    /// `Pane::closing_at` had three production readers and `move_pane` was
    /// not one of them, so the unconditional `present::from` at the end of
    /// every placement overwrote the closing transform with a *released*
    /// one aimed at `Frame::real` — full size, full opacity. Any sweep
    /// inside the 190 ms `CLOSING` window did it, and a sweep inside that
    /// window is ordinary: another window opening, a layer surface's first
    /// configure, a GTK4 `set_parent`. That is the "sometimes close
    /// animations does not even play" the issue was filed for.
    ///
    /// **Three assertions, and they are three different claims.** The
    /// opacity is the fault itself. The position is the *decision* — a
    /// closing pane animates out from where it was, and does not slide to
    /// the slot the layout has just given it; see `move_pane`, which argues
    /// it. And `placed` is the half that must keep working: only the
    /// transform is suppressed, the layout's bookkeeping is untouched, and
    /// #124 reads `placed`.
    #[test]
    fn a_layout_sweep_does_not_cancel_a_close_that_is_already_playing() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (_window, pane) = opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );

        let was = state
            .pane_outer_of(pane)
            .expect("a mapped pane has a rectangle");
        // Sampled before the call, which samples it again for itself: the
        // gap is microseconds and every instant below is offset by enough
        // milliseconds that it cannot matter.
        let pressed = state.clock.now();
        state.close_pane(pane);
        assert!(
            drawn_now(&state, pane, pressed + Duration::from_millis(95)).opacity < 0.9,
            "the close animation is under way; if it were not, nothing \
             below is testing anything"
        );

        // The sweep. A window opening on the other half of the screen is
        // the commonest way to get one, and `tiling.apply` re-places every
        // leaf on every visible monitor — including this one, which is
        // still in the tree because its client has not gone yet.
        let elsewhere = at(1000, 300, 64, 64);
        state.move_pane(
            pane,
            elsewhere,
            was,
            AnimationSpec::default(),
            pressed + Duration::from_millis(95),
            Standing::Tile,
        );

        let landed = drawn_now(&state, pane, pressed + Duration::from_millis(400));
        assert!(
            landed.opacity.abs() < f32::EPSILON,
            "a pane the layout moved mid-close must still finish its \
             close: it was drawn at opacity {} instead",
            landed.opacity
        );
        // 400 + 64 * (1 - 0.86) / 2: `Frame::scaled` is about the centre,
        // so the shrunk rectangle sits inside the one the window was
        // closed at. The claim is which of the two places it is near, and
        // 600 logical pixels separate them.
        assert!(
            landed.rect.loc.x < 500.0,
            "a closing pane animates out from where it was, not from the \
             slot it will never occupy: it was drawn at x={}",
            landed.rect.loc.x
        );

        let placed = state
            .panes
            .get(pane)
            .and_then(Pane::placed)
            .expect("the sweep placed this pane");
        assert_eq!(
            placed, elsewhere,
            "only the transform is suppressed. The layout's own answer is \
             still written, because the layout is still right about where \
             this pane lives and #124 reads it"
        );

        // **And the close still completes.** The assertions above are all
        // about the frame at T+400, which a suppression that also swallowed
        // the request would satisfy perfectly: the window would sit at
        // opacity 0 for ever and no client would ever be told. Driving
        // `settle_closing` past the deadline is what separates "still
        // animating out" from "stuck invisible", and it is the claim the
        // sentence "a swept close still sends its one request and stays
        // gone" was making with nothing behind it.
        state.settle_closing(pressed + Duration::from_millis(400));
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(
            client.closes.len(),
            1,
            "a close the layout swept still sends its one request"
        );
        assert!(
            drawn_now(&state, pane, pressed + Duration::from_millis(500))
                .opacity
                .abs()
                < f32::EPSILON,
            "and the window stays gone afterwards rather than being handed \
             back by the sweep"
        );
    }

    /// **Issue #127, fault 2: a second close restarted an invisible
    /// animation and sent a second `send_close`.**
    ///
    /// `close_pane`'s guard was `closing_at().is_some()`, and
    /// `settle_closing` clears `closing_at` before it stamps `asked_at` —
    /// so from the moment the request goes out until `settle_refused` gives
    /// up on it, a pane already on its way out answered "not closing" to
    /// the one question that was asked about it. A second `super+q` in that
    /// window restarted `present::close` from the held opacity-0 frame
    /// (invisible to invisible, so nothing to see) and asked the client
    /// again.
    ///
    /// Asking twice is not harmless. A client showing "save your work?" is
    /// a client that received the first request and is acting on it; a
    /// second one is a second dialog.
    #[test]
    fn a_second_close_inside_the_grace_neither_restarts_nor_asks_again() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (_window, pane) = opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert!(
            client.closes.is_empty(),
            "opening a window asks nothing to close"
        );

        let pressed = state.clock.now();
        state.close_pane(pane);
        let asked = pressed + present::CLOSING + Duration::from_millis(10);
        state.settle_closing(asked);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(client.closes.len(), 1, "the press asked, once");
        assert!(
            drawn_now(&state, pane, asked).opacity.abs() < f32::EPSILON,
            "and the window is held invisible while the client decides, \
             which is what makes a second press invisible too"
        );

        // The second `super+q`, well inside the grace period. There is
        // nothing on screen for the user to have aimed it at, which is
        // exactly why it happens: the window went and the client has not.
        state.close_pane(pane);
        assert!(
            state.panes.get(pane).and_then(Pane::closing_at).is_none(),
            "a pane already on its way out must not have its animation \
             restarted -- and restarting it from an invisible frame to an \
             invisible frame is 190ms of nothing"
        );

        // Far enough past a second `CLOSING` that a restarted timer would
        // have come due and sent its request.
        state.settle_closing(asked + present::CLOSING + Duration::from_millis(10));
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(
            client.closes.len(),
            1,
            "and the client is asked once per window, not once per press"
        );
    }

    /// **Issue #127, fault 3: a slow client faded out, faded back in, then
    /// popped.**
    ///
    /// `settle_refused` measures its grace from the instant the request
    /// went out and brings the window back when it expires. At 400 ms that
    /// is not a deadline on *refusal* — it is a deadline on slowness, and
    /// Electron's `before-quit`, the JVM's window listeners and Firefox's
    /// session flush all run past it. What the user saw was the window fade
    /// away, come back, and then vanish with no animation at all when the
    /// client finally did close.
    ///
    /// **Written against a client latency, not against the constant.** 600
    /// ms is the claim: a client that takes that long to honour a close is
    /// never shown again. A test that restated `GRACE` would pass at any
    /// value including the one that caused the bug — this fails for every
    /// grace period shorter than 600 ms, whatever it is called.
    ///
    /// **And the other direction, in the same test**, because the recovery
    /// is right and deleting it would otherwise turn this green: a window
    /// that never answers is still brought back, and soon enough that it
    /// reads as an answer to the press. Together the two halves pin the
    /// grace period into a range rather than onto a number.
    #[test]
    fn a_client_that_takes_six_hundred_milliseconds_to_close_is_never_shown_again() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (_window, pane) = opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );

        let pressed = state.clock.now();
        state.close_pane(pane);
        let asked = pressed + present::CLOSING + Duration::from_millis(10);
        state.settle_closing(asked);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(client.closes.len(), 1, "the request went out");

        /// What a heavy client costs between receiving `xdg_toplevel.close`
        /// and destroying its toplevel. The fixture's client never destroys
        /// anything, so this is simply how long the test waits before
        /// declaring the window safely gone.
        const SLOW: Duration = Duration::from_millis(600);
        /// A frame at 60Hz. The fault is a frame the user can see, so the
        /// assertion is made at the rate frames are drawn.
        const FRAME: Duration = Duration::from_millis(16);

        /// By when a window that never answers must be back on screen and
        /// fully opaque. Not arithmetic: it is the span in which a window
        /// reappearing still reads as caused by the press rather than as
        /// the session doing something by itself, and it has to cover the
        /// recovery's own fade as well as the wait before it.
        const LOST: Duration = Duration::from_millis(1500);

        let mut waited = Duration::ZERO;
        // The loop that ships: `Solium::settle` calls this once a frame.
        // Sampling the frame at the same instant is what makes the
        // assertion about what is on screen rather than about a timer.
        while waited <= LOST {
            let now = asked + waited;
            state.settle_refused(now);
            let opacity = drawn_now(&state, pane, now).opacity;
            if waited <= SLOW {
                assert!(
                    opacity.abs() < f32::EPSILON,
                    "{}ms after the request the window was drawn at opacity \
                     {opacity}; a client this slow is closing, not \
                     refusing, and bringing it back means fade out, fade \
                     in, pop",
                    waited.as_millis()
                );
            }
            waited += FRAME;
        }

        // And the other direction. The recovery is right and must survive:
        // a window animated away that then refuses to close would
        // otherwise be invisible and alive, holding its place in the
        // layout, with nothing to bring it back.
        let back = drawn_now(&state, pane, asked + LOST).opacity;
        assert!(
            (back - 1.0).abs() < f32::EPSILON,
            "a window that never answers is still brought back, and within \
             a second and a half of being asked: it was drawn at opacity \
             {back}"
        );
    }

    /// **#127 review, finding 1: an invisible closing pane went on winning
    /// every hit test at the rectangle it used to occupy.**
    ///
    /// `window_under`, `surface_under`, `pane_chrome` and `decorated_under`
    /// all asked `drawn_at(..).rect.contains(location)` and nothing else.
    /// `present::close` ends at opacity 0 and is written with
    /// `release: false` on purpose, so the transform *holds* there — for
    /// `CLOSING` plus the whole grace period, which #127 took from about
    /// 590 ms to about 1190 ms. For all of it the dead window was the
    /// topmost thing at a rectangle the layout had already given to
    /// somebody else.
    ///
    /// **The trade #127 made without noticing.** It removed a cosmetic
    /// flicker — the window fading back in and popping — and what that
    /// flicker had been doing was *telling the user the window was still
    /// there*. Silencing it while leaving the hit test alone turns a
    /// visible glitch into an invisible one: the sibling has reflowed into
    /// the space and is what is on screen, a click there lands in the dead
    /// window, and every keystroke after it follows the focus that click
    /// set. For exactly the slow-but-honest clients the grace bump was
    /// written for.
    ///
    /// **Both halves of a press, because they are two walks and either
    /// alone would leave the other broken.** `window_under` is what
    /// click-to-focus raises and focuses — the keystroke half, since focus
    /// is what the typing follows — and `surface_under` is what the pointer
    /// event is actually delivered to. `chrome_under` is asserted beside
    /// them because an invisible titlebar is still a titlebar to a walk
    /// that only measures rectangles.
    ///
    /// The pane being closed is opened *second* so that it is above the
    /// survivor in the stack: a test where the right answer is also the
    /// topmost one is not testing the walk.
    #[test]
    fn a_press_where_a_closed_window_used_to_be_reaches_what_is_drawn_there() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (kept, survivor) =
            opened_at(&mut display, &mut state, &conn, &client, &qh, (1000, 300));
        let (doomed, closing) =
            opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );

        // Past both windows' opening animations, and *settled*, which is
        // what the render loop does once a frame. Two reasons, and the test
        // needs both. `present::open` starts at opacity 0 and eases up, so
        // a window in its first frame is genuinely not on screen yet and
        // these walks correctly decline it. And `open`'s target was
        // captured before `opened_at` moved the window, so until the
        // released transform is retired the pane is still drawn at the
        // rectangle it mapped at rather than the one it now lives at.
        state.clock.advance(Duration::from_millis(300));
        state.settle(state.clock.now());

        let vacated = state
            .pane_outer_of(closing)
            .expect("a mapped pane has a rectangle");
        let was = state
            .pane_outer_of(survivor)
            .expect("a mapped pane has a rectangle");
        // The middle of the window the user is about to close, which is
        // where the window that replaces it will be too.
        let point = Point::<f64, Logical>::from((
            f64::from(vacated.loc.x) + f64::from(vacated.size.w) / 2.0,
            f64::from(vacated.loc.y) + f64::from(vacated.size.h) / 2.0,
        ));

        // The premise. If the closing window did not own this point to
        // begin with, nothing below is about anything.
        assert_eq!(
            state.window_under(point).map(|(window, _)| window),
            Some(doomed.clone()),
            "before the close, the point belongs to the window that is \
             about to be closed"
        );

        state.close_pane(closing);
        // Past `CLOSING`: the animation has landed, the request has gone
        // out, and the fixture's client never destroys anything -- which is
        // what a client still running its quit handlers looks like from
        // here.
        state
            .clock
            .advance(present::CLOSING + Duration::from_millis(10));
        let asked = state.clock.now();
        state.settle_closing(asked);
        assert_eq!(client.closes.len(), 0, "the client has not answered yet");

        // The layout reflows into the space, which is the ordinary
        // consequence of a close and the reason the stale rectangle matters
        // at all.
        state.move_pane(
            survivor,
            vacated,
            was,
            AnimationSpec::default(),
            asked,
            Standing::Tile,
        );
        // Past the survivor's own move, so what is drawn at the point is
        // the survivor itself rather than a frame of it in transit.
        state.clock.advance(Duration::from_millis(400));

        // The premise for the second half: the closing pane is invisible
        // and its rectangle still covers the point. Without this the walk
        // could be answering correctly for the wrong reason.
        let dead = drawn_now(&state, closing, state.clock.now());
        assert!(
            dead.opacity.abs() < f32::EPSILON && dead.rect.contains(point),
            "the closing pane is held at opacity 0 over the point -- \
             opacity {}, rect {:?}. That is the situation under test",
            dead.opacity,
            dead.rect
        );

        assert_eq!(
            state.window_under(point).map(|(window, _)| window),
            Some(kept.clone()),
            "a click where a closed window used to be belongs to the \
             window that is drawn there now. `window_under` is what \
             click-to-focus focuses, so answering the dead window sends \
             every keystroke after the click into a window that is not on \
             screen"
        );

        let surface = state
            .surface_under(point)
            .map(|(surface, _)| surface)
            .expect("the survivor is drawn at this point and has a surface");
        assert_eq!(
            Some(&surface),
            kept.wl_surface().as_deref(),
            "and the pointer event is delivered to that window's surface, \
             not to the dead one's"
        );

        // The compositor's own chrome, by the same rule: an invisible
        // titlebar has no buttons and an invisible edge cannot be dragged.
        // A `Halo` would be wrong here too, which is why `chrome_offered`
        // is gated and not only `covers`.
        assert!(
            state
                .chrome_under(point)
                .is_none_or(|under| under.pane != closing),
            "no chrome of the closed window is under the point either"
        );
    }

    /// **#127 review, finding 2: the guard covered the transform and left
    /// the client-facing half of a placement running.**
    ///
    /// `move_pane` suppressed `present::from` for a leaving pane and went on
    /// calling `offers_size`, `size_window` and `map_stacked`. The configure
    /// is the visible one: it asks a client that is tearing itself down to
    /// re-lay-out at a size nobody will ever see, and if the client answers,
    /// `real_geometry` moves under a `frame.rect` that `present::close` has
    /// pinned — which is exactly the pair `resizing::factor` divides, so the
    /// dying buffer is stretched to fill a rectangle it was never painted
    /// for, mid-fade.
    ///
    /// Counted at the client, for the reason `Client::configures` gives:
    /// from the server's own side a compositor that sends a configure looks
    /// identical to one that does not.
    #[test]
    fn a_layout_sweep_does_not_configure_a_window_that_is_closing() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (_window, pane) = opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );

        let was = state
            .pane_outer_of(pane)
            .expect("a mapped pane has a rectangle");
        let pressed = state.clock.now();
        state.close_pane(pane);

        let before = client.configures.len();
        // A different size, not just a different place: `offers_size`
        // deduplicates on the whole rectangle, so a sweep that only moved
        // the pane would send nothing even without the guard and the test
        // would pass against the bug.
        state.move_pane(
            pane,
            at(1000, 300, 250, 180),
            was,
            AnimationSpec::default(),
            pressed + Duration::from_millis(95),
            Standing::Tile,
        );
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );

        assert_eq!(
            client.configures.len(),
            before,
            "a client that has been asked to close is not asked to \
             re-lay-out at a size that will never be drawn"
        );

        // And the half that must keep working, for the same reason the
        // sweep test asserts it: only what a client can observe is
        // suppressed, and #124 reads `placed`.
        let placed = state
            .panes
            .get(pane)
            .and_then(Pane::placed)
            .expect("the sweep placed this pane");
        assert_eq!(
            placed,
            at(1000, 300, 250, 180),
            "the layout's own answer is still written for a leaving pane"
        );
    }

    /// **#127 review, finding 3: `GRACE` inverts for the client that
    /// refuses on purpose.**
    ///
    /// The grace period was lengthened on the argument that too long only
    /// makes a genuinely refused window wait. That holds for the
    /// honest-but-slow client and reverses for this one: "save your changes
    /// before closing?" is a refusal delivered as a question, and under a
    /// flat deadline the parent was a hole for the whole second with the
    /// dialog floating over nothing to read. A second `super+q` could not
    /// clear it either — correctly, since `Pane::leaving` declines to start
    /// a second close on a pane already in one.
    ///
    /// A new window parented to the one being closed is an answer, and it
    /// is the safe kind to act on: being wrong gives a window back that was
    /// going to leave anyway. See `Solium::refused_with_a_dialog`.
    ///
    /// **Asserted well inside `GRACE`**, which is the whole claim. Sampling
    /// after it would pass against the plain deadline and test nothing.
    #[test]
    fn a_window_that_answers_a_close_with_a_dialog_comes_straight_back() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        // `opened_at`'s body, inlined for the one thing it discards: the
        // client-side toplevel proxy, which is the only end `set_parent`
        // can be sent from.
        let (parent, parent_toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
        state.map_stacked(parent.clone(), (400, 300), false);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&parent)
            .expect("a client in the space has a pane");
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );

        state.close_pane(pane);
        state
            .clock
            .advance(present::CLOSING + Duration::from_millis(10));
        let asked = state.clock.now();
        state.settle_closing(asked);
        assert!(
            state
                .panes
                .get(pane)
                .is_some_and(|pane| pane.asked_at().is_some()),
            "the request has gone out and the client has not answered"
        );
        assert!(
            drawn_now(&state, pane, asked).opacity.abs() < f32::EPSILON,
            "and the window is invisible, which is the hole the dialog \
             would otherwise float over"
        );

        // The client's answer: not a destroy, a dialog. `set_parent` is
        // what says the dialog is about *this* window, and it is the
        // request `parent_changed` fires on.
        let (dialog, dialog_toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
        dialog_toplevel.set_parent(Some(&parent_toplevel));
        conn.flush().expect("flushing set_parent");
        display
            .dispatch_clients(&mut state)
            .expect("dispatching set_parent");

        // The fixture is doing what it claims: the compositor read the
        // parent back, so this is a dialog about the window being closed
        // and not merely another window.
        assert_eq!(
            state.parent_of(&dialog),
            Parentage::Window(pane.get()),
            "the client called set_parent and the compositor did not read \
             it back, so nothing below is about a dialog for this window"
        );

        let back = drawn_now(&state, pane, state.clock.now() + Duration::from_millis(200));
        assert!(
            (back.opacity - 1.0).abs() < f32::EPSILON,
            "a window whose client answered with a dialog is back on \
             screen without waiting out the grace period: it was drawn at \
             opacity {}",
            back.opacity
        );
        assert!(
            state.panes.get(pane).is_some_and(|pane| !pane.leaving()),
            "and it is no longer leaving, so a second super+q can close it"
        );
    }

    /// **#127 third review, finding 1: an X11 tooltip could cancel a
    /// close.**
    ///
    /// The rule is `refused_with_a_dialog`'s own: a window that places
    /// itself — a menu, a tooltip, a splash, a notification, an
    /// override-redirect window — appearing over a window that is closing
    /// says nothing about whether the close was refused. It stood as a
    /// comment at one of the three call sites, and the `TransientFor` hook
    /// added beside it did not repeat it. Every one of those windows is in
    /// `self.space` and receives `PROPERTY_CHANGE`, so any of them that set
    /// `WM_TRANSIENT_FOR` after mapping brought its parent back: `super+q`,
    /// the window fades out, returns, and never closes.
    ///
    /// **Asserted at rule level, and the limit is stated rather than
    /// implied.** An `X11Surface` cannot be built without a live XWayland
    /// and nothing in this suite has one, so the X11 event that carries the
    /// case cannot be produced here. What *can* be produced is the fact the
    /// gate turns on, which is not an X11 fact at all: `Pane::managed`,
    /// false for exactly those windows and set in one place —
    /// `take_unmanaged_pane`, which both of XWayland's self-placing branches
    /// call. So the child here is given the unmanaged pane a tooltip gets,
    /// and driven through the one caller this fixture can drive. That pins
    /// the gate; it does not pin the X11 plumbing above it.
    #[test]
    fn a_window_that_places_itself_does_not_cancel_a_close() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (parent, parent_toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
        state.map_stacked(parent.clone(), (400, 300), false);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&parent)
            .expect("a client in the space has a pane");
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );

        state.close_pane(pane);
        state
            .clock
            .advance(present::CLOSING + Duration::from_millis(10));
        let asked = state.clock.now();
        state.settle_closing(asked);
        assert!(
            state
                .panes
                .get(pane)
                .is_some_and(|pane| pane.asked_at().is_some()),
            "the request has gone out and the client has not answered, \
             which is the state a tooltip must not end"
        );

        // The tooltip. Everything about it is ordinary except its pane,
        // which is the whole of what makes it one.
        let (tip, tip_toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
        let tip_pane = state
            .panes
            .id_of(&tip)
            .expect("a client in the space has a pane");
        if let Some(unmanaged) = state.panes.get_mut(tip_pane) {
            unmanaged.unmanage();
        }
        assert!(
            state
                .panes
                .get(tip_pane)
                .is_some_and(|pane| !pane.managed()),
            "the premise: this child holds the unmanaged pane every menu, \
             tooltip, splash and override-redirect window is given, and \
             without it this test is about an ordinary dialog"
        );

        tip_toplevel.set_parent(Some(&parent_toplevel));
        conn.flush().expect("flushing set_parent");
        display
            .dispatch_clients(&mut state)
            .expect("dispatching set_parent");

        // The second premise: the compositor really did read the parent
        // back, so the gate is what declined and not a parent nobody found.
        assert_eq!(
            state.parent_of(&tip),
            Parentage::Window(pane.get()),
            "the tooltip named the closing window as its parent and the \
             compositor did not read it back, so nothing below is about \
             the gate"
        );

        assert!(
            state.panes.get(pane).is_some_and(Pane::leaving),
            "a window that places itself is not an answer to anything: the \
             close must still be running"
        );
        let still = drawn_now(&state, pane, state.clock.now() + Duration::from_millis(200));
        assert!(
            still.opacity.abs() < f32::EPSILON,
            "and the closing window must still be held invisible rather \
             than faded back up by a tooltip: it was drawn at opacity {}",
            still.opacity
        );
    }

    /// **#127 third review, finding 3: `refused_with_a_dialog` threw away
    /// `give_back`'s answer.**
    ///
    /// `give_back` reports whether `present::clear` took, because
    /// `with_slot` declines rather than panics when the transform slot is
    /// already borrowed. On a declined frame nothing is retired — and
    /// inside `CLOSING` there is no `asked_at`, so `settle_refused` is not
    /// looking at this pane and never will be. The close then ran to its
    /// deadline and `settle_closing` sent the request, closing the parent
    /// out from under the dialog that had just answered for it.
    ///
    /// **The observation is at the client, because that is the only end
    /// that can tell.** `send_close` is a call into smithay and a
    /// compositor that made it looks, from its own side, exactly like one
    /// that did not. See [`Client::closes`].
    ///
    /// **What the jam costs this test, said plainly.** `present::jam_slot`
    /// is one-way, so the give-back retries for ever here and there is no
    /// frame on which it succeeds; `present::frame` also falls back to real
    /// geometry while the slot is busy, so opacity says nothing either.
    /// What this asserts is the half that was actually lost — the request
    /// that must not go out, and the close staying owed rather than
    /// forgotten. The other half, a give-back on a free slot bringing the
    /// window back, is
    /// `a_window_that_answers_a_close_with_a_dialog_comes_straight_back`.
    #[test]
    fn a_dialog_whose_give_back_is_declined_does_not_lose_its_parent() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (parent, parent_toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
        state.map_stacked(parent.clone(), (400, 300), false);
        state.sync_panes();
        let pane = state
            .panes
            .id_of(&parent)
            .expect("a client in the space has a pane");
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );

        state.close_pane(pane);
        // Inside `CLOSING`, which is where a file chooser arrives and is
        // the state `settle_refused` cannot see: the request has not gone
        // out, so there is no `asked_at` for it to be due on.
        state.clock.advance(Duration::from_millis(50));
        assert!(
            state
                .panes
                .get(pane)
                .is_some_and(|pane| pane.closing_at().is_some() && pane.asked_at().is_none()),
            "the premise: the animation is playing and the request has not \
             gone out, which is the only window in which this fault exists"
        );

        // The busy frame. Nothing un-jams this, which is why it is taken
        // after the close has started and before the dialog arrives.
        if let Some(busy) = state.panes.get(pane) {
            present::jam_slot(busy);
        }

        let (dialog, dialog_toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
        dialog_toplevel.set_parent(Some(&parent_toplevel));
        conn.flush().expect("flushing set_parent");
        display
            .dispatch_clients(&mut state)
            .expect("dispatching set_parent");
        assert_eq!(
            state.parent_of(&dialog),
            Parentage::Window(pane.get()),
            "the client called set_parent and the compositor did not read \
             it back, so nothing below is about a dialog for this window"
        );

        // Past the deadline the request would go out on.
        state
            .clock
            .advance(present::CLOSING + Duration::from_millis(10));
        let due = state.clock.now();
        let still_going = state.settle_closing(due);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );

        assert!(
            client.closes.is_empty(),
            "a client that answered a close with a dialog was asked to \
             close anyway, on the frame its give-back was declined: the \
             parent is shut out from under its own prompt"
        );
        assert!(
            still_going,
            "and the close is still owed, so the backend keeps drawing and \
             the give-back is retried -- a deadline that answered false \
             here would strand the window instead"
        );
        assert!(
            state.panes.get(pane).is_some_and(Pane::leaving),
            "the pane is still leaving, so a second super+q cannot start a \
             second close over the top of this one"
        );

        // And it is a retry rather than one reprieve: another frame, and
        // the request still does not go out.
        state.clock.advance(Duration::from_millis(100));
        let later = state.clock.now();
        assert!(
            state.settle_closing(later),
            "the retry is still live a frame later"
        );
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert!(
            client.closes.is_empty(),
            "the reprieve lasts as long as the give-back is owed, rather \
             than for one frame"
        );
    }

    /// **#127 third review, finding 2: focus could land on a window nobody
    /// can see.**
    ///
    /// `settle_focus`'s topmost arm was taught to skip a pane that is
    /// `leaving` and nothing else, while the pointer arm it falls back from
    /// goes through `window_under` and so asks `Frame::covers` — opacity
    /// *and* the rectangle. A hidden workspace is parked a screen away
    /// rather than unmapped (`workspaces.lua`: a switch moves the view, not
    /// the windows), so its panes are perfectly visible to a filter that
    /// only asks about the close.
    ///
    /// Closing the only window on the workspace in view therefore handed
    /// the keyboard to a desk the user cannot see, and with it
    /// `focus_window`'s `trigger_focus`, which is what a workspace script
    /// acts on. The window that then refused to close came back to a
    /// session where `give_back`'s `settle_focus` declines — something is
    /// focused — so it was permanent. That is #127's own symptom by a
    /// second route.
    ///
    /// **Asserted at the client**, for the reason
    /// `typing_after_a_close_reaches_the_window_that_is_drawn` gives: from
    /// the compositor's own side a seat holding an off-stage surface looks
    /// exactly like one holding a visible one. Here the right answer is
    /// that the keystroke goes *nowhere* — an idle keyboard loses no
    /// characters to the wrong application. This test stops at the
    /// request; that the window which then comes back takes the keyboard is
    /// `a_refused_close_gives_the_keyboard_back_to_the_window_it_brings_back`'s
    /// to show, and until #127's fourth review nothing did.
    #[test]
    fn a_close_does_not_hand_the_keyboard_to_a_workspace_nobody_can_see() {
        tiled_fixture!(display, state, conn, queue, client, qh);

        // One monitor, because "off screen" is a question with no answer
        // without one -- `nothing_on_stage` says so itself.
        let screen = Output::new(
            "hidden-desk-test".to_string(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "solium".to_string(),
                model: "test".to_string(),
            },
        );
        screen.change_current_state(
            Some(Mode {
                size: (1920, 1080).into(),
                refresh: 60_000,
            }),
            None,
            Some(Scale::Fractional(1.0)),
            None,
        );
        state.space.map_output(&screen, (0, 0));

        let (parked, parked_pane) =
            opened_at(&mut display, &mut state, &conn, &client, &qh, (1000, 300));
        let (visible, closing) =
            opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
        // Twice, for the reason `typing_after_a_close...` gives: the
        // keyboard is a request the client makes in answer to the seat's
        // capabilities.
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert!(
            client.keyboard.is_some(),
            "the client bound a keyboard; without one this test cannot \
             observe anything"
        );

        // Past both opening animations and settled, so no pane is still
        // drawn back near the origin where the untouched pointer is.
        state.clock.advance(Duration::from_millis(300));
        state.settle(state.clock.now());
        assert!(
            state.window_under((0.0, 0.0).into()).is_none(),
            "the pointer has not been moved and there is nothing under it, \
             which is what makes this the keyboard's question"
        );

        // Workspace 2 goes away: one selection, carried a screen and a bit
        // to the left. `workspaces.lua`'s own arrangement and its own
        // numbers.
        let desk = state.window_id(&parked);
        let switch = state.clock.now();
        state.groups.declare(
            "desk-2",
            crate::group::Selection {
                members: vec![crate::group::Member::Window(desk)],
                on: None,
            },
            switch,
        );
        state.groups.present(
            "desk-2",
            crate::group::Shift {
                dx: -1920.0 * 1.06,
                ..crate::group::Shift::NONE
            },
            switch,
            Duration::from_millis(300),
            present::Curve::OutCubic,
        );
        state.clock.advance(Duration::from_millis(400));
        state.settle(state.clock.now());

        // The premise, both halves of it: the parked window is fully
        // opaque -- it is not hidden by being faded out, which the
        // `shows()` half of the gate would have caught on its own -- and
        // its own rectangle is still on the monitor, because a hidden
        // workspace is parked rather than unmapped. Only the *drawn*
        // rectangle knows it is gone.
        let landed = state.clock.now();
        let away = drawn_now(&state, parked_pane, landed);
        assert!(
            (away.opacity - 1.0).abs() < f32::EPSILON,
            "the parked window is fully opaque, so opacity alone cannot be \
             what declines it"
        );
        let real = state
            .pane_outer_of(parked_pane)
            .expect("a mapped pane has a rectangle");
        assert!(
            state.on_any_output(real),
            "and it still lives on the monitor -- a workspace switch moves \
             the view, not the windows -- so `pane_outer` cannot be what \
             declines it either"
        );
        let screens: Vec<Rectangle<i32, Logical>> = state
            .space
            .outputs()
            .filter_map(|output| state.space.output_geometry(output))
            .collect();
        assert_eq!(
            nothing_on_stage([(real, away.rect)], &screens),
            Some(true),
            "the parked window is drawn off every monitor, which is the \
             one thing about it that is true"
        );

        // The window the user is working in, and the premise that this
        // fixture can see where typing goes at all.
        state.focus_window(&visible, SERIAL_COUNTER.next_serial());
        types(&mut state, KEY_A);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(
            client.typed,
            vec![(Some(surface_id(&visible)), KEY_A)],
            "the premise: typing reaches the focused window"
        );

        // And the user closes the only window they can see.
        state.close_pane(closing);
        state
            .clock
            .advance(present::CLOSING + Duration::from_millis(10));
        let asked = state.clock.now();
        state.settle_closing(asked);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );

        // The client first, because it is the only end that can tell, and
        // the compositor's own view of the seat second.
        let before = client.typed.len();
        types(&mut state, KEY_A);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(
            client.typed.len(),
            before,
            "a keystroke after the last visible window closed goes \
             nowhere, rather than onto a workspace the user cannot see: it \
             reached {:?}, and the parked window's surface is {}",
            client.typed.last(),
            surface_id(&parked)
        );
        assert!(
            state.focused_window().is_none(),
            "and the seat is holding nothing at all -- it must, because \
             `give_back`'s `settle_focus` declines when something already \
             has focus, which is what would make this permanent rather \
             than a wrong answer for one second"
        );
    }

    /// Whether any of this pane's drawn rectangle at `at` reaches a screen,
    /// and whether it paints anything there — the two halves of "on stage",
    /// measured the way the premises below need them measured.
    fn drawn_on_stage(state: &Solium, pane: crate::pane::PaneId, at: Duration) -> bool {
        let frame = drawn_now(state, pane, at);
        let slot = state
            .pane_outer_of(pane)
            .expect("a mapped pane has a rectangle");
        let screens: Vec<Rectangle<i32, Logical>> = state
            .space
            .outputs()
            .filter_map(|output| state.space.output_geometry(output))
            .collect();
        frame.shows() && nothing_on_stage([(slot, frame.rect)], &screens) == Some(false)
    }

    /// **#127 fourth review, NEW-1: a refused window came back with the
    /// keyboard on nothing.**
    ///
    /// `give_back` calls `settle_focus` straight after `present::clear`, so
    /// the restore it has just started is at progress zero, and at progress
    /// zero a transform answers its `from` — which past `CLOSING` is
    /// `present::close`'s opacity-zero end. Judged by the frame being
    /// drawn, the window being given back was invisible, so the topmost arm
    /// declined the only window there was to focus, and the window stood
    /// back up at full opacity with the seat holding nothing.
    ///
    /// **Deterministic, where the fault was not.** In the running
    /// compositor `settle_focus` reads the clock a little after the frame's
    /// `now`, and the window escaped whenever enough real time had passed
    /// between the two for the fade to clear one step of eight bits. This
    /// hands `settle_refused` an instant the clock has not reached, so
    /// every reading `settle_focus` takes falls at or before the restore's
    /// start, where `Animation::progress` is exactly zero. The failing case,
    /// made certain rather than likely.
    ///
    /// Nothing else is open, so `hand_off_keyboard` empties the seat at the
    /// request — asserted, because that is the path the fault needs.
    /// Asserted at the client as well as the seat, for the reason
    /// `typing_after_a_close_reaches_the_window_that_is_drawn` gives.
    #[test]
    fn a_refused_close_gives_the_keyboard_back_to_the_window_it_brings_back() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        one_screen(&mut state);
        let (window, pane) = opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
        // Twice: the keyboard is a request the client makes in answer to
        // the seat's capabilities. See `typing_after_a_close...`.
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert!(
            client.keyboard.is_some(),
            "the client bound a keyboard; without one this test cannot \
             observe anything"
        );
        // Past the opening animation and settled, so the untouched pointer
        // at the origin is over nothing and the topmost arm is the one
        // asked.
        state.clock.advance(Duration::from_millis(300));
        state.settle(state.clock.now());
        assert!(
            state.window_under((0.0, 0.0).into()).is_none(),
            "the pointer is over nothing, which makes this the keyboard's \
             question and not the mouse's"
        );

        state.focus_window(&window, SERIAL_COUNTER.next_serial());
        state.close_pane(pane);
        state
            .clock
            .advance(present::CLOSING + Duration::from_millis(10));
        let asked = state.clock.now();
        state.settle_closing(asked);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(client.closes.len(), 1, "the request went out");
        assert!(
            state.focused_window().is_none(),
            "the premise: with nothing else open, handing the keyboard off \
             at the request leaves the seat holding nothing"
        );

        // The refusal, at an instant the clock has not reached. Past the
        // grace period, so the window is due.
        let refused = asked + Duration::from_millis(1500);
        state.settle_refused(refused);
        assert!(
            !drawn_now(&state, pane, state.clock.now()).shows(),
            "the premise: at every instant `settle_focus` could have read, \
             the window being given back is still drawn at nothing"
        );
        assert!(
            drawn_on_stage(&state, pane, refused + Duration::from_millis(200)),
            "and it is on its way back: once the restore lands it is on \
             screen and opaque"
        );

        assert_eq!(
            state.focused_window().as_ref(),
            Some(&window),
            "a window given back to a session whose keyboard is idle takes \
             the keyboard, even on the frame its restore starts"
        );
        let before = client.typed.len();
        types(&mut state, KEY_A);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(
            client.typed.get(before),
            Some(&(Some(surface_id(&window)), KEY_A)),
            "and typing reaches it, rather than going nowhere until the \
             user clicks"
        );
    }

    /// How far a hidden desk is carried: a screen and a bit to the left,
    /// `workspaces.lua`'s own arrangement and its own numbers.
    const AWAY: f64 = -1920.0 * 1.06;

    /// Two desks with one window each, and a switch between them on its
    /// first frame.
    ///
    /// Returns `(leaving, arriving)`. `leaving` is on the desk in view and
    /// is opened second, so it is the topmost pane: a walk whose right
    /// answer is also the topmost one is not testing the walk. `arriving`
    /// starts parked `AWAY` and settled there.
    ///
    /// **The switch starts at an instant the clock has not reached**, so
    /// every reading `settle_focus` takes falls at or before its start,
    /// where `Animation::progress` is exactly zero — the first frame, made
    /// certain rather than likely. Its premises are asserted here, both
    /// ends of them: on that frame `leaving` is drawn on stage and
    /// `arriving` off it, and once the switch lands it is the other way
    /// round. Focus that follows the drawn frame and focus that follows the
    /// destination give different answers only because of this.
    ///
    /// The seat is emptied by hand at the end. How it came to be empty is
    /// not the question — a close, a lock, a window going — what
    /// `settle_focus` does about it is.
    fn a_switch_on_its_first_frame(
        display: &mut Display<Solium>,
        state: &mut Solium,
        conn: &Connection,
        qh: &QueueHandle<Client>,
        queue: &mut wayland_client::EventQueue<Client>,
        client: &mut Client,
    ) -> ((Window, crate::pane::PaneId), (Window, crate::pane::PaneId)) {
        one_screen(state);
        let arriving = opened_at(display, state, conn, client, qh, (1000, 300));
        let leaving = opened_at(display, state, conn, client, qh, (400, 300));
        pump(display, state, conn, qh, queue, client);
        pump(display, state, conn, qh, queue, client);
        assert!(
            client.keyboard.is_some(),
            "the client bound a keyboard; without one this test cannot \
             observe anything"
        );
        state.clock.advance(Duration::from_millis(300));
        state.settle(state.clock.now());

        let parked = state.clock.now();
        for (name, window) in [("desk-1", &leaving.0), ("desk-2", &arriving.0)] {
            let id = state.window_id(window);
            state.groups.declare(
                name,
                crate::group::Selection {
                    members: vec![crate::group::Member::Window(id)],
                    on: None,
                },
                parked,
            );
        }
        state.groups.present(
            "desk-2",
            crate::group::Shift {
                dx: AWAY,
                ..crate::group::Shift::NONE
            },
            parked,
            Duration::from_millis(300),
            present::Curve::OutCubic,
        );
        state.clock.advance(Duration::from_millis(400));
        state.settle(state.clock.now());

        let switch = state.clock.now() + Duration::from_millis(50);
        state.groups.present(
            "desk-1",
            crate::group::Shift {
                dx: AWAY,
                ..crate::group::Shift::NONE
            },
            switch,
            Duration::from_millis(300),
            present::Curve::OutCubic,
        );
        state.groups.present(
            "desk-2",
            crate::group::Shift::NONE,
            switch,
            Duration::from_millis(300),
            present::Curve::OutCubic,
        );

        let first = state.clock.now();
        let landed = switch + Duration::from_millis(400);
        assert!(
            drawn_on_stage(state, leaving.1, first) && !drawn_on_stage(state, arriving.1, first),
            "the premise: on the switch's first frame the desk being left is \
             still the one drawn"
        );
        assert!(
            !drawn_on_stage(state, leaving.1, landed) && drawn_on_stage(state, arriving.1, landed),
            "and once it lands the desk switched to is"
        );

        assert!(
            state.seat.get_keyboard().is_some(),
            "the fixture's seat has a keyboard; without one there is no \
             focus to be wrong about"
        );
        state.give_keyboard(None, SERIAL_COUNTER.next_serial());
        assert!(state.focused_window().is_none(), "the seat is empty");
        (leaving, arriving)
    }

    /// **#127 fourth review, NEW-2: on a workspace switch's first frame,
    /// focus judged by where the windows had been.**
    ///
    /// `on_stage` sampled the instant it was called, while
    /// `everything_is_off_stage` — the one other caller asking the same
    /// question — samples `SETTLED` ahead, because a group transform that
    /// has just started is at progress zero. So `settle_focus` inside the
    /// first frames of a switch handed the keyboard to the desk being left
    /// and passed over the desk being switched to. The pointer is over
    /// nothing, so this is the topmost arm.
    #[test]
    fn the_first_frame_of_a_workspace_switch_focuses_the_desk_switched_to() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let ((leaving, _), (arriving, _)) = a_switch_on_its_first_frame(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert!(
            state.window_under((0.0, 0.0).into()).is_none(),
            "the pointer is over nothing, so the topmost arm is the one \
             asked"
        );

        state.settle_focus();
        assert_eq!(
            state.focused_window().as_ref(),
            Some(&arriving),
            "focus goes to the desk being switched to, not to the one being \
             left (surface {}), which is topmost and still drawn",
            surface_id(&leaving)
        );
        let before = client.typed.len();
        types(&mut state, KEY_A);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(
            client.typed.get(before),
            Some(&(Some(surface_id(&arriving)), KEY_A)),
            "and typing reaches it"
        );
    }

    /// **The pointer arm of the same question, and where the two rules
    /// meet.**
    ///
    /// `settle_focus` asks the window under the pointer first, because with
    /// focus-follows-mouse that is where focus would land the moment the
    /// pointer moved — and *the moment it moved* is after the switch has
    /// landed, not on this frame. So a pointer resting where the desk being
    /// left is drawn must not hand that desk the keyboard.
    ///
    /// **And the click on the same pixel still goes to what is drawn**,
    /// which is the half that must not change: `window_under` is the hit
    /// test, a press lands on what is on screen this frame, and on this
    /// frame that is the window being left. Both are asserted at the same
    /// point on the same frame, so a fix that moved the hit test instead
    /// fails here.
    #[test]
    fn a_pointer_over_the_desk_being_left_does_not_hand_it_the_keyboard() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let ((leaving, _), (arriving, _)) = a_switch_on_its_first_frame(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        // The middle of the window being left, as it is drawn this frame.
        let point: Point<f64, Logical> = (432.0, 332.0).into();
        let pointer = state.seat.get_pointer().expect(
            "the fixture's seat has a pointer; without one there is no \
             pointer arm to test",
        );
        pointer.motion(
            &mut state,
            None,
            &smithay::input::pointer::MotionEvent {
                location: point,
                serial: SERIAL_COUNTER.next_serial(),
                time: 0,
            },
        );
        pointer.frame(&mut state);
        assert_eq!(
            state.window_under(point).map(|(window, _)| window).as_ref(),
            Some(&leaving),
            "a press on this pixel this frame lands on the window drawn \
             there, which is the desk being left -- the hit test judges \
             the present frame, and that is right"
        );

        state.settle_focus();
        assert_eq!(
            state.focused_window().as_ref(),
            Some(&arriving),
            "but the keyboard goes where the desks are settling: the window \
             under the pointer is sliding off stage (surface {}), so the \
             desk switched to takes it",
            surface_id(&leaving)
        );
    }

    /// **A closing window under the pointer, while it is still fading.**
    ///
    /// The case [`SETTLED`] lists first. `settle_focus`'s pointer arm used
    /// to get "never a window on its way out" from `window_under`, which
    /// declines a pane only once it shows nothing — so for the 190 ms a
    /// close is still playing, a seat left empty by anything else handed
    /// the keyboard to the window being closed, if the pointer was resting
    /// on it. The topmost arm's `leaving()` filter never reached it: the
    /// pointer arm answers first.
    ///
    /// Judged at the destination, a closing pane is at opacity zero from
    /// the press, so it is no candidate; and the same press on the same
    /// frame still lands on it, because it is still drawn and a half-faded
    /// window keeps its clicks. Both are asserted.
    #[test]
    fn a_window_mid_close_under_the_pointer_is_not_handed_the_keyboard() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        one_screen(&mut state);
        let (kept, _) = opened_at(&mut display, &mut state, &conn, &client, &qh, (1000, 300));
        let (doomed, closing) =
            opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        state.clock.advance(Duration::from_millis(300));
        state.settle(state.clock.now());

        let point: Point<f64, Logical> = (432.0, 332.0).into();
        let pointer = state.seat.get_pointer().expect(
            "the fixture's seat has a pointer; without one there is no \
             pointer arm to test",
        );
        pointer.motion(
            &mut state,
            None,
            &smithay::input::pointer::MotionEvent {
                location: point,
                serial: SERIAL_COUNTER.next_serial(),
                time: 0,
            },
        );
        pointer.frame(&mut state);

        // A quarter of the way into the fade: drawn, visibly, and still
        // taking its clicks.
        state.close_pane(closing);
        state.clock.advance(Duration::from_millis(50));
        assert!(
            drawn_now(&state, closing, state.clock.now()).shows(),
            "the premise: the window being closed is still on screen"
        );
        assert_eq!(
            state.window_under(point).map(|(window, _)| window).as_ref(),
            Some(&doomed),
            "and a press on it this frame lands on it, which is right"
        );

        assert!(
            state.seat.get_keyboard().is_some(),
            "the fixture's seat has a keyboard; without one there is no \
             focus to be wrong about"
        );
        state.give_keyboard(None, SERIAL_COUNTER.next_serial());
        state.settle_focus();
        assert_eq!(
            state.focused_window().as_ref(),
            Some(&kept),
            "but the keyboard is not handed to a window that is on its way \
             out (surface {}): the one staying takes it",
            surface_id(&doomed)
        );
    }

    /// The protocol id of a window's surface.
    ///
    /// The one number the two ends of this fixture share. A `wl_surface` is
    /// created by the client, so the id it picked is the id the server
    /// knows it by, and a test holding the server's `Window` can say which
    /// surface a `wl_keyboard.enter` named without threading the client's
    /// proxy through every helper.
    fn surface_id(window: &Window) -> u32 {
        window
            .wl_surface()
            .expect("a mapped client has a surface")
            .id()
            .protocol_id()
    }

    /// One keystroke, through the same `KeyboardHandle::input` that
    /// `crate::input::keyboard` ends in.
    ///
    /// `Forward` unconditionally: whether a combination is a binding is
    /// `input::keyboard`'s question and not this one's.
    ///
    /// `key` is the evdev code, which is what an input backend reports and
    /// what the client is sent. The `+ 8` in the middle is the X11 offset
    /// libxkbcommon works in and smithay unwinds again on the wire, so the
    /// number that goes in here is the number that comes out at the other
    /// end — and a test asserting on a different one would be asserting on
    /// this fixture's arithmetic rather than on the compositor's.
    fn types(state: &mut Solium, key: u32) {
        let keyboard = state.seat.get_keyboard().expect(
            "the fixture's seat has a keyboard; without one there is no \
             typing to be wrong about",
        );
        keyboard.input::<(), _>(
            state,
            smithay::input::keyboard::Keycode::new(key + 8),
            smithay::backend::input::KeyState::Pressed,
            SERIAL_COUNTER.next_serial(),
            0,
            |_, _, _| smithay::input::keyboard::FilterResult::Forward,
        );
    }

    /// `KEY_A`, as evdev and the client both spell it. See [`types`].
    const KEY_A: u32 = 30;

    /// **#127 second review, finding 1: the keystroke fix only worked via a
    /// click.**
    ///
    /// The review before this one stopped an invisible closing pane from
    /// winning `window_under`, and `window_under` is what click-to-focus
    /// focuses — so a press where the dead window used to be goes to the
    /// window that is drawn there, and the typing after it follows. That is
    /// the whole of the fix, and it requires the user to touch the mouse.
    ///
    /// **Nothing moved focus otherwise.** `close_pane` does not, and
    /// `settle_focus` runs from `sync_panes` only when the pane set
    /// changes, which a close that has been asked and not yet answered does
    /// not do. So `super+q` and carry on typing — the ordinary way anyone
    /// meets this — put every character into a window at opacity zero for
    /// the 190 ms animation *and* the 1000 ms grace after it, while the
    /// sibling that reflowed into the space sat on screen looking like the
    /// thing being typed into.
    ///
    /// **Asserted at the client, which is the only end that can tell.**
    /// From the compositor's own side a seat holding an invisible surface
    /// looks exactly like one holding a visible one; `give_keyboard`'s
    /// `true` says only that the lock's rule allowed the surface, and
    /// nothing complains about opacity. So this binds a `wl_keyboard` and
    /// asks where the key came out. See [`Client::typed`].
    ///
    /// The premise is typed first, before the close: a test whose second
    /// half passes because the keyboard was never wired at all would be
    /// worth nothing, and this makes that failure loud instead.
    #[test]
    fn typing_after_a_close_reaches_the_window_that_is_drawn() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (kept, _survivor) =
            opened_at(&mut display, &mut state, &conn, &client, &qh, (1000, 300));
        let (doomed, closing) =
            opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
        // Twice: the first round trip carries the seat's capabilities to
        // the client, and `get_keyboard` is a request it makes in answer to
        // them, so the keyboard does not exist on the server until the
        // second.
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert!(
            client.keyboard.is_some(),
            "the client bound a keyboard; without one this test cannot \
             observe anything"
        );

        // Past both opening animations, and *settled*, which is what the
        // render loop does once a frame.
        //
        // Not housekeeping: `present::open`'s target was captured before
        // `opened_at` moved each window, so until that released transform
        // is retired a pane is still drawn near the origin — where the
        // pointer is sitting, having never been moved. `settle_focus` asks
        // `window_under` first, so without this it answers from the pointer
        // and the topmost-pane arm that this test is about is never
        // reached. It passed that way, for a reason that had nothing to do
        // with the fix.
        state.clock.advance(Duration::from_millis(300));
        state.settle(state.clock.now());
        assert!(
            state.window_under((0.0, 0.0).into()).is_none(),
            "the pointer has not been moved and there is nothing under it, \
             which is what makes this the keyboard's question and not the \
             mouse's"
        );

        // The window the user is working in, which is the one they are
        // about to close.
        state.focus_window(&doomed, SERIAL_COUNTER.next_serial());
        types(&mut state, KEY_A);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(
            client.typed,
            vec![(Some(surface_id(&doomed)), KEY_A)],
            "the premise: typing reaches the focused window, and this \
             fixture can see it happen"
        );

        state.close_pane(closing);
        // Past `CLOSING`: the animation has landed, the request has gone
        // out, and the fixture's client never destroys anything — a client
        // still running its quit handlers, which is what the longer grace
        // period exists for.
        state
            .clock
            .advance(present::CLOSING + Duration::from_millis(10));
        let asked = state.clock.now();
        state.settle_closing(asked);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );

        // The situation under test, stated rather than assumed: the window
        // that was closed is drawn at nothing, and the user has not touched
        // the mouse — there has been no press for `window_under` to answer.
        assert!(
            drawn_now(&state, closing, asked).opacity.abs() < f32::EPSILON,
            "the closed window is invisible, which is what makes typing \
             into it silent"
        );

        // And the user keeps typing, which is the whole of the case.
        types(&mut state, KEY_A);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(
            client.typed.last(),
            Some(&(Some(surface_id(&kept)), KEY_A)),
            "a keystroke after a close belongs to the window that is drawn, \
             not to the one that was closed: it went to surface {:?}, and \
             the closed window's is {}",
            client.typed.last().and_then(|(surface, _)| *surface),
            surface_id(&doomed)
        );
    }

    /// **#127 second review, finding 2: suppressing `map_stacked` left the
    /// three copies of a pane's position disagreeing.**
    ///
    /// The guard `move_pane` needs is over the *configure*: a client that
    /// answers a new size while `present::close` holds `frame.rect` pinned
    /// gets its last buffer stretched mid-fade, because those two
    /// rectangles are the pair `resizing::factor` divides. `map_stacked`
    /// carries a location and no size, so it is no part of that — and
    /// suppressing it anyway cost what `move_pane`'s own opening paragraph
    /// warns about: *setting the slot without telling the space is undone
    /// before the next frame is drawn, silently*. `pane_geometry` reads
    /// `real_geometry` for a mapped client, so `sync_panes` copied the
    /// stale rectangle straight back over `set_slot`.
    ///
    /// **`sync_panes` is the line that made it a defect**, and a test that
    /// only looked at the frame `move_pane` returns on would pass against
    /// the bug. The sweep is a workspace switch, a `rescue_offscreen` or a
    /// config reload; the refusal is a client that goes on living. Between
    /// them the window came back where it had been *closed*, while
    /// `Pane::placed` said the layout had moved it — which is the pair
    /// `pane_laid_out` hands #124's edge drag.
    ///
    /// **And the last assertion is the one this replaces a comment with.**
    /// The client's *size* really is one configure behind while the pane is
    /// leaving, because that configure is deliberately never sent. The
    /// claim made for that is that the next sweep after the window comes
    /// back sends it — the suppression happened before `offers_size` could
    /// record the size as told, so re-placing at the same rectangle is
    /// still a change. That is asserted here rather than promised there.
    #[test]
    fn a_window_the_layout_moved_mid_close_comes_back_where_it_was_put() {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (window, pane) = opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );

        let was = state
            .pane_outer_of(pane)
            .expect("a mapped pane has a rectangle");
        state.close_pane(pane);
        state
            .clock
            .advance(present::CLOSING + Duration::from_millis(10));
        let asked = state.clock.now();
        state.settle_closing(asked);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(client.closes.len(), 1, "the request went out");

        // The sweep, mid-close. A different size as well as a different
        // place, so that the configure guard is still being exercised and
        // this is not quietly testing a move nobody suppressed.
        let elsewhere = at(1000, 500, 250, 180);
        let configures = client.configures.len();
        state.move_pane(
            pane,
            elsewhere,
            was,
            AnimationSpec::default(),
            asked,
            Standing::Tile,
        );
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(
            client.configures.len(),
            configures,
            "the leaving client is still told nothing, which is the guard \
             that belongs here"
        );

        // The next frame. This is where the defect lived: the space was
        // never told, so the space's stale rectangle came back over the
        // slot the sweep had just written.
        state.sync_panes();

        let outer = state
            .pane_outer_of(pane)
            .expect("a mapped pane has a rectangle");
        assert_eq!(
            outer.loc, elsewhere.loc,
            "a pane the layout moved during a close lives where the layout \
             put it, one frame later as well as on the frame it was moved"
        );
        let laid_out = state
            .pane_laid_out(&window)
            .expect("a pane the layout has placed answers this");
        assert_eq!(
            laid_out.0.loc, outer.loc,
            "and the rectangle #124's edge drag starts from is one the \
             window is actually at: `pane_laid_out` says {:?} and the pane \
             is at {:?}",
            laid_out.0.loc, outer.loc
        );

        // The refusal. The client never destroyed anything, so the deadline
        // is what brings the window back.
        let refused = asked + Duration::from_millis(1500);
        state.settle_refused(refused);
        let back = drawn_now(&state, pane, refused + Duration::from_millis(200));
        assert!(
            (back.opacity - 1.0).abs() < f32::EPSILON,
            "the premise for the rest: the window is back on screen, at \
             opacity {}",
            back.opacity
        );
        assert_eq!(
            state
                .pane_outer_of(pane)
                .expect("a mapped pane has a rectangle")
                .loc,
            elsewhere.loc,
            "and it comes back where the layout left it rather than where \
             it was closed"
        );

        // The residue, and the claim made about it. The size is still the
        // one the client last committed, because the configure that would
        // have changed it was suppressed — and the next sweep sends it.
        let configures = client.configures.len();
        state.move_pane(
            pane,
            elsewhere,
            elsewhere,
            AnimationSpec::default(),
            refused + Duration::from_millis(200),
            Standing::Tile,
        );
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert!(
            client.configures.len() > configures,
            "the configure the close swallowed is sent by the first sweep \
             after the window comes back, which is what makes the \
             suppression a delay rather than a loss"
        );
    }
}

/// **#100, the Wayland half: a menu near a screen edge was drawn off it.**
///
/// `new_popup` took the positioner as `_positioner` and dropped it, which
/// left Smithay's `get_geometry()` standing — anchor and gravity honoured,
/// `constraint_adjustment` ignored. This walks the arithmetic of the case
/// that produces: a window whose right edge is near the right edge of a
/// 1920-wide screen, and a context menu anchored at that edge opening
/// rightwards.
///
/// Both directions are asserted. The first assertion is the bug — the
/// placement the old code produced is *outside* the screen — and the
/// second is the fix. Without the first, a `popup_target` that returned
/// something absurdly large would pass the test by making every placement
/// look fine.
///
/// It is a unit test of arithmetic rather than of `place_popup`, because
/// `place_popup` needs a live `PopupSurface`, which needs a client. What
/// it does pin is the part that was wrong: the translation between the
/// compositor's coordinates and the positioner's, which is `popup_target`,
/// and the fact that we ask for the *unconstrained* geometry.
#[test]
fn a_menu_at_the_screen_edge_is_flipped_back_onto_it() {
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_positioner::{
        Anchor, ConstraintAdjustment, Gravity,
    };

    let screen = at(0, 0, 1920, 1080);
    // A window whose right edge is at x = 1900, twenty pixels short of the
    // screen's.
    let window = at(1400, 100, 500, 800);

    // What a toolkit sends for a menu hung off a control near that edge:
    // anchored on the right of a small widget, opening rightwards, and
    // allowed to flip on either axis if that does not fit.
    let positioner = PositionerState {
        rect_size: (200, 300).into(),
        anchor_rect: at(450, 200, 10, 10),
        anchor_edges: Anchor::Right,
        gravity: Gravity::Right,
        constraint_adjustment: ConstraintAdjustment::FlipX | ConstraintAdjustment::FlipY,
        ..PositionerState::default()
    };

    // No parent popups: this menu hangs off the toplevel itself.
    let parents = Point::<i32, Logical>::from((0, 0));
    let target = popup_target(screen, window.loc, parents);

    // The placement before the fix, in compositor coordinates.
    let unconstrained = positioner.get_geometry();
    let on_screen = |geometry: Rectangle<i32, Logical>| {
        Rectangle::new(window.loc + parents + geometry.loc, geometry.size)
    };
    assert!(
        !screen.contains_rect(on_screen(unconstrained)),
        "the test case has stopped reaching off the screen, so it no \
         longer pins anything: {:?}",
        on_screen(unconstrained)
    );

    // And after it.
    let constrained = positioner.get_unconstrained_geometry(target);
    assert!(
        screen.contains_rect(on_screen(constrained)),
        "a popup that was allowed to flip is still off the screen: {:?}",
        on_screen(constrained)
    );
    // Flipped rather than merely shrunk: the size the client asked for is
    // the size it gets, which is the difference between a menu with its
    // entries in it and a menu with a scrollbar.
    assert_eq!(constrained.size, positioner.rect_size);
}

/// A submenu is measured from its parent popup, not from the window.
///
/// `get_popup_toplevel_coords` is the second of the two translations in
/// `popup_target`, and it is the one with nothing else to catch it: a
/// first-level menu has a zero offset, so dropping the term entirely would
/// leave every test that uses one passing. The screen a submenu is
/// constrained against has to be moved by how far down the chain it hangs,
/// or the deeper it goes the more room it thinks it has.
#[test]
fn a_submenu_is_offset_by_the_chain_above_it() {
    let screen = at(0, 0, 1920, 1080);
    let root = Point::<i32, Logical>::from((1400, 100));
    let parents = Point::<i32, Logical>::from((250, 60));

    let direct = popup_target(screen, root, (0, 0).into());
    let nested = popup_target(screen, root, parents);

    assert_eq!(nested.loc, direct.loc - parents);
    assert_eq!(nested.size, screen.size);
    // A point that is the top-left of the screen in compositor
    // coordinates is the top-left of the target in either popup's own.
    assert_eq!(root + parents + nested.loc, screen.loc);
}

/// **The refusal path a stale serial takes.**
///
/// A client may ask for a popup grab with any serial it likes, including
/// one from an event that is long gone or one it never received. The
/// compositor's answer has to be "no" — `grab` declines and returns —
/// rather than an assertion or an unwrap, because there is nothing above a
/// compositor to restart it.
///
/// `may_grab` is the half of that decision that can be pinned without a
/// client: the seat answers three booleans about a device and this decides
/// whether a popup may take it. The other half — `grab_popup` returning
/// `Err` for a popup that is already mapped, orphaned, or not the topmost
/// — is Smithay's, and `grab` handles it by logging and returning; see the
/// `match` there.
#[test]
fn a_grab_held_by_a_stranger_is_refused() {
    // Nothing holds the device: the ordinary case, a menu opening while
    // the compositor is idle.
    assert!(may_grab(false, false, false));
    // This chain already holds it. A submenu opening inside its parent's
    // grab arrives here, and refusing it would break every nested menu.
    assert!(may_grab(true, true, false));
    assert!(may_grab(true, false, true));
    // Somebody else holds it -- one of Solium's own move or resize grabs,
    // or a serial this client made up. Declined.
    assert!(!may_grab(true, false, false));
}
/// **A drag owns the pointer, so the compositor stops describing what is
/// under it.**
///
/// A client that starts a drag gets a `DnDGrab` installed on the pointer,
/// so `PointerHandle::is_grabbed` is true for the whole gesture -- checked
/// against smithay 0.7's `selection/data_device/device.rs`, which calls
/// `set_grab` with it, and `input/pointer/mod.rs`, where `is_grabbed` is
/// `!matches!(guard.grab, GrabStatus::None)`. That is the machinery, and it
/// is why a drag needs no flag of its own: [`Solium::assert_cursor`]
/// already declines to recompute while the pointer is grabbed.
///
/// What it buys is that dragging a file across a window's edge does not
/// make the pointer offer a resize. The press that would perform that
/// resize cannot happen -- the button is already down and belongs to the
/// drag -- so a resize arrow there is #108's symptom again, arrived at from
/// a fourth direction, and it would flicker on and off along every edge the
/// drag crosses.
///
/// The `false` half is what makes this able to fail: the same call with no
/// grab clears the shape, so an `assert_cursor` that had lost its early
/// return would clear it in both.
#[test]
fn a_grabbed_pointer_keeps_the_shape_it_had() {
    let display = smithay::reexports::wayland_server::Display::<Solium>::new()
        .expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());

    // Where a border drag leaves the pointer: `input::pointer_button`
    // asserts the shape as it starts the grab, precisely so that it holds
    // for the drag. A DnD grab arrives at the same place by a different
    // road -- whatever was showing when the buttons went down.
    assert!(state.pointer.assert(Some(CursorIcon::NwseResize)));

    // Nothing is mapped, so the hit test under this point claims nothing
    // and the compositor's answer for it is "say nothing" -- which is a
    // *write*, and the one the grab has to suppress.
    let location = Point::<f64, Logical>::from((300.0, 300.0));
    assert_eq!(state.claim_under(location), Claim::Nothing);
    assert_eq!(state.claim_under(location).cursor(), None);

    state.assert_cursor(location, true);
    assert_eq!(
        state.pointer.showing(),
        CursorImageStatus::Named(CursorIcon::NwseResize),
        "a grab owns the pointer until it ends, so crossing anything \
         underneath must not change the shape"
    );

    state.assert_cursor(location, false);
    assert_eq!(
        state.pointer.showing(),
        CursorImageStatus::default_named(),
        "and the first motion after the grab ends hands the pointer back \
         to the ordinary hit test"
    );
}
/// **Issue #57's state machine: the icon is kept for exactly one drag.**
///
/// Needs a real `WlSurface`, and one cannot be conjured: smithay offers no
/// constructor, and `wl_surface`'s server-side user data type is private,
/// so `Client::create_resource` cannot name it either. A client has to ask
/// over the wire. That is the same conclusion `scale_resend` above reaches
/// for `ToplevelSurface`, and this fixture is deliberately its smaller
/// half: one global, one surface, no `xdg_shell`, no buffer, no output.
///
/// **Nothing here may build a Qt scene**, which is why no toplevel is
/// opened and none is needed. A window mapped inside a process that is
/// already holding a raw libwayland connection aborts the whole test
/// binary -- see the long note in `scale_resend`, which found that the hard
/// way. A bare `wl_surface` never reaches `new_toplevel`, so no decoration
/// is ever built for it.
mod drag_icon {
    use super::*;
    use smithay::reexports::wayland_server::Display;
    use std::os::unix::net::UnixStream;
    use wayland_client::protocol::{wl_compositor, wl_registry, wl_surface};
    use wayland_client::{Connection, Dispatch, Proxy as _, QueueHandle};

    /// The client side: `wl_compositor` and nothing else, because a
    /// surface is the whole of what is wanted.
    #[derive(Debug, Default)]
    struct Client {
        compositor: Option<wl_compositor::WlCompositor>,
    }

    impl Dispatch<wl_registry::WlRegistry, ()> for Client {
        fn event(
            state: &mut Self,
            registry: &wl_registry::WlRegistry,
            event: wl_registry::Event,
            (): &(),
            _conn: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            let wl_registry::Event::Global {
                name, interface, ..
            } = event
            else {
                return;
            };
            if interface == "wl_compositor" {
                state.compositor = Some(registry.bind(name, 1, qh, ()));
            }
        }
    }

    wayland_client::delegate_noop!(Client: ignore wl_compositor::WlCompositor);
    wayland_client::delegate_noop!(Client: ignore wl_surface::WlSurface);

    /// Start a drag, drop it, start a second one, and destroy the surface
    /// under that one.
    ///
    /// One test rather than three because the fixture is the expensive
    /// part -- a display, a socket pair and a round trip -- and because
    /// each step is the next step's precondition: "cleared on drop" says
    /// nothing unless something was there to be cleared.
    #[test]
    fn an_icon_lasts_one_drag_and_outlives_neither_the_drop_nor_its_surface() {
        let mut display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());

        let (server_side, client_side) =
            UnixStream::pair().expect("a socket pair for the test client");
        let served = display
            .handle()
            .insert_client(server_side, std::sync::Arc::new(ClientState::default()))
            .expect("inserting the test client");
        let conn = Connection::from_socket(client_side).expect("wrapping the client socket");
        let mut event_queue = conn.new_event_queue::<Client>();
        let qh = event_queue.handle();
        let mut client = Client::default();

        conn.display().get_registry(&qh, ());
        conn.flush().expect("flushing get_registry");
        display
            .dispatch_clients(&mut state)
            .expect("dispatching get_registry");
        display
            .flush_clients()
            .expect("flushing the registry snapshot");
        // Safe to block: the server wrote the whole registry on the line
        // above and nothing but this thread drives it, so these bytes are
        // already in the kernel buffer. Same argument as `scale_resend`'s
        // one blocking read.
        event_queue
            .blocking_dispatch(&mut client)
            .expect("reading the registry snapshot");

        let compositor = client.compositor.clone().expect("wl_compositor bound");
        let asked = compositor.create_surface(&qh, ());
        conn.flush().expect("flushing create_surface");
        display
            .dispatch_clients(&mut state)
            .expect("dispatching create_surface");

        // The compositor's own handle on the surface the client just made.
        // The protocol id is the same number on both sides of one
        // connection, which is what makes this lookup exact rather than a
        // search for "the only surface around".
        let icon: WlSurface = served
            .object_from_protocol_id(&display.handle(), asked.id().protocol_id())
            .expect("the compositor made a wl_surface for the request");

        // Taken once: every call below needs it, and it cannot be read
        // out of `state` in the same expression that borrows `state`
        // mutably.
        let seat = state.seat.clone();

        // A drag with no icon is ordinary -- a text selection dragged
        // inside one window often has none -- and must leave nothing
        // behind to be drawn.
        ClientDndGrabHandler::started(&mut state, None, None, seat.clone());
        assert!(
            state.dnd_icon().is_none(),
            "a drag the client chose not to illustrate draws nothing"
        );

        // The drag the issue is about.
        state.redraw = false;
        ClientDndGrabHandler::started(&mut state, None, Some(icon.clone()), seat.clone());
        assert_eq!(
            state.dnd_icon().as_ref(),
            Some(&icon),
            "the icon is offered exactly once, at the start of the drag, \
             and keeping it is the whole of #57"
        );
        assert!(
            state.redraw,
            "the icon appears at a pointer that has not moved, so nothing \
             else on screen damages the region it is about to occupy"
        );

        // The buttons come up. `DnDGrab::unset` calls its own `drop`,
        // which calls this, so a cancelled or stolen grab arrives here
        // too -- checked against smithay 0.7's `dnd_grab.rs`.
        state.redraw = false;
        ClientDndGrabHandler::dropped(&mut state, None, true, seat.clone());
        assert!(
            state.dnd_icon.is_none(),
            "the drag is over, so the icon stops being drawn -- otherwise \
             it stays painted over the session that outlived it"
        );
        assert!(state.redraw, "and the frame that removes it has to happen");

        // A drop that nobody accepted ends the drag just as thoroughly.
        ClientDndGrabHandler::started(&mut state, None, Some(icon.clone()), seat.clone());
        ClientDndGrabHandler::dropped(&mut state, None, false, seat.clone());
        assert!(state.dnd_icon.is_none());

        // **The client goes away mid-drag**, which never reaches `dropped`:
        // the grab is only unset when the buttons come up, and an
        // application that is gone will not be raising any. Destroying the
        // surface is the same road a disconnect takes -- every object the
        // client owned is destroyed -- and it is the deterministic half.
        ClientDndGrabHandler::started(&mut state, None, Some(icon.clone()), seat.clone());
        assert!(state.dnd_icon().is_some());
        asked.destroy();
        conn.flush().expect("flushing the surface destroy");
        display
            .dispatch_clients(&mut state)
            .expect("dispatching the surface destroy");
        assert!(!icon.alive(), "the fixture destroyed the surface");
        assert!(
            state.dnd_icon().is_none(),
            "a dead surface produces no render elements, so keeping one \
             is a drag icon that is never drawn and never cleared"
        );
        assert!(
            state.dnd_icon.is_none(),
            "and the reader clears the field rather than filtering it on \
             every frame for the rest of the session"
        );
    }
}
