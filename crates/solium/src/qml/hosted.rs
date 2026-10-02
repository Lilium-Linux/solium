//! The compositor's half of what a hosted scene and the compositor say to each
//! other: properties written in place, the monitor it is on, the models' rows,
//! pointer events, and what its items claim of the pointer
//! (`tests::the_item_tree_decides_what_a_point_claims`). What it reserves
//! (#162), its grabs and its keyboard wants (#163) are still to come.

use std::{
    ffi::{CString, c_char, c_int},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{Result, anyhow};

use super::Scene;
use crate::json::Json;

#[expect(unsafe_code, reason = "the Qt host is C++; this is its C ABI")]
mod ffi {
    use super::{c_char, c_int};

    unsafe extern "C" {
        pub(super) fn solium_qml_scene_set_json(
            scene: *mut super::super::ffi::Scene,
            path: *const c_char,
            json_value: *const c_char,
        ) -> c_int;
        pub(super) fn solium_qml_host_next_on(monitor: *const c_char);
        pub(super) fn solium_qml_rows_apply(model: c_int, ops_json: *const c_char) -> c_int;
        pub(super) fn solium_qml_scene_pointer_event(
            scene: *mut super::super::ffi::Scene,
            kind: c_int,
            x: f64,
            y: f64,
            button: u32,
            buttons: u32,
            modifiers: u32,
            time: u64,
            angle_x: f64,
            angle_y: f64,
            pixel_x: f64,
            pixel_y: f64,
        );
        pub(super) fn solium_qml_scene_hit(
            scene: *const super::super::ffi::Scene,
            x: f64,
            y: f64,
        ) -> c_int;
        pub(super) fn solium_qml_scene_pointer_leave(scene: *mut super::super::ffi::Scene);
    }
}

/// A model's number, as `host.h`'s `SOLIUM_QML_ROWS_*` say it.
/// `tests::a_published_monitor_reaches_solium_monitor_in_its_scene`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Model {
    Monitors = 0,
    #[expect(dead_code, reason = "host.h numbers it; nothing publishes windows yet")]
    Windows = 1,
    #[expect(
        dead_code,
        reason = "host.h numbers it; nothing publishes workspaces yet"
    )]
    Workspaces = 2,
}

/// Apply one batch of row operations, rendered by `models::diff::render`, to
/// a model: `tests::a_published_monitor_reaches_solium_monitor_in_its_scene`.
/// False, with none of it taken, when Qt could not take it:
/// `tests::a_batch_that_does_not_match_the_rows_held_is_refused`,
/// `tests::a_refused_batch_takes_none_of_its_steps`.
#[expect(unsafe_code, reason = "calling into the Qt host")]
pub(crate) fn apply_rows(model: Model, ops: &str) -> bool {
    // A row a binding reads can move a scene's items.
    // `surface::tests::a_cached_hit_follows_published_rows_and_a_taken_action`.
    touched();
    let Ok(ops) = CString::new(ops) else {
        return false;
    };
    // SAFETY: `ops` outlives the call; the host copies what it keeps.
    unsafe { ffi::solium_qml_rows_apply(model as c_int, ops.as_ptr()) != 0 }
}

impl Scene {
    /// A scene hosted on one monitor: `Solium.monitor` inside it is that
    /// monitor's row, from the moment it is built.
    /// `tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn for_monitor(
        qml_path: &Path,
        width: i32,
        height: i32,
        initial: Option<&str>,
        monitor: &str,
    ) -> Result<Self> {
        let name =
            CString::new(monitor).map_err(|_| anyhow!("a monitor's name contains a NUL byte"))?;
        // SAFETY: `name` outlives the call; the host copies it.
        unsafe { ffi::solium_qml_host_next_on(name.as_ptr()) };
        let built = Self::for_host(qml_path, width, height, initial);
        // Cleared whether or not the scene was built, so no later scene is
        // hosted on this monitor by accident.
        // `tests::a_hosted_build_refused_before_the_host_leaves_the_next_scene_unhosted`.
        // SAFETY: null is the documented "none".
        unsafe { ffi::solium_qml_host_next_on(std::ptr::null()) };
        built
    }

    /// Write one property of the scene's root, by path.
    /// `scripted::tests::a_redeclared_property_is_written_into_the_live_scene`.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn set_json(&mut self, path: &str, value: &Json) -> bool {
        touched();
        let (Ok(path), Ok(value)) = (CString::new(path), CString::new(value.render())) else {
            return false;
        };
        // SAFETY: the scene is live for as long as `self`, and both strings
        // outlive the call.
        unsafe { ffi::solium_qml_scene_set_json(self.scene, path.as_ptr(), value.as_ptr()) != 0 }
    }

    /// Deliver one pointer event at a point in scene coordinates. A press
    /// that doubles the one before it is a double-click, by Qt's own rule.
    /// `tests::a_right_press_reaches_a_mouse_area_as_the_right_button`,
    /// `tests::a_side_button_reaches_the_scene_as_back`,
    /// `tests::the_wheel_reaches_a_wheel_handler_with_its_angle`,
    /// `tests::a_double_press_on_a_mouse_area_is_one_double_click`.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn pointer_event(&mut self, x: f64, y: f64, event: &ScenePointer) {
        touched();
        let still = ((0.0, 0.0), (0.0, 0.0));
        let (kind, button, (angle, pixels)) = match event.kind {
            PointerKind::Motion => (0, 0, still),
            PointerKind::Press(button) => (1, button, still),
            PointerKind::Release(button) => (2, button, still),
            PointerKind::Wheel { angle, pixels } => (3, 0, (angle, pixels)),
        };
        // SAFETY: the scene is live for as long as `self`.
        unsafe {
            ffi::solium_qml_scene_pointer_event(
                self.scene,
                kind,
                x,
                y,
                button,
                event.buttons,
                event.modifiers,
                event.time,
                angle.0,
                angle.1,
                pixels.0,
                pixels.1,
            );
        }
    }
}

/// What a scene's items claim at a point (Ruling 6).
/// `tests::the_item_tree_decides_what_a_point_claims`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hit {
    Nothing,
    /// Motion only: a hover strip, a `HoverHandler`.
    Hover,
    /// Presses, releases and the wheel too.
    Press,
}

/// What a pointer event asks of a point: motion asks for hover, a button or
/// the wheel for a press.
/// `state::tests::real_client::reflow_on_close::hosted::a_hover_strip_hears_the_motion_and_leaves_the_window_its_press`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Asking {
    Hover,
    Press,
}

impl Hit {
    pub(crate) fn claims(self, asking: Asking) -> bool {
        matches!(
            (self, asking),
            (Self::Press, _) | (Self::Hover, Asking::Hover)
        )
    }
}

impl Asking {
    pub(crate) fn of(kind: PointerKind) -> Self {
        match kind {
            PointerKind::Motion => Self::Hover,
            PointerKind::Press(_) | PointerKind::Release(_) | PointerKind::Wheel { .. } => {
                Self::Press
            }
        }
    }
}

/// How many times Qt may have changed an item tree: every tick, drain,
/// resize, property write, row batch, string taken and delivery. A hit cached
/// at one generation is good until the next, which is how one item walk
/// serves a whole frame's questions about a still pointer.
///
/// Qt also moves items when it polishes them, which is where a `Row`, a
/// `Column` or a layout places its children, and it polishes them as the
/// frame is rendered, after the tick's bump. So nothing may ask a hit between
/// `qml::tick` and that render, or it would be cached against the tree
/// before the polish.
/// `surface::tests::a_cached_hit_follows_the_scene_once_qt_has_run`,
/// `surface::tests::a_cached_hit_follows_published_rows_and_a_taken_action`.
static GENERATION: AtomicU64 = AtomicU64::new(0);

pub(crate) fn touched() {
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

impl Scene {
    /// What the items under a point claim.
    /// `tests::the_item_tree_decides_what_a_point_claims`.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn hit(&self, x: f64, y: f64) -> Hit {
        // SAFETY: the scene is live for as long as `self`.
        match unsafe { ffi::solium_qml_scene_hit(self.scene, x, y) } {
            2 => Hit::Press,
            1 => Hit::Hover,
            _ => Hit::Nothing,
        }
    }

    /// The pointer left this scene. `tests::a_left_scene_drops_its_hover`.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn leave(&mut self) {
        touched();
        // SAFETY: the scene is live for as long as `self`.
        unsafe { ffi::solium_qml_scene_pointer_leave(self.scene) }
    }
}

/// What a pointer event is, for a scene.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum PointerKind {
    Motion,
    /// A press of this Qt button.
    Press(u32),
    /// A release of this Qt button.
    Release(u32),
    /// Qt's `angleDelta` and `pixelDelta` (Ruling 9).
    Wheel {
        angle: (f64, f64),
        pixels: (f64, f64),
    },
}

/// One pointer event as a scene is told it: what it is, the Qt buttons held
/// after it, the keyboard modifiers, and when it happened, in milliseconds on
/// the compositor's clock, which is what Qt counts a double-click and a
/// `TapHandler`'s taps by.
/// `state::tests::real_client::reflow_on_close::hosted::a_scene_is_told_when_each_event_happened_on_the_compositors_clock`,
/// `tests::a_double_press_on_a_mouse_area_is_one_double_click`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ScenePointer {
    pub(crate) kind: PointerKind,
    pub(crate) buttons: u32,
    pub(crate) modifiers: u32,
    pub(crate) time: u64,
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;

    use super::{Hit, PointerKind, ScenePointer};
    use crate::qml::{Scene, qt_test::on_the_qt_thread};

    const CLAIMS: &str = r#"
        import QtQuick
        import QtQuick.Controls
        import Solium
        Item {
            // A button at 0..20 x 0..20.
            MouseArea { x: 0; y: 0; width: 20; height: 20 }
            // A hover-only strip at 30..40, and a button under a hover-only item at 50..60.
            Item { x: 30; y: 0; width: 10; height: 32; Solium.input: "hover" }
            MouseArea { x: 50; y: 0; width: 10; height: 32 }
            Item { x: 50; y: 0; width: 10; height: 32; Solium.input: "hover" }
            // A rounded button at 0..20 x 22..32 whose corner is outside its
            // mask. Qt calls only a typed `contains(point: point): bool`, and
            // the mask is the whole shape, its bounds too.
            MouseArea {
                x: 0; y: 22; width: 20; height: 10
                containmentMask: QtObject {
                    function contains(point: point): bool {
                        return point.x >= 0 && point.x < 20 && point.y >= 0 && point.y < 10
                            && (point.x >= 4 || point.y >= 4)
                    }
                }
            }
            // Opted out, invisible, transparent, and a tap and a hover handler.
            MouseArea { x: 22; y: 0; width: 6; height: 10; Solium.input: false }
            MouseArea { x: 22; y: 11; width: 6; height: 10; visible: false }
            MouseArea { x: 22; y: 22; width: 6; height: 10; opacity: 0 }
            Item { x: 42; y: 0; width: 6; height: 10; TapHandler {} }
            Item { x: 42; y: 11; width: 6; height: 10; HoverHandler {} }
            // Items that take presses themselves, each with a HoverHandler,
            // as a pointer cursor is set.
            MouseArea { x: 42; y: 22; width: 6; height: 10; HoverHandler {} }
            Button { x: 60; y: 0; width: 2; height: 10; HoverHandler {} }
            TextInput { x: 60; y: 11; width: 2; height: 10; HoverHandler {} }
            PathView { x: 60; y: 22; width: 2; height: 10; HoverHandler {} }
            MultiPointTouchArea { x: 62; y: 0; width: 2; height: 20; HoverHandler {} }
        }
    "#;

    /// **What claims a point is the live item tree** (#173, Ruling 6): where
    /// nothing takes input the point is nobody's, a button takes the press, a
    /// hover strip only hover, an item outside its mask nothing, and an
    /// opted-out, invisible or transparent item nothing.
    #[test]
    fn the_item_tree_decides_what_a_point_claims() {
        on_the_qt_thread(|| {
            let (directory, scene) = hosted("solium-hosted-claims", CLAIMS, "claims-1");
            let cases = [
                ((63.0, 31.0), Hit::Nothing, "where the scene draws nothing"),
                ((10.0, 10.0), Hit::Press, "a MouseArea"),
                ((35.0, 10.0), Hit::Hover, "Solium.input: \"hover\""),
                (
                    (55.0, 10.0),
                    Hit::Press,
                    "a hover-only item over a button leaves the press to the button",
                ),
                (
                    (1.0, 23.0),
                    Hit::Nothing,
                    "a rounded corner outside the containment mask",
                ),
                ((10.0, 27.0), Hit::Press, "inside the mask"),
                ((24.0, 5.0), Hit::Nothing, "Solium.input: false"),
                ((24.0, 15.0), Hit::Nothing, "invisible"),
                ((24.0, 26.0), Hit::Nothing, "opacity 0"),
                ((44.0, 5.0), Hit::Press, "a TapHandler"),
                ((44.0, 15.0), Hit::Hover, "a HoverHandler"),
                ((44.0, 27.0), Hit::Press, "a MouseArea with a HoverHandler"),
                ((61.0, 5.0), Hit::Press, "a Button with a HoverHandler"),
                ((61.0, 15.0), Hit::Press, "a TextInput with a HoverHandler"),
                ((61.0, 27.0), Hit::Press, "a PathView with a HoverHandler"),
                (
                    (63.0, 5.0),
                    Hit::Press,
                    "a MultiPointTouchArea with a HoverHandler",
                ),
            ];
            let wrong: Vec<String> = cases
                .iter()
                .filter(|((x, y), wanted, _)| scene.hit(*x, *y) != *wanted)
                .map(|((x, y), wanted, what)| {
                    format!(
                        "{what} at ({x}, {y}): wanted {wanted:?}, got {:?}",
                        scene.hit(*x, *y)
                    )
                })
                .collect();
            assert!(wrong.is_empty(), "{wrong:#?}");
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    /// **A `Text` takes a press only on a link it handles**, as Qt gives it
    /// one: every `Text` accepts the left button, to look for a link under a
    /// press, and lets the press go where there is none, or where nothing
    /// hears `linkActivated`. So a link takes the press, with a
    /// `HoverHandler` beside it for the pointer's shape too; the rest of that
    /// `Text` is only its hover; a link nothing handles is only hover too, as
    /// Qt gives a styled `Text` the hover for its links; and a plain label
    /// takes nothing.
    #[test]
    fn a_text_takes_a_press_only_on_a_link() {
        on_the_qt_thread(|| {
            let (directory, scene) = hosted(
                "solium-hosted-link",
                r#"
                import QtQuick
                Item {
                    Text {
                        x: 0; y: 0; width: 64; height: 16
                        textFormat: Text.StyledText
                        text: "<a href=\"x\">xx</a>"
                        onLinkActivated: (link) => {}
                        HoverHandler { cursorShape: Qt.PointingHandCursor }
                    }
                    Text { x: 0; y: 16; width: 32; height: 16; text: "label" }
                    Text {
                        x: 32; y: 16; width: 32; height: 16
                        textFormat: Text.StyledText
                        text: "<a href=\"x\">xx</a>"
                    }
                }
                "#,
                "link-1",
            );
            let got =
                [(2.0, 8.0), (60.0, 8.0), (5.0, 24.0), (34.0, 24.0)].map(|(x, y)| scene.hit(x, y));
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                got,
                [Hit::Press, Hit::Hover, Hit::Nothing, Hit::Hover],
                "[on the link, on its Text beside it, on a plain label, on a link nothing handles]"
            );
        });
    }

    /// **The Tweaks panel keeps a press on its empty part**: its background
    /// takes input, so a press between its entries is the panel's, not the
    /// window's under it.
    #[test]
    fn the_tweaks_panel_keeps_a_press_on_its_empty_part() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/qml/tweaks.qml"));
            let scene = Scene::for_monitor(path, 64, 32, Some(r#"{"entries":[]}"#), "tweaks-1")
                .expect("the panel builds");
            assert_eq!(scene.hit(60.0, 30.0), Hit::Press);
            drop(scene);
        });
    }

    /// **A disabled pointer handler claims nothing** (Ruling 6): Qt hands it
    /// no event, though its item still accepts every button on its behalf,
    /// so a press there is what is under the scene's.
    #[test]
    fn a_disabled_handler_claims_nothing() {
        on_the_qt_thread(|| {
            let (directory, scene) = hosted(
                "solium-hosted-disabled",
                r"
                import QtQuick
                Item {
                    Item {
                        x: 0; y: 0; width: 20; height: 20
                        TapHandler { enabled: false }
                    }
                    Item {
                        x: 20; y: 0; width: 20; height: 20
                        HoverHandler { enabled: false }
                    }
                    Item {
                        x: 40; y: 0; width: 20; height: 20
                        TapHandler { enabled: false }
                        HoverHandler {}
                    }
                    MouseArea {
                        x: 0; y: 20; width: 20; height: 12
                        TapHandler { enabled: false }
                    }
                }
                ",
                "disabled-1",
            );
            let cases = [
                ((10.0, 10.0), Hit::Nothing, "a disabled TapHandler"),
                ((30.0, 10.0), Hit::Nothing, "a disabled HoverHandler"),
                (
                    (50.0, 10.0),
                    Hit::Hover,
                    "a disabled TapHandler beside a HoverHandler",
                ),
                (
                    (10.0, 25.0),
                    Hit::Press,
                    "a MouseArea whose TapHandler is disabled",
                ),
            ];
            let wrong: Vec<String> = cases
                .iter()
                .filter(|((x, y), wanted, _)| scene.hit(*x, *y) != *wanted)
                .map(|((x, y), wanted, what)| {
                    format!(
                        "{what} at ({x}, {y}): wanted {wanted:?}, got {:?}",
                        scene.hit(*x, *y)
                    )
                })
                .collect();
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert!(wrong.is_empty(), "{wrong:#?}");
        });
    }

    /// **An open Qt Quick Controls popup claims its press**, though Qt draws
    /// it in the window's overlay, beside the scene's root rather than under
    /// it; and the overlay around it, which takes every button for itself,
    /// claims nothing.
    #[test]
    fn an_open_controls_popup_claims_its_press() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted(
                "solium-hosted-popup",
                r"
                import QtQuick
                import QtQuick.Controls
                Item {
                    readonly property int opened: menu.opened ? 1 : 0
                    Popup {
                        id: menu
                        x: 10; y: 0; width: 20; height: 20; padding: 0
                        visible: true
                        enter: null; exit: null
                        closePolicy: Popup.NoAutoClose
                        contentItem: MouseArea {}
                    }
                }
                ",
                "popup-1",
            );
            let got = (
                scene.get_int("opened"),
                scene.hit(20.0, 10.0),
                scene.hit(50.0, 10.0),
            );
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                got,
                (1, Hit::Press, Hit::Nothing),
                "(the popup is open, what it claims, what the overlay beside it claims)"
            );
        });
    }

    /// **A press let go of where the scene could not see is cancelled, not
    /// clicked**: told of a release of no button with none held, as the
    /// session locking tells it, whatever took the press, a `MouseArea`, a
    /// Qt Quick Controls `Button` or a `TapHandler`, one that grabs the
    /// press only passively and one that grabs it for itself
    /// (`TapHandler.WithinBounds`), is no longer pressed and hears
    /// `canceled`, and none of them is clicked.
    #[test]
    fn a_press_let_go_of_unseen_is_cancelled_not_clicked() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted(
                "solium-hosted-cancel",
                r"
                import QtQuick
                import QtQuick.Controls
                Item {
                    id: root
                    readonly property int down: (area.pressed ? 1 : 0) | (button.pressed ? 2 : 0)
                        | (tap.pressed ? 4 : 0) | (within.pressed ? 8 : 0)
                    property int clicked: 0
                    property int canceled: 0
                    MouseArea {
                        id: area
                        width: 20; height: 32
                        onClicked: root.clicked |= 1
                        onCanceled: root.canceled |= 1
                    }
                    Button {
                        id: button
                        x: 22; width: 20; height: 32
                        onClicked: root.clicked |= 2
                        onCanceled: root.canceled |= 2
                    }
                    Item {
                        x: 44; width: 20; height: 16
                        TapHandler {
                            id: tap
                            onTapped: root.clicked |= 4
                            onCanceled: root.canceled |= 4
                        }
                    }
                    Item {
                        x: 44; y: 16; width: 20; height: 16
                        TapHandler {
                            id: within
                            gesturePolicy: TapHandler.WithinBounds
                            onTapped: root.clicked |= 8
                            onCanceled: root.canceled |= 8
                        }
                    }
                }
                ",
                "cancel-1",
            );
            for (x, y, time) in [
                (10.0, 10.0, 1000),
                (32.0, 10.0, 3000),
                (54.0, 8.0, 5000),
                (54.0, 24.0, 7000),
            ] {
                for (kind, buttons, time) in [
                    (PointerKind::Motion, 0, time),
                    (PointerKind::Press(0x1), 0x1, time + 10),
                    (PointerKind::Release(0), 0, time + 500),
                ] {
                    scene.pointer_event(
                        x,
                        y,
                        &ScenePointer {
                            kind,
                            buttons,
                            modifiers: 0,
                            time,
                        },
                    );
                }
            }
            let got = ["down", "clicked", "canceled"].map(|name| scene.get_int(name));
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                got,
                [0, 0, 0b1111],
                "[still pressed, clicked, canceled], one bit each for a MouseArea, a Button, a TapHandler and one that grabs exclusively"
            );
        });
    }

    /// **`Solium.input` on a Qt Quick Controls popup is its item's**: a
    /// `Popup` is not an item, and Qt draws it with one of its own, so
    /// `Solium.input` written on the `Popup` opts that item out, or makes it
    /// hover-only, as it does any item's.
    #[test]
    fn solium_input_on_a_controls_popup_is_its_items() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted(
                "solium-hosted-popup-input",
                r#"
                import QtQuick
                import QtQuick.Controls
                import Solium
                Item {
                    readonly property int opened: (osd.opened ? 1 : 0) + (tip.opened ? 1 : 0)
                    Popup {
                        id: osd
                        x: 0; y: 0; width: 20; height: 20; padding: 0
                        visible: true; enter: null; exit: null
                        closePolicy: Popup.NoAutoClose
                        Solium.input: false
                    }
                    Popup {
                        id: tip
                        x: 30; y: 0; width: 20; height: 20; padding: 0
                        visible: true; enter: null; exit: null
                        closePolicy: Popup.NoAutoClose
                        Solium.input: "hover"
                    }
                }
                "#,
                "popup-input-1",
            );
            let got = (
                scene.get_int("opened"),
                scene.hit(10.0, 10.0),
                scene.hit(40.0, 10.0),
            );
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                got,
                (2, Hit::Nothing, Hit::Hover),
                "(both popups are open, the opted-out one, the hover-only one)"
            );
        });
    }

    /// **A modal Qt Quick Controls popup takes every point of its scene
    /// while it is open**, as Qt's own modality does: its dim covers the
    /// whole scene and takes every press, so nothing under the surface is
    /// clicked until it closes.
    #[test]
    fn a_modal_popup_takes_every_point_of_its_scene_while_it_is_open() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted(
                "solium-hosted-modal",
                r"
                import QtQuick
                import QtQuick.Controls
                Item {
                    readonly property int opened: dialog.opened ? 1 : 0
                    Popup {
                        id: dialog
                        x: 10; y: 0; width: 20; height: 20; padding: 0
                        modal: true
                        visible: true
                        enter: null; exit: null
                        closePolicy: Popup.NoAutoClose
                    }
                }
                ",
                "modal-1",
            );
            let got = (
                scene.get_int("opened"),
                [(20.0, 10.0), (50.0, 10.0), (2.0, 30.0)].map(|(x, y)| scene.hit(x, y)),
            );
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                got,
                (1, [Hit::Press, Hit::Press, Hit::Press]),
                "(the popup is open, [on it, beside it, in a far corner])"
            );
        });
    }

    /// **A Qt Quick Controls popup is laid out against the whole scene**:
    /// one that keeps inside its window (`margins: 0`, as a `Menu`, a
    /// `ToolTip` and a `ComboBox`'s list do) opens where it was asked and
    /// claims the points there, not at the middle of the scene.
    #[test]
    fn a_controls_popup_is_laid_out_against_the_whole_scene() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted(
                "solium-hosted-popup-bounds",
                r"
                import QtQuick
                import QtQuick.Controls
                Item {
                    readonly property int opened: menu.opened ? 1 : 0
                    Popup {
                        id: menu
                        x: 10; y: 0; width: 20; height: 20; padding: 0; margins: 0
                        visible: true
                        enter: null; exit: null
                        closePolicy: Popup.NoAutoClose
                        contentItem: MouseArea {}
                    }
                }
                ",
                "popup-bounds-1",
            );
            let got = (
                scene.get_int("opened"),
                scene.hit(20.0, 10.0),
                scene.hit(50.0, 25.0),
            );
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                got,
                (1, Hit::Press, Hit::Nothing),
                "(the popup is open, where it was asked, the middle of the scene)"
            );
        });
    }

    /// **A scene told the pointer left un-hovers what it hovered.**
    #[test]
    fn a_left_scene_drops_its_hover() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted(
                "solium-hosted-leave",
                "import QtQuick\nItem { readonly property int hovered: area.containsMouse ? 1 : 0\n MouseArea { id: area; anchors.fill: parent; hoverEnabled: true } }\n",
                "leave-1",
            );
            scene.pointer_event(
                10.0,
                10.0,
                &ScenePointer {
                    kind: PointerKind::Motion,
                    buttons: 0,
                    modifiers: 0,
                    time: 0,
                },
            );
            assert_eq!(scene.get_int("hovered"), 1);
            scene.leave();
            assert_eq!(
                scene.get_int("hovered"),
                0,
                "the hover outlived the pointer leaving"
            );
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    const BUTTONS: &str = r"
        import QtQuick
        Item {
            property int pressed: 0
            property int shifted: 0
            property int angle: 0
            MouseArea {
                anchors.fill: parent
                acceptedButtons: Qt.AllButtons
                onPressed: (mouse) => {
                    parent.pressed = mouse.button
                    parent.shifted = (mouse.modifiers & Qt.ShiftModifier) ? 1 : 0
                }
            }
            WheelHandler { onWheel: (event) => parent.angle = event.angleDelta.y }
        }
    ";

    /// **A right press reaches the scene as the right button, with the
    /// modifiers held** (#163). It used to arrive as a left press.
    #[test]
    fn a_right_press_reaches_a_mouse_area_as_the_right_button() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted("solium-hosted-right", BUTTONS, "right-1");
            let press = ScenePointer {
                kind: PointerKind::Press(0x2),
                buttons: 0x2,
                modifiers: crate::qml::keys::QT_SHIFT,
                time: 0,
            };
            scene.pointer_event(10.0, 10.0, &press);
            assert_eq!(scene.get_int("pressed"), 2, "Qt.RightButton is 2");
            assert_eq!(scene.get_int("shifted"), 1, "shift was held");
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    #[test]
    fn a_side_button_reaches_the_scene_as_back() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted("solium-hosted-back", BUTTONS, "back-1");
            scene.pointer_event(
                10.0,
                10.0,
                &ScenePointer {
                    kind: PointerKind::Press(0x8),
                    buttons: 0x8,
                    modifiers: 0,
                    time: 0,
                },
            );
            assert_eq!(scene.get_int("pressed"), 8, "Qt.BackButton is 8");
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    #[test]
    fn the_wheel_reaches_a_wheel_handler_with_its_angle() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted("solium-hosted-wheel", BUTTONS, "wheel-1");
            let wheel = ScenePointer {
                kind: PointerKind::Wheel {
                    angle: (0.0, 120.0),
                    pixels: (0.0, 0.0),
                },
                buttons: 0,
                modifiers: 0,
                time: 0,
            };
            scene.pointer_event(10.0, 10.0, &wheel);
            assert_eq!(scene.get_int("angle"), 120, "one notch away from the user");
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    /// A left click at `(x, y)`: the press at `time`, the release 20 ms on.
    fn click(scene: &mut Scene, (x, y): (f64, f64), time: u64) {
        for (kind, buttons, time) in [
            (PointerKind::Press(0x1), 0x1, time),
            (PointerKind::Release(0x1), 0, time + 20),
        ] {
            scene.pointer_event(
                x,
                y,
                &ScenePointer {
                    kind,
                    buttons,
                    modifiers: 0,
                    time,
                },
            );
        }
    }

    /// **A double press on a `MouseArea` is one double-click**, by Qt's own
    /// rule: a second press of the same button, sooner than
    /// `QStyleHints::mouseDoubleClickInterval` after the first and no
    /// further than `mouseDoubleClickDistance` from it. A press far from the
    /// one before is a click of its own, and so is a third press right after
    /// a double-click.
    #[test]
    fn a_double_press_on_a_mouse_area_is_one_double_click() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted(
                "solium-hosted-double",
                r"
                import QtQuick
                Item {
                    property int clicks: 0
                    property int doubles: 0
                    MouseArea {
                        anchors.fill: parent
                        onClicked: parent.clicks += 1
                        onDoubleClicked: parent.doubles += 1
                    }
                }
                ",
                "double-1",
            );
            click(&mut scene, (10.0, 10.0), 1000);
            click(&mut scene, (40.0, 10.0), 1100);
            click(&mut scene, (41.0, 10.0), 1200);
            click(&mut scene, (41.0, 10.0), 1300);
            let got = (scene.get_int("clicks"), scene.get_int("doubles"));
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                got,
                (3, 1),
                "(clicks, double-clicks) of a press, one far from it, one that doubles that, and a third"
            );
        });
    }

    /// **A `TapHandler` counts taps by when they happened**: two quick taps
    /// are a double tap, and a tap long after them is a single one again.
    #[test]
    fn a_tap_handler_counts_taps_by_when_they_happened() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted(
                "solium-hosted-taps",
                r"
                import QtQuick
                Item {
                    property int count: 0
                    property int singles: 0
                    property int doubles: 0
                    TapHandler {
                        onTapped: parent.count = tapCount
                        onSingleTapped: parent.singles += 1
                        onDoubleTapped: parent.doubles += 1
                    }
                }
                ",
                "taps-1",
            );
            click(&mut scene, (10.0, 10.0), 1000);
            click(&mut scene, (10.0, 10.0), 1100);
            let quick = scene.get_int("count");
            click(&mut scene, (10.0, 10.0), 5000);
            let got = (
                quick,
                scene.get_int("count"),
                scene.get_int("singles"),
                scene.get_int("doubles"),
            );
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                got,
                (2, 1, 2, 1),
                "(tapCount after two quick taps, after a slow one, singleTapped, doubleTapped)"
            );
        });
    }

    /// **Two presses further apart than the double-click interval are two
    /// single clicks**, to a `MouseArea` and to a `TapHandler` alike.
    #[test]
    fn two_presses_further_apart_than_the_interval_are_two_single_clicks() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted(
                "solium-hosted-slow",
                r"
                import QtQuick
                Item {
                    id: root
                    property int clicks: 0
                    property int doubles: 0
                    property int singles: 0
                    property int doubleTaps: 0
                    MouseArea {
                        width: 30; height: 32
                        onClicked: root.clicks += 1
                        onDoubleClicked: root.doubles += 1
                    }
                    Item {
                        x: 32; width: 32; height: 32
                        TapHandler {
                            onSingleTapped: root.singles += 1
                            onDoubleTapped: root.doubleTaps += 1
                        }
                    }
                }
                ",
                "slow-1",
            );
            for (at, time) in [
                ((10.0, 10.0), 1000),
                ((10.0, 10.0), 3000),
                ((50.0, 10.0), 5000),
                ((50.0, 10.0), 7000),
            ] {
                click(&mut scene, at, time);
            }
            let got =
                ["clicks", "doubles", "singles", "doubleTaps"].map(|name| scene.get_int(name));
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                got,
                [2, 0, 2, 0],
                "[MouseArea clicks, its double-clicks, TapHandler single taps, its double taps]"
            );
        });
    }

    /// `qml` written to `<temp>/<name>/Scene.qml`, built hosted on `monitor`.
    /// Monitor rows are process-wide, so every test names a monitor of its own.
    pub(crate) fn hosted(name: &str, qml: &str, monitor: &str) -> (PathBuf, Scene) {
        crate::qml::start().expect("Qt starts");
        let directory = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let path = directory.join("Scene.qml");
        std::fs::write(&path, qml).expect("writing the scene");
        let scene = Scene::for_monitor(&path, 64, 32, None, monitor).expect("the scene builds");
        (directory, scene)
    }

    /// **The attached `Solium` type shares the URI `Solium` with the shipped
    /// QML module.** `Theme` is the module's, `Solium.monitor` is the C++
    /// type's, and one `import Solium` reaches both. A monitor no row was
    /// published for is neither `present` nor `valid`.
    #[test]
    fn the_attached_type_shares_the_solium_uri_with_the_shipped_module() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted(
                "solium-hosted-s2",
                r#"
                import QtQuick
                import Solium
                Item {
                    readonly property int themed: Theme.accent !== undefined ? 1 : 0
                    readonly property int named: Solium.monitor.name === "s2-1" ? 1 : 0
                    readonly property int present: Solium.monitor.present ? 1 : 0
                    readonly property int valid: Solium.monitor.valid ? 1 : 0
                }
                "#,
                "s2-1",
            );
            assert_eq!(
                scene.get_int("themed"),
                1,
                "Theme did not resolve beside the attached type"
            );
            assert_eq!(
                scene.get_int("named"),
                1,
                "Solium.monitor does not name the monitor it is hosted on"
            );
            assert_eq!(
                scene.get_int("present"),
                0,
                "no row was published, so the monitor is not present yet"
            );
            assert_eq!(
                scene.get_int("valid"),
                0,
                "no row was published, so the monitor is not valid yet"
            );
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    /// **A scene built for no monitor reads an absent one, never `null`**:
    /// a window frame or the pointer can bind `Solium.monitor` and get a
    /// value.
    #[test]
    fn a_scene_hosted_on_no_monitor_reads_an_absent_monitor() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let directory = std::env::temp_dir().join("solium-hosted-none");
            let _ = std::fs::create_dir_all(&directory);
            let path = directory.join("Scene.qml");
            std::fs::write(
                &path,
                r#"
                import QtQuick
                import Solium
                Item {
                    readonly property int unnamed: Solium.monitor.name === "" ? 1 : 0
                    readonly property int present: Solium.monitor.present ? 1 : 0
                }
                "#,
            )
            .expect("writing the scene");
            let mut scene = Scene::for_host(&path, 16, 16, None).expect("the scene builds");
            assert_eq!(scene.get_int("unnamed"), 1);
            assert_eq!(scene.get_int("present"), 0);
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    /// **A hosted build that fails leaves the next scene unhosted**: the
    /// monitor handed over for one build is never inherited by another.
    #[test]
    fn a_scene_built_after_a_failed_hosted_one_is_not_hosted() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let missing = std::env::temp_dir().join("solium-hosted-missing/Nothing.qml");
            assert!(Scene::for_monitor(&missing, 16, 16, None, "inherited-1").is_err());
            let directory = std::env::temp_dir().join("solium-hosted-after");
            let _ = std::fs::create_dir_all(&directory);
            let path = directory.join("Scene.qml");
            std::fs::write(
                &path,
                "import QtQuick\nimport Solium\nItem { readonly property int unnamed: Solium.monitor.name === \"\" ? 1 : 0 }\n",
            )
            .expect("writing the scene");
            let mut scene = Scene::for_host(&path, 16, 16, None).expect("the scene builds");
            assert_eq!(
                scene.get_int("unnamed"),
                1,
                "the failed build's monitor was inherited"
            );
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    /// **Every object of a hosted scene finds its monitor, whenever it asks**:
    /// an item from another file, and one a `Loader` makes only after a second
    /// scene was built on another monitor, both read their own scene's
    /// monitor, so the record is the scene's and not the build's.
    #[test]
    fn every_object_of_a_hosted_scene_finds_its_monitor_after_the_build() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let directory = std::env::temp_dir().join("solium-hosted-late");
            let _ = std::fs::remove_dir_all(&directory);
            std::fs::create_dir_all(&directory).expect("a temporary directory");
            std::fs::write(
                directory.join("Inner.qml"),
                "import QtQuick\nimport Solium\nItem { readonly property string seen: Solium.monitor.name }\n",
            )
            .expect("writing the inner item");
            let path = directory.join("Scene.qml");
            std::fs::write(
                &path,
                r#"
                import QtQuick
                import Solium
                Item {
                    property int go: 0
                    Inner { id: inner }
                    Loader { id: late; active: go === 1; sourceComponent: Inner {} }
                    readonly property int nested: inner.seen === "late-a" ? 1 : 0
                    readonly property int loaded: late.item !== null && late.item.seen === "late-a" ? 1 : 0
                }
                "#,
            )
            .expect("writing the scene");
            let mut first =
                Scene::for_monitor(&path, 16, 16, None, "late-a").expect("the scene builds");
            let second =
                Scene::for_monitor(&path, 16, 16, None, "late-b").expect("the scene builds");
            assert_eq!(
                first.get_int("nested"),
                1,
                "an item from another file does not find its monitor"
            );
            assert_eq!(
                first.get_int("loaded"),
                0,
                "the Loader was active before it was asked"
            );
            first.set_int("go", 1);
            assert_eq!(
                first.get_int("loaded"),
                1,
                "an item made after the build does not find its scene's monitor"
            );
            drop(second);
            drop(first);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    /// **A hosted build refused before the host is reached leaves the next
    /// scene unhosted**: `for_monitor` takes the monitor back itself, so a
    /// path the host never saw does not hand its monitor on.
    #[test]
    fn a_hosted_build_refused_before_the_host_leaves_the_next_scene_unhosted() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let refused = std::path::Path::new("solium-hosted-refused\0/Scene.qml");
            assert!(Scene::for_monitor(refused, 16, 16, None, "refused-1").is_err());
            let directory = std::env::temp_dir().join("solium-hosted-after-refused");
            let _ = std::fs::create_dir_all(&directory);
            let path = directory.join("Scene.qml");
            std::fs::write(
                &path,
                "import QtQuick\nimport Solium\nItem { readonly property int unnamed: Solium.monitor.name === \"\" ? 1 : 0 }\n",
            )
            .expect("writing the scene");
            let mut scene = Scene::for_host(&path, 16, 16, None).expect("the scene builds");
            assert_eq!(
                scene.get_int("unnamed"),
                1,
                "the refused build's monitor was inherited"
            );
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    /// **The host takes the monitor with the build it is for**: a build the
    /// host fails still consumes what `solium_qml_host_next_on` handed over,
    /// so the scene after it is unhosted with nobody clearing it.
    #[test]
    #[expect(unsafe_code, reason = "handing the host a monitor directly")]
    fn the_host_consumes_the_monitor_even_for_a_build_that_fails() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let name = std::ffi::CString::new("consumed-1").expect("no NUL in the name");
            // SAFETY: `name` outlives the call; the host copies it.
            unsafe { super::ffi::solium_qml_host_next_on(name.as_ptr()) };
            let missing = std::env::temp_dir().join("solium-hosted-consumed-missing/Nothing.qml");
            assert!(Scene::for_host(&missing, 16, 16, None).is_err());
            let directory = std::env::temp_dir().join("solium-hosted-after-consumed");
            let _ = std::fs::create_dir_all(&directory);
            let path = directory.join("Scene.qml");
            std::fs::write(
                &path,
                "import QtQuick\nimport Solium\nItem { readonly property int unnamed: Solium.monitor.name === \"\" ? 1 : 0 }\n",
            )
            .expect("writing the scene");
            let mut scene = Scene::for_host(&path, 16, 16, None).expect("the scene builds");
            assert_eq!(
                scene.get_int("unnamed"),
                1,
                "the failed build left its monitor for the next scene"
            );
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    fn monitor_row(name: &str, width: i32, scale: f64) -> crate::models::diff::Row {
        crate::models::diff::Row {
            key: name.to_owned(),
            values: std::collections::BTreeMap::from([
                ("name", crate::json::Json::Text(name.to_owned())),
                (
                    "whole",
                    crate::models::monitors::rect(smithay::utils::Rectangle::new(
                        (0, 0).into(),
                        (1920, 1080).into(),
                    )),
                ),
                (
                    "area",
                    crate::models::monitors::rect(smithay::utils::Rectangle::new(
                        (0, 0).into(),
                        (width, 1080).into(),
                    )),
                ),
                ("scale", crate::json::Json::Number(scale)),
                ("transform", crate::json::Json::Text("normal".to_owned())),
                ("primary", crate::json::Json::Bool(true)),
            ]),
        }
    }

    /// **A published monitor is `Solium.monitor` in a scene on it**, every
    /// batch is announced once however many values it changed, a monitor
    /// that goes reads absent and keeps its name, and the same connector
    /// coming back is the same row again.
    #[test]
    fn a_published_monitor_reaches_solium_monitor_in_its_scene() {
        use crate::models::diff::{diff, render};

        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted(
                "solium-hosted-rows",
                r#"
                import QtQuick
                import Solium
                Item {
                    readonly property int areaWidth: Solium.monitor.area.width
                    readonly property int present: Solium.monitor.present ? 1 : 0
                    readonly property int named: Solium.monitor.name === "rows-2" ? 1 : 0
                    property int changes: 0
                    Connections { target: Solium.monitor; function onChanged() { changes += 1 } }
                }
                "#,
                "rows-2",
            );
            let first = vec![monitor_row("rows-2", 1820, 1.0)];
            assert!(super::apply_rows(
                super::Model::Monitors,
                &render(&diff(&[], &first))
            ));
            assert_eq!(
                (
                    scene.get_int("present"),
                    scene.get_int("areaWidth"),
                    scene.get_int("changes")
                ),
                (1, 1820, 1)
            );

            let second = vec![monitor_row("rows-2", 1800, 2.0)];
            assert!(super::apply_rows(
                super::Model::Monitors,
                &render(&diff(&first, &second))
            ));
            assert_eq!(scene.get_int("areaWidth"), 1800);
            assert_eq!(
                scene.get_int("changes"),
                2,
                "two values in one batch were announced more than once"
            );

            assert!(super::apply_rows(
                super::Model::Monitors,
                &render(&diff(&second, &[]))
            ));
            assert_eq!(
                (scene.get_int("present"), scene.get_int("named")),
                (0, 1),
                "a gone monitor must read absent and keep its name"
            );

            assert!(super::apply_rows(
                super::Model::Monitors,
                &render(&diff(&[], &first))
            ));
            assert_eq!(
                (scene.get_int("present"), scene.get_int("areaWidth")),
                (1, 1820),
                "the connector came back, and the scene's row did not"
            );
            assert_eq!(
                scene.get_int("changes"),
                4,
                "the row the scene first got is not the one that came back"
            );
            assert!(super::apply_rows(
                super::Model::Monitors,
                &render(&diff(&first, &[]))
            ));
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    /// **A batch that does not match the rows held is refused**, and so is
    /// text that is not a batch, so a publish that was not taken is sent
    /// again rather than recorded as taken.
    #[test]
    fn a_batch_that_does_not_match_the_rows_held_is_refused() {
        use crate::models::diff::{diff, render};

        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let held = vec![monitor_row("refused-rows-1", 1920, 1.0)];
            assert!(super::apply_rows(
                super::Model::Monitors,
                &render(&diff(&[], &held))
            ));
            let elsewhere = vec![monitor_row("refused-rows-2", 1920, 1.0)];
            assert!(
                !super::apply_rows(super::Model::Monitors, &render(&diff(&elsewhere, &[]))),
                "a remove of a row that is not there was taken"
            );
            assert!(
                !super::apply_rows(super::Model::Monitors, "not a batch"),
                "text that is not a batch was taken"
            );
            assert!(super::apply_rows(
                super::Model::Monitors,
                &render(&diff(&held, &[]))
            ));
        });
    }

    /// **A refused batch takes none of its steps**: a batch whose last step
    /// does not match leaves every row as it was, and a row already held is
    /// never inserted again, so a batch sent again from what Qt last took
    /// lands once.
    #[test]
    fn a_refused_batch_takes_none_of_its_steps() {
        use crate::models::diff::{Op, diff, render};

        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted(
                "solium-hosted-atomic",
                r#"
                import QtQuick
                import Solium
                Item { readonly property int present: Solium.monitor.present ? 1 : 0 }
                "#,
                "atomic-1",
            );
            let held = vec![monitor_row("atomic-1", 1920, 1.0)];
            let refused = vec![
                Op::Insert {
                    at: 0,
                    row: monitor_row("atomic-1", 1920, 1.0),
                },
                Op::Remove {
                    at: 1,
                    key: "atomic-none".to_owned(),
                },
            ];
            assert!(!super::apply_rows(
                super::Model::Monitors,
                &render(&refused)
            ));
            assert_eq!(
                scene.get_int("present"),
                0,
                "a refused batch's first step was taken"
            );

            assert!(super::apply_rows(
                super::Model::Monitors,
                &render(&diff(&[], &held))
            ));
            assert_eq!(scene.get_int("present"), 1);
            assert!(
                !super::apply_rows(super::Model::Monitors, &render(&diff(&[], &held))),
                "a row already held was inserted again"
            );
            assert!(super::apply_rows(
                super::Model::Monitors,
                &render(&diff(&held, &[]))
            ));
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    /// **A shell's own file is not shadowed by a row's type**: a `Monitor.qml`
    /// beside a scene that imports `Solium` is still the scene's `Monitor`.
    #[test]
    fn a_shell_file_named_like_a_row_is_still_the_shells() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let directory = std::env::temp_dir().join("solium-hosted-shadow");
            let _ = std::fs::remove_dir_all(&directory);
            std::fs::create_dir_all(&directory).expect("a temporary directory");
            std::fs::write(
                directory.join("Monitor.qml"),
                "import QtQuick\nItem { readonly property int mine: 1 }\n",
            )
            .expect("writing the shell's own file");
            let path = directory.join("Scene.qml");
            std::fs::write(
                &path,
                "import QtQuick\nimport Solium\nItem {\n    Monitor { id: own }\n    readonly property int mine: own.mine\n}\n",
            )
            .expect("writing the scene");
            let mut scene = Scene::for_monitor(&path, 16, 16, None, "shadow-1")
                .expect("a scene using its own Monitor.qml builds");
            assert_eq!(
                scene.get_int("mine"),
                1,
                "Monitor is not the shell's own file"
            );
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }
}
