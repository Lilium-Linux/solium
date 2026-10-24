//! The pointer, for `Solium.cursor`: the named shape it shows, whether a
//! button is held, how fast it moves, the scale of the monitor under it and
//! the configured size, and a warp's flight: whether the picture is flying,
//! from where and to where (#220). Sent once a frame while a pointer scene is
//! configured, and only when something moved
//! (`tests::the_published_pointer_is_its_buttons_its_motion_its_monitor_and_its_size`,
//! `tests::a_flight_is_published_while_it_flies_and_said_to_have_landed_after`,
//! `tests::nothing_is_published_with_no_scene`).
//!
//! **While Solium's own lock holds the session, no button is held and the
//! pointer does not move**, as far as the pointer's scene is told: a press,
//! and the way the pointer goes, are a tap on the passcode pad and the way to
//! its key (`lock_auth.rs`), which no scene but the compositor may know
//! (`tests::at_the_native_lock_the_pointer_scene_hears_no_press_and_no_motion`).

use std::collections::BTreeMap;

use crate::json::Json;
use crate::state::Solium;

/// What Qt last took.
#[derive(Debug, Default)]
pub(crate) struct Published {
    values: Option<BTreeMap<String, Json>>,
}

impl Published {
    /// The value Qt last took under `key`.
    fn get(&self, key: &str) -> Option<Json> {
        self.values
            .as_ref()
            .and_then(|values| values.get(key))
            .cloned()
    }
}

/// Send the pointer if it changed since Qt last took it, while a scene is
/// configured: with none, nothing is published and nothing is asked for, so a
/// session that names no scene does what it did before there was one
/// (`tests::nothing_is_published_with_no_scene`). A pointer that moved asks
/// for the frame after, which publishes it standing still
/// (`tests::a_moving_pointer_asks_for_the_frame_that_says_it_stopped`). A batch
/// Qt could not take, before it has started, is sent again whole.
pub(crate) fn publish(state: &mut Solium, last: &mut Published, apply: fn(&str) -> bool) {
    if !state.pointer.has_scene() {
        return;
    }
    let (values, moving) = values(state, last);
    if moving {
        state.redraw = true;
    }
    if last.values.as_ref() == Some(&values) {
        return;
    }
    if apply(&Json::Object(values.clone()).render()) {
        last.values = Some(values);
    }
}

/// The pointer's values now, and whether it moved since they were last read.
/// The shape is its CSS name; with no named shape shown, a client's own cursor
/// surface or none, it stays the one last published, `previous`.
///
/// While a warp's picture flies, `moving` is true and the velocity is the
/// flight's own; `from` and `to` are the last flight's ends, in the global
/// space's logical pixels, and stay where they were once it lands (#220).
/// `tests::the_published_pointer_is_its_buttons_its_motion_its_monitor_and_its_size`,
/// `tests::a_named_shape_reaches_solium_cursor_shape`,
/// `tests::a_flight_is_published_while_it_flies_and_said_to_have_landed_after`.
fn values(state: &mut Solium, previous: &Published) -> (BTreeMap<String, Json>, bool) {
    let shape = state
        .pointer
        .shape()
        .map(|icon| Json::Text(icon.name().to_owned()))
        .or_else(|| previous.get("shape"))
        .unwrap_or_else(|| Json::Text("default".to_owned()));
    let now = state.clock.now();
    // Read either way, so a movement the flight hid, or the lock, is not
    // told later as if it were new.
    let moved = state.pointer.velocity();
    let flight = state.flight.filter(|flight| !flight.landed(now));
    let velocity = flight.map_or(moved, |flight| flight.velocity(now));
    // At Solium's own lock: no press and no motion, from its first instant.
    let blind = state
        .lock
        .as_ref()
        .is_some_and(crate::lock::Lock::is_native);
    let (pressed, velocity, flight) = if blind {
        (false, smithay::utils::Point::default(), None)
    } else {
        (state.pointer_buttons != 0, velocity, flight)
    };
    let point = |at: smithay::utils::Point<f64, smithay::utils::Logical>| {
        Json::Object(BTreeMap::from([
            ("x".to_owned(), Json::Number(at.x)),
            ("y".to_owned(), Json::Number(at.y)),
        ]))
    };
    let (from, to) = match state.flight {
        Some(flight) => (point(flight.from()), point(flight.to())),
        None => (
            previous
                .get("from")
                .unwrap_or_else(|| point(Default::default())),
            previous
                .get("to")
                .unwrap_or_else(|| point(Default::default())),
        ),
    };
    let scale = state
        .active_output()
        .map_or(1.0, |output| output.current_scale().fractional_scale());
    let values = BTreeMap::from([
        ("shape".to_owned(), shape),
        ("pressed".to_owned(), Json::Bool(pressed)),
        (
            "velocity".to_owned(),
            Json::Object(BTreeMap::from([
                ("x".to_owned(), Json::Number(velocity.x)),
                ("y".to_owned(), Json::Number(velocity.y)),
            ])),
        ),
        ("scale".to_owned(), Json::Number(scale)),
        (
            "size".to_owned(),
            Json::Number(f64::from(state.pointer.size())),
        ),
        ("moving".to_owned(), Json::Bool(flight.is_some())),
        ("from".to_owned(), from),
        ("to".to_owned(), to),
    ]);
    (
        values,
        flight.is_some() || velocity.x != 0.0 || velocity.y != 0.0,
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    use smithay::backend::input::ButtonState;
    use smithay::input::pointer::{CursorIcon, CursorImageStatus};
    use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
    use smithay::utils::{Logical, Point};

    use super::{Published, publish};
    use crate::cursor::theme;
    use crate::json::Json;
    use crate::keymap::tests::{tap, us_ru};
    use crate::state::Solium;

    thread_local! {
        /// Every batch the fake `apply` was handed.
        static SENT: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    fn taken(json: &str) -> bool {
        SENT.with(|sent| sent.borrow_mut().push(json.to_owned()));
        true
    }

    fn sent() -> Vec<Json> {
        SENT.with(|sent| {
            sent.borrow_mut()
                .drain(..)
                .filter_map(|json| Json::parse(&json))
                .collect()
        })
    }

    /// A monitor named `name`, `width` pixels wide at `x`, at `scale`.
    fn monitor(state: &mut Solium, name: &str, x: i32, scale: f64) {
        let output = Output::new(
            name.to_owned(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "solium".to_owned(),
                model: name.to_owned(),
            },
        );
        output.change_current_state(
            Some(Mode {
                size: (1920, 1080).into(),
                refresh: 60_000,
            }),
            None,
            Some(Scale::Fractional(scale)),
            Some((x, 0).into()),
        );
        state.space.map_output(&output, (x, 0));
    }

    /// A scene configured for the pointer, at a path nothing reads: the model
    /// asks only whether one is configured.
    fn with_a_scene(state: &mut Solium, size: i32) {
        state.pointer.configure(
            &theme::Configured {
                scene: Some("/solium-test/never-built/Cursor.qml".to_owned()),
                size: Some(size),
                ..theme::Configured::default()
            },
            &theme::Environment::default(),
        );
    }

    fn motion(state: &mut Solium, by: (f64, f64), at: u64) {
        let region = crate::monitor::union(&state.space).expect("a monitor");
        crate::synth::send_motion(state, region, Point::<f64, Logical>::from(by), at);
    }

    fn value<'a>(batch: &'a Json, key: &str) -> Option<&'a Json> {
        match batch {
            Json::Object(values) => values.get(key),
            _ => None,
        }
    }

    /// **With no scene configured, nothing is published and nothing asked
    /// for**: a session that names no scene does what it did before there was
    /// one, however the pointer moves.
    #[test]
    fn nothing_is_published_with_no_scene() {
        let (_display, mut state) = us_ru(1);
        monitor(&mut state, "pointer-none", 0, 1.0);
        state.pointer.configure(
            &theme::Configured::default(),
            &theme::Environment::default(),
        );
        let mut last = Published::default();
        motion(&mut state, (40.0, 0.0), 16_000);
        state.redraw = false;
        publish(&mut state, &mut last, taken);
        assert_eq!(
            (sent(), state.redraw, state.pointer.has_scene()),
            (Vec::new(), false, false),
            "(what was published, whether a frame was asked for, whether a scene is \
             configured)"
        );
    }

    /// **The published pointer is its buttons, its motion, the monitor under
    /// it and its size**: a press is `pressed`, motion is `velocity` in
    /// logical pixels a second, the scale is the monitor the pointer is on,
    /// and the size is the configured one. Tested with the Cyrillic group
    /// active (#132).
    #[test]
    fn the_published_pointer_is_its_buttons_its_motion_its_monitor_and_its_size() {
        let (_display, mut state) = us_ru(1);
        monitor(&mut state, "pointer-left", 0, 1.0);
        monitor(&mut state, "pointer-right", 1920, 2.0);
        with_a_scene(&mut state, 32);
        let mut last = Published::default();
        motion(&mut state, (100.0, 100.0), 1_000_000);
        publish(&mut state, &mut last, taken);
        motion(&mut state, (8.0, 0.0), 1_008_000);
        motion(&mut state, (8.0, -4.0), 1_016_000);
        let region = crate::monitor::union(&state.space).expect("a monitor");
        crate::synth::send_button(&mut state, region, 0x110, ButtonState::Pressed, 1_017_000);
        publish(&mut state, &mut last, taken);
        crate::synth::send_button(&mut state, region, 0x110, ButtonState::Released, 1_018_000);
        motion(&mut state, (2000.0, 0.0), 2_000_000);
        publish(&mut state, &mut last, taken);
        let batches = sent();
        let read = |batch: &Json| {
            (
                value(batch, "pressed").cloned(),
                value(batch, "velocity").cloned(),
                value(batch, "scale").cloned(),
                value(batch, "size").cloned(),
            )
        };
        let velocity = |x: f64, y: f64| {
            Some(Json::Object(BTreeMap::from([
                ("x".to_owned(), Json::Number(x)),
                ("y".to_owned(), Json::Number(y)),
            ])))
        };
        assert_eq!(
            batches.iter().map(read).collect::<Vec<_>>(),
            vec![
                (
                    Some(Json::Bool(false)),
                    velocity(0.0, 0.0),
                    Some(Json::Number(1.0)),
                    Some(Json::Number(32.0))
                ),
                (
                    Some(Json::Bool(true)),
                    velocity(1000.0, -250.0),
                    Some(Json::Number(1.0)),
                    Some(Json::Number(32.0))
                ),
                (
                    Some(Json::Bool(false)),
                    velocity(0.0, 0.0),
                    Some(Json::Number(2.0)),
                    Some(Json::Number(32.0))
                ),
            ],
            "(pressed, velocity, scale, size): placed, moving with a button held, then on \
             the 2x monitor after a pause"
        );
    }

    /// **At Solium's own lock the pointer's scene hears no press and no
    /// motion**: a button held and the pointer moved while locked publish
    /// `pressed` false and a still pointer, from the instant of the lock, so
    /// the scene drawn over the lock cannot tell a tap on the passcode pad or
    /// the way to its key; the motion is not told after the unlock either.
    /// Unlocked, the same press and motion are told. Tested with the
    /// Cyrillic group active (#132).
    #[test]
    fn at_the_native_lock_the_pointer_scene_hears_no_press_and_no_motion() {
        let (_display, mut state) = us_ru(1);
        monitor(&mut state, "pointer-locked", 0, 1.0);
        with_a_scene(&mut state, 32);
        let mut last = Published::default();
        motion(&mut state, (100.0, 100.0), 1_000_000);
        publish(&mut state, &mut last, taken);
        let region = crate::monitor::union(&state.space).expect("a monitor");
        let read = |batch: &Json| {
            (
                value(batch, "pressed").cloned(),
                value(batch, "velocity").cloned(),
            )
        };
        let still = Some(Json::Object(BTreeMap::from([
            ("x".to_owned(), Json::Number(0.0)),
            ("y".to_owned(), Json::Number(0.0)),
        ])));
        let _ = sent();

        state.lock_natively();
        motion(&mut state, (8.0, 0.0), 1_008_000);
        crate::synth::send_button(&mut state, region, 0x110, ButtonState::Pressed, 1_009_000);
        motion(&mut state, (8.0, -4.0), 1_016_000);
        state.redraw = false;
        publish(&mut state, &mut last, taken);
        let locked: Vec<_> = sent().iter().map(read).collect();
        assert!(
            locked.iter().all(
                |(pressed, velocity)| *pressed == Some(Json::Bool(false)) && *velocity == still
            ),
            "(pressed, velocity) told while natively locked: {locked:?}"
        );
        assert!(
            !state.redraw,
            "a still pointer at the lock asked for a frame"
        );
        crate::synth::send_button(&mut state, region, 0x110, ButtonState::Released, 1_017_000);

        state.unlock_natively();
        publish(&mut state, &mut last, taken);
        let after: Vec<_> = sent().iter().map(read).collect();
        assert!(
            after.iter().all(|(_, velocity)| *velocity == still),
            "the motion at the lock was told after the unlock: {after:?}"
        );

        // The premise: unlocked, the same press and motion are told.
        motion(&mut state, (8.0, 0.0), 2_008_000);
        crate::synth::send_button(&mut state, region, 0x110, ButtonState::Pressed, 2_009_000);
        motion(&mut state, (8.0, -4.0), 2_016_000);
        publish(&mut state, &mut last, taken);
        let unlocked: Vec<_> = sent().iter().map(read).collect();
        assert!(
            unlocked
                .iter()
                .any(|(pressed, velocity)| *pressed == Some(Json::Bool(true)) && *velocity != still),
            "the premise: unlocked, a press and motion are told: {unlocked:?}"
        );
        crate::synth::send_button(&mut state, region, 0x110, ButtonState::Released, 2_017_000);
    }

    /// **A flight is published while it flies, and said to have landed
    /// after** (#220): a warp's picture flying is `moving`, from where it set
    /// off to where the pointer went, at the flight's own velocity, and asks
    /// for the next frame; once it lands it is still, the two ends are kept,
    /// and no frame is asked for. Tested with the Cyrillic group active
    /// (#132).
    #[test]
    fn a_flight_is_published_while_it_flies_and_said_to_have_landed_after() {
        use crate::script::{AnimationSpec, Warp, WarpTo};
        let (_display, mut state) = us_ru(1);
        monitor(&mut state, "pointer-flight", 0, 1.0);
        with_a_scene(&mut state, 24);
        let mut last = Published::default();
        motion(&mut state, (100.0, 100.0), 1_000_000);
        publish(&mut state, &mut last, taken);
        crate::input::warp::warp(
            &mut state,
            Warp {
                to: WarpTo::Point {
                    x: 1100.0,
                    y: 100.0,
                },
                motion: Some(AnimationSpec {
                    duration: std::time::Duration::from_secs(10),
                    easing: crate::present::Curve::Linear,
                }),
                interrupt: crate::script::Interrupt::Pointer,
            },
        );
        state.redraw = false;
        publish(&mut state, &mut last, taken);
        let flying = state.redraw;
        state.clock.advance(std::time::Duration::from_secs(11));
        let now = state.clock.now();
        state.settle_flight(now);
        state.redraw = false;
        publish(&mut state, &mut last, taken);
        let landed = state.redraw;
        let point = |x: f64, y: f64| {
            Some(Json::Object(BTreeMap::from([
                ("x".to_owned(), Json::Number(x)),
                ("y".to_owned(), Json::Number(y)),
            ])))
        };
        let batches = sent();
        let read = |batch: &Json| {
            let speed = match value(batch, "velocity") {
                Some(Json::Object(velocity)) => match (velocity.get("x"), velocity.get("y")) {
                    (Some(Json::Number(x)), Some(Json::Number(y))) => Some((x.round(), y.round())),
                    _ => None,
                },
                _ => None,
            };
            (
                value(batch, "moving").cloned(),
                value(batch, "from").cloned(),
                value(batch, "to").cloned(),
                speed,
            )
        };
        assert_eq!(
            (
                batches.iter().skip(1).map(read).collect::<Vec<_>>(),
                flying,
                landed
            ),
            (
                vec![
                    (
                        Some(Json::Bool(true)),
                        point(100.0, 100.0),
                        point(1100.0, 100.0),
                        Some((100.0, 0.0)),
                    ),
                    (
                        Some(Json::Bool(false)),
                        point(100.0, 100.0),
                        point(1100.0, 100.0),
                        Some((0.0, 0.0)),
                    ),
                ],
                true,
                false,
            ),
            "([(moving, from, to, velocity) flying, then landed], a frame asked for flying, \
             one asked for landed)"
        );
    }

    /// **A moving pointer asks for the frame that says it stopped**: the frame
    /// that publishes a velocity asks for the next, which publishes zero and
    /// asks for nothing more.
    #[test]
    fn a_moving_pointer_asks_for_the_frame_that_says_it_stopped() {
        let (_display, mut state) = us_ru(1);
        monitor(&mut state, "pointer-stops", 0, 1.0);
        with_a_scene(&mut state, 24);
        let mut last = Published::default();
        motion(&mut state, (10.0, 10.0), 1_000_000);
        motion(&mut state, (16.0, 0.0), 1_016_000);
        let mut asked = Vec::new();
        for _ in 0..3 {
            state.redraw = false;
            publish(&mut state, &mut last, taken);
            asked.push(state.redraw);
        }
        let velocities: Vec<_> = sent()
            .iter()
            .map(|batch| value(batch, "velocity").cloned())
            .collect();
        assert_eq!(
            asked,
            [true, false, false],
            "frames asked for after the pointer stopped, with velocities {velocities:?}"
        );
    }

    /// **A named shape reaches `Solium.cursor.shape`**, by its CSS name: the
    /// one a client names over its surface, the compositor's over a resize
    /// border, and the client's again when the compositor stops saying. A
    /// cursor surface of the client's own, here none at all, draws no named
    /// shape, so the scene keeps the last. The scene itself is configured by a
    /// binding pressed with the Cyrillic group active (#132).
    #[test]
    fn a_named_shape_reaches_solium_cursor_shape() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            let (directory, path) = crate::qml::pointer::tests::written(
                "solium-models-pointer-shape",
                "import QtQuick\nimport Solium\nItem { readonly property string shape: Solium.cursor.shape }\n",
            );
            let entry = directory.join("init.lua");
            std::fs::write(
                &entry,
                format!(
                    "sol.bind(\"super+c\", function() sol.cursor_theme({{ scene = {:?} }}) end)\n",
                    path.display().to_string()
                ),
            )
            .expect("writing the test script");
            let (_display, mut state) = us_ru(1);
            monitor(&mut state, "pointer-shape", 0, 1.0);
            state.pointer.configure(
                &theme::Configured::default(),
                &theme::Environment::default(),
            );
            state.start_scripts(Some(
                crate::script::Scripts::load(&entry).expect("loading the test script"),
            ));
            // super, then the key that is `c` on the Latin layout and `с` on
            // the Russian one.
            tap(&mut state, &[133, 54]);
            assert!(
                state.pointer.has_scene(),
                "the binding pressed on Russian did not configure the scene"
            );
            let mut last = Published::default();
            let mut read = |state: &mut Solium| {
                publish(state, &mut last, crate::qml::pointer::publish);
                state
                    .pointer
                    .scene_for_test()
                    .map(|scene| scene.get_string_for_test("shape"))
                    .unwrap_or_default()
            };
            state
                .pointer
                .show(CursorImageStatus::Named(CursorIcon::Text));
            let text = read(&mut state);
            state.pointer.assert(Some(CursorIcon::EwResize));
            let border = read(&mut state);
            state.pointer.assert(None);
            let back = read(&mut state);
            state.pointer.show(CursorImageStatus::Hidden);
            let hidden = read(&mut state);
            drop(state);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                [text, border, back, hidden],
                ["text", "ew-resize", "text", "text"],
                "[a client's named shape, a resize border, the client's again, a hidden pointer]"
            );
        });
    }

    /// **A configured scene hears every named shape**, each by its CSS name,
    /// with a theme configured beside it, so it can draw every one of them.
    #[test]
    fn a_configured_scene_hears_every_named_shape() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            let (directory, path) = crate::qml::pointer::tests::written(
                "solium-models-pointer-every-shape",
                "import QtQuick\nimport Solium\nItem { readonly property string shape: Solium.cursor.shape }\n",
            );
            let (_display, mut state) = us_ru(1);
            monitor(&mut state, "pointer-every", 0, 1.0);
            state.pointer.configure(
                &theme::Configured {
                    theme: Some("Adwaita".to_owned()),
                    scene: Some(path.display().to_string()),
                    ..theme::Configured::default()
                },
                &theme::Environment::default(),
            );
            let mut last = Published::default();
            let mut heard = Vec::new();
            for icon in EVERY_SHAPE {
                state.pointer.show(CursorImageStatus::Named(icon));
                publish(&mut state, &mut last, crate::qml::pointer::publish);
                heard.push(
                    state
                        .pointer
                        .scene_for_test()
                        .map(|scene| scene.get_string_for_test("shape"))
                        .unwrap_or_default(),
                );
            }
            drop(state);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                heard,
                EVERY_SHAPE.map(|icon| icon.name().to_owned()).to_vec(),
                "the shapes the scene heard"
            );
        });
    }

    /// Every shape `wp_cursor_shape_v1` names.
    const EVERY_SHAPE: [CursorIcon; 34] = [
        CursorIcon::Default,
        CursorIcon::ContextMenu,
        CursorIcon::Help,
        CursorIcon::Pointer,
        CursorIcon::Progress,
        CursorIcon::Wait,
        CursorIcon::Cell,
        CursorIcon::Crosshair,
        CursorIcon::Text,
        CursorIcon::VerticalText,
        CursorIcon::Alias,
        CursorIcon::Copy,
        CursorIcon::Move,
        CursorIcon::NoDrop,
        CursorIcon::NotAllowed,
        CursorIcon::Grab,
        CursorIcon::Grabbing,
        CursorIcon::EResize,
        CursorIcon::NResize,
        CursorIcon::NeResize,
        CursorIcon::NwResize,
        CursorIcon::SResize,
        CursorIcon::SeResize,
        CursorIcon::SwResize,
        CursorIcon::WResize,
        CursorIcon::EwResize,
        CursorIcon::NsResize,
        CursorIcon::NeswResize,
        CursorIcon::NwseResize,
        CursorIcon::ColResize,
        CursorIcon::RowResize,
        CursorIcon::AllScroll,
        CursorIcon::ZoomIn,
        CursorIcon::ZoomOut,
    ];
}
