//! The compositor's half of what a hosted scene and the compositor say to each
//! other: properties written in place, the monitor it is on, the models' rows,
//! pointer events, what its items claim of the pointer
//! (`tests::the_item_tree_decides_what_a_point_claims`), what it reserves
//! (`tests::a_scene_reserve_is_reported_once_per_change`), and its grabs
//! (`tests::a_grab_is_held_while_active_and_dismissed_on_request`), and its
//! keyboard wants and the keys it is told
//! (`tests::a_field_that_wants_the_keyboard_reports_its_claims`,
//! `tests::text_typed_on_russian_reaches_the_field`), and the actions it sends
//! (`tests::solium_send_queues_every_action_with_its_data_in_order`).

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
        pub(super) fn solium_qml_scene_take_reserve(
            scene: *mut super::super::ffi::Scene,
            edges: *mut c_int,
        ) -> c_int;
        pub(super) fn solium_qml_scene_take_grab(
            scene: *mut super::super::ffi::Scene,
            name: *mut *const c_char,
        ) -> c_int;
        pub(super) fn solium_qml_scene_grab_contains(
            scene: *const super::super::ffi::Scene,
            x: f64,
            y: f64,
        ) -> c_int;
        pub(super) fn solium_qml_scene_dismiss(scene: *mut super::super::ffi::Scene);
        pub(super) fn solium_qml_scene_take_keyboard(
            scene: *mut super::super::ffi::Scene,
            claims: *mut *const c_char,
        ) -> c_int;
        pub(super) fn solium_qml_scene_key(
            scene: *mut super::super::ffi::Scene,
            pressed: c_int,
            qt_key: c_int,
            modifiers: u32,
            text: *const c_char,
            autorepeat: c_int,
            scan_code: u32,
        );
        pub(super) fn solium_qml_scene_let_go_keyboard(scene: *mut super::super::ffi::Scene);
        pub(super) fn solium_qml_scene_take_action(
            scene: *mut super::super::ffi::Scene,
            action: *mut *const c_char,
            data_json: *mut *const c_char,
        ) -> c_int;
    }
}

/// A model's number, as `host.h`'s `SOLIUM_QML_ROWS_*` say it.
/// `tests::a_published_monitor_reaches_solium_monitor_in_its_scene`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Model {
    Monitors = 0,
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

    /// The scene's reserve, when it changed since it was last asked.
    /// `tests::a_scene_reserve_is_reported_once_per_change`.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn take_reserve(&mut self) -> Option<crate::scripted::SceneReserve> {
        let mut edges: [c_int; 4] = [-1; 4];
        // SAFETY: the scene is live for as long as `self`, and `edges` has
        // room for the four the host writes.
        if unsafe { ffi::solium_qml_scene_take_reserve(self.scene, edges.as_mut_ptr()) } == 0 {
            return None;
        }
        let edge = |value: c_int| (value >= 0).then_some(value);
        Some(crate::scripted::SceneReserve {
            top: edge(edges[0]),
            right: edge(edges[1]),
            bottom: edge(edges[2]),
            left: edge(edges[3]),
        })
    }
}

/// What a scene says of its grabs since it was last asked (Ruling 12).
/// `tests::a_grab_is_held_while_active_and_dismissed_on_request`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GrabReport {
    Unchanged,
    /// No grab is active now.
    Released,
    /// One is, and this is the newest's name.
    /// `tests::a_scenes_newest_grab_is_reported_and_every_active_one_counts`.
    Held(String),
}

impl Scene {
    /// The scene's grab, when it changed since it was last asked; a scene
    /// says what it has at its first take.
    /// `tests::a_grab_is_held_while_active_and_dismissed_on_request`,
    /// `tests::a_scene_with_no_active_grab_says_so_once`.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn take_grab(&mut self) -> GrabReport {
        let mut name: *const c_char = std::ptr::null();
        // SAFETY: the scene is live for as long as `self`, and the host sets
        // `name` only when it returns 1.
        match unsafe { ffi::solium_qml_scene_take_grab(self.scene, &raw mut name) } {
            1 if !name.is_null() => {
                // SAFETY: a NUL-terminated string the host keeps valid until
                // its next call, copied here before any.
                let held = unsafe { std::ffi::CStr::from_ptr(name) };
                GrabReport::Held(held.to_string_lossy().into_owned())
            }
            0 => GrabReport::Released,
            _ => GrabReport::Unchanged,
        }
    }

    /// Whether a point in scene coordinates is inside any active grab's
    /// target. `tests::a_scenes_newest_grab_is_reported_and_every_active_one_counts`.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn grab_contains(&self, x: f64, y: f64) -> bool {
        // SAFETY: the scene is live for as long as `self`.
        unsafe { ffi::solium_qml_scene_grab_contains(self.scene, x, y) != 0 }
    }

    /// Dismiss every active grab, newest first.
    /// `tests::a_scenes_newest_grab_is_reported_and_every_active_one_counts`.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn dismiss(&mut self) {
        // `onDismissed` runs the scene's own code, which can move its items.
        // `surface::tests::a_cached_hit_follows_a_dismissal`.
        touched();
        // SAFETY: the scene is live for as long as `self`.
        unsafe { ffi::solium_qml_scene_dismiss(self.scene) }
    }
}

/// What a scene says of its keyboard wants since it was last asked (Ruling
/// 14). `tests::a_field_that_wants_the_keyboard_reports_its_claims`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum KeyboardReport {
    Unchanged,
    /// No visible item wants the keyboard now.
    /// `tests::an_invisible_field_does_not_hold_the_keyboard`.
    LetGo,
    /// One does, and these are the keys it claims, as the scene spells them.
    Wanted(Vec<String>),
}

/// One key as a scene is told it.
/// `tests::text_typed_on_russian_reaches_the_field`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SceneKey {
    pub(crate) pressed: bool,
    /// A `Qt::Key`.
    pub(crate) qt_key: i32,
    /// Qt's `KeyboardModifiers`.
    pub(crate) modifiers: u32,
    /// What it types, from the compositor's xkb state with the active group.
    /// `input::tests::while_the_shell_holds_the_keyboard_russian_letters_reach_it_as_cyrillic`.
    pub(crate) text: String,
    pub(crate) autorepeat: bool,
    /// Whether it repeats while held, as the keymap says: no modifier does,
    /// nor a group toggle.
    /// `input::tests::a_held_group_toggle_does_not_repeat_into_the_scene`.
    pub(crate) repeats: bool,
    /// The xkb keycode, so a release finds its press and a repeat its key.
    /// `input::tests::a_release_whose_press_went_to_a_scene_reaches_no_window`.
    pub(crate) code: u32,
}

impl Scene {
    /// Who in the scene wants the keyboard, when that changed since it was
    /// last asked; a scene says what it has at its first take.
    /// `tests::a_field_that_wants_the_keyboard_reports_its_claims`,
    /// `tests::an_invisible_field_does_not_hold_the_keyboard`.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn take_keyboard(&mut self) -> KeyboardReport {
        let mut claims: *const c_char = std::ptr::null();
        // SAFETY: the scene is live for as long as `self`, and the host sets
        // `claims` only when it returns 1.
        match unsafe { ffi::solium_qml_scene_take_keyboard(self.scene, &raw mut claims) } {
            1 if !claims.is_null() => {
                // SAFETY: a NUL-terminated string the host keeps valid until
                // its next call, copied here before any.
                let text = unsafe { std::ffi::CStr::from_ptr(claims) }.to_string_lossy();
                KeyboardReport::Wanted(
                    text.split('\n')
                        .filter(|claim| !claim.is_empty())
                        .map(str::to_owned)
                        .collect(),
                )
            }
            0 => KeyboardReport::LetGo,
            _ => KeyboardReport::Unchanged,
        }
    }

    /// Tell the scene one key.
    /// `tests::text_typed_on_russian_reaches_the_field`.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn key(&mut self, key: &SceneKey) {
        // A key's handlers are the scene's own code, which can move its items.
        // `tests::a_key_the_scene_hears_moves_its_items_for_the_next_hit`.
        touched();
        let Ok(text) = CString::new(key.text.as_str()) else {
            return;
        };
        // SAFETY: the scene is live for as long as `self`, and `text`
        // outlives the call.
        unsafe {
            ffi::solium_qml_scene_key(
                self.scene,
                c_int::from(key.pressed),
                key.qt_key,
                key.modifiers,
                text.as_ptr(),
                c_int::from(key.autorepeat),
                key.code,
            );
        }
    }

    /// The compositor has taken the keyboard back: the item holding it
    /// loses its focus, and no item that wanted it holds it again until it
    /// asks anew. `tests::a_field_that_wants_the_keyboard_reports_its_claims`,
    /// `tests::a_scene_let_go_of_takes_the_keyboard_again_only_when_asked_anew`.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn let_go_keyboard(&mut self) {
        touched();
        // SAFETY: the scene is live for as long as `self`.
        unsafe { ffi::solium_qml_scene_let_go_keyboard(self.scene) }
    }

    /// The oldest action the scene queued with `Solium.send`, with its data,
    /// `Json::Null` for none (Ruling 15).
    /// `tests::solium_send_queues_every_action_with_its_data_in_order`,
    /// `tests::an_unhosted_scene_may_send_and_queues_nothing`.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn take_action(&mut self) -> Option<(String, Json)> {
        let mut action: *const c_char = std::ptr::null();
        let mut data: *const c_char = std::ptr::null();
        // SAFETY: the scene is live for as long as `self`, and the host sets
        // both only when it returns 1.
        let taken = unsafe {
            ffi::solium_qml_scene_take_action(self.scene, &raw mut action, &raw mut data)
        };
        if taken == 0 || action.is_null() || data.is_null() {
            return None;
        }
        // SAFETY: NUL-terminated strings the host keeps valid until its next
        // call, copied here before any.
        let (action, data) = unsafe {
            (
                std::ffi::CStr::from_ptr(action)
                    .to_string_lossy()
                    .into_owned(),
                std::ffi::CStr::from_ptr(data)
                    .to_string_lossy()
                    .into_owned(),
            )
        };
        let data = Json::parse(&data)
            .and_then(|wrapped| wrapped.get("data").cloned())
            .unwrap_or(Json::Null);
        Some((action, data))
    }

    /// A one-value string property, read as a list of one.
    #[cfg(test)]
    pub(crate) fn get_string_for_test(&self, name: &str) -> String {
        self.string_list(name)
            .into_iter()
            .next()
            .unwrap_or_default()
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

    use super::{GrabReport, Hit, KeyboardReport, PointerKind, SceneKey, ScenePointer};
    use crate::qml::{Scene, qt_test::on_the_qt_thread};
    use crate::scripted::SceneReserve;

    /// A field that wants the keyboard while it has the focus, claiming two
    /// keys, and a button beside it that hides on Escape.
    const FIELD: &str = r#"
        import QtQuick
        import QtQuick.Controls
        import Solium
        Item {
            property bool shown: true
            property bool buttonShown: true
            readonly property string typed: field.text
            TextField {
                id: field
                width: 40; height: 20
                visible: parent.shown
                focus: true
                Solium.keyboard.wants: activeFocus
                Solium.keyboard.claims: [ "Escape", "Return" ]
                Keys.onEscapePressed: parent.buttonShown = false
            }
            MouseArea { x: 44; width: 20; height: 20; visible: parent.buttonShown }
        }
    "#;

    /// One key pressed and let go, as the compositor tells it.
    fn tap(scene: &mut Scene, code: u32, qt_key: i32, text: &str) {
        for pressed in [true, false] {
            scene.key(&SceneKey {
                pressed,
                qt_key,
                modifiers: 0,
                text: text.to_owned(),
                autorepeat: false,
                repeats: true,
                code,
            });
        }
    }

    /// **A field that wants the keyboard reports its claims** (Ruling 14:
    /// the hosted window is active from the start, so `focus: true` is
    /// enough), once, as the scene spells them, and letting go takes its
    /// focus, so it lets go of the keyboard too.
    #[test]
    fn a_field_that_wants_the_keyboard_reports_its_claims() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted("solium-hosted-field", FIELD, "field-1");
            let wanted = scene.take_keyboard();
            let again = scene.take_keyboard();
            scene.let_go_keyboard();
            let let_go = scene.take_keyboard();
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                (wanted, again, let_go),
                (
                    KeyboardReport::Wanted(vec!["Escape".to_owned(), "Return".to_owned()]),
                    KeyboardReport::Unchanged,
                    KeyboardReport::LetGo,
                ),
                "(the first take, a second with no change, the take after letting go)"
            );
        });
    }

    /// **Text typed on Russian reaches a `TextField` as Cyrillic** (#132): the
    /// text is the compositor's, from xkb with the active group, and the
    /// field types what it is told.
    #[test]
    fn text_typed_on_russian_reaches_the_field() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted("solium-hosted-russian", FIELD, "russian-1");
            let _ = scene.take_keyboard();
            for (code, text) in [(41_u32, "п"), (27, "р"), (44, "о")] {
                let qt_key =
                    crate::qml::keys::qt_key(smithay::input::keyboard::Keysym::NoSymbol, text);
                tap(&mut scene, code, qt_key, text);
            }
            // BackSpace, which the field takes as a named key.
            tap(&mut scene, 22, 0x0100_0003, "\u{8}");
            let typed = scene.get_string_for_test("typed");
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(typed, "пр");
        });
    }

    /// **A field that is hidden lets go of the keyboard**: only a visible
    /// item holds it.
    #[test]
    fn an_invisible_field_does_not_hold_the_keyboard() {
        on_the_qt_thread(|| {
            let (directory, mut scene) =
                hosted("solium-hosted-hidden-field", FIELD, "hidden-field-1");
            let shown = scene.take_keyboard();
            scene.set_bool("shown", false);
            let hidden = scene.take_keyboard();
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                (matches!(shown, KeyboardReport::Wanted(_)), hidden),
                (true, KeyboardReport::LetGo),
                "(the field shown wanted it, the take once it is hidden)"
            );
        });
    }

    /// **The item holding the keyboard is the wanting one with active focus,
    /// else the one that came to want it last** (Ruling 14): its claims are
    /// the ones reported.
    #[test]
    fn the_holder_is_the_focused_wanting_item_else_the_one_that_wanted_last() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted(
                "solium-hosted-holder",
                r#"
                import QtQuick
                import Solium
                Item {
                    id: root
                    property bool first: false
                    property bool second: false
                    property bool focusFirst: false
                    Item {
                        focus: root.focusFirst
                        Solium.keyboard.wants: root.first
                        Solium.keyboard.claims: [ "Up" ]
                    }
                    Item {
                        Solium.keyboard.wants: root.second
                        Solium.keyboard.claims: [ "Down" ]
                    }
                }
                "#,
                "holder-1",
            );
            let none = scene.take_keyboard();
            scene.set_bool("first", true);
            let first = scene.take_keyboard();
            scene.set_bool("second", true);
            let second = scene.take_keyboard();
            scene.set_bool("focusFirst", true);
            let focused = scene.take_keyboard();
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            let claims = |claim: &str| KeyboardReport::Wanted(vec![claim.to_owned()]);
            assert_eq!(
                (none, first, second, focused),
                (
                    KeyboardReport::LetGo,
                    claims("Up"),
                    claims("Down"),
                    claims("Up")
                ),
                "(nobody wanting, the first wanting, the second too, the first focused)"
            );
        });
    }

    /// **A scene that is not hosted may bind `Solium.keyboard` and holds
    /// nothing**: a window frame, or a `QtObject` in one, that writes it
    /// builds, and says nothing to take.
    #[test]
    fn an_unhosted_scene_may_bind_the_keyboard_and_holds_nothing() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let directory = std::env::temp_dir().join("solium-hosted-keyboard-none");
            let _ = std::fs::create_dir_all(&directory);
            let path = directory.join("Scene.qml");
            std::fs::write(
                &path,
                "import QtQuick\nimport Solium\nItem {\n    focus: true\n    Solium.keyboard.wants: true\n    QtObject { Solium.keyboard.wants: true }\n}\n",
            )
            .expect("writing the scene");
            let mut scene = Scene::for_host(&path, 16, 16, None).expect("the scene builds");
            let taken = scene.take_keyboard();
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(taken, KeyboardReport::Unchanged);
        });
    }

    /// **A field in a Qt Quick Controls `Popup` that wants the keyboard
    /// takes the keys** (Ruling 14): `Solium.keyboard` written on the
    /// `Popup`, which is no item, is the item's Qt draws it as, so the open
    /// search popup wants the keyboard with its claims, the field in it
    /// types Russian, and the closed popup lets go. A `QtObject` beside it
    /// that writes `Solium.keyboard` holds nothing.
    #[test]
    fn a_field_in_a_popup_that_wants_the_keyboard_takes_the_keys() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted(
                "solium-hosted-popup-keyboard",
                r#"
                import QtQuick
                import QtQuick.Controls
                import Solium
                Item {
                    id: root
                    property bool open: false
                    readonly property string typed: field.text
                    QtObject { Solium.keyboard.wants: true }
                    Popup {
                        x: 0; y: 0; width: 40; height: 20; padding: 0
                        visible: root.open
                        focus: true
                        enter: null; exit: null
                        closePolicy: Popup.NoAutoClose
                        Solium.keyboard.wants: activeFocus
                        Solium.keyboard.claims: [ "Escape" ]
                        TextField { id: field; anchors.fill: parent; focus: true }
                    }
                }
                "#,
                "popup-keyboard-1",
            );
            let closed = scene.take_keyboard();
            scene.set_bool("open", true);
            let opened = scene.take_keyboard();
            for (code, text) in [(41_u32, "п"), (27, "р")] {
                let qt_key =
                    crate::qml::keys::qt_key(smithay::input::keyboard::Keysym::NoSymbol, text);
                tap(&mut scene, code, qt_key, text);
            }
            let typed = scene.get_string_for_test("typed");
            scene.set_bool("open", false);
            let closed_again = scene.take_keyboard();
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                (closed, opened, typed.as_str(), closed_again),
                (
                    KeyboardReport::LetGo,
                    KeyboardReport::Wanted(vec!["Escape".to_owned()]),
                    "пр",
                    KeyboardReport::LetGo,
                ),
                "(the take with the popup closed, once it is open, what the field typed, \
                 the take once it is closed again)"
            );
        });
    }

    /// **A scene the compositor took the keyboard from takes it again only
    /// when asked anew** (Ruling 14): an item whose `wants` is not bound to
    /// its focus, let go of, wants nothing, so the window just clicked keeps
    /// the keyboard; it wants it again once it comes to want it again, and
    /// once it is shown again.
    #[test]
    fn a_scene_let_go_of_takes_the_keyboard_again_only_when_asked_anew() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted(
                "solium-hosted-let-go",
                r#"
                import QtQuick
                import Solium
                Item {
                    id: root
                    property bool shown: true
                    property bool asking: true
                    Item {
                        focus: true
                        visible: root.shown
                        Solium.keyboard.wants: root.asking
                        Solium.keyboard.claims: [ "Escape" ]
                    }
                }
                "#,
                "let-go-1",
            );
            let first = scene.take_keyboard();
            scene.let_go_keyboard();
            let let_go = scene.take_keyboard();
            scene.set_bool("asking", false);
            scene.set_bool("asking", true);
            let asked_again = scene.take_keyboard();
            scene.let_go_keyboard();
            let let_go_again = scene.take_keyboard();
            scene.set_bool("shown", false);
            scene.set_bool("shown", true);
            let shown_again = scene.take_keyboard();
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            let wanted = KeyboardReport::Wanted(vec!["Escape".to_owned()]);
            assert_eq!(
                (first, let_go, asked_again, let_go_again, shown_again),
                (
                    wanted.clone(),
                    KeyboardReport::LetGo,
                    wanted.clone(),
                    KeyboardReport::LetGo,
                    wanted,
                ),
                "(the first take, after the let-go, after wanting it again, after a second \
                 let-go, after being shown again)"
            );
        });
    }

    /// **A let-go takes the focus from a field inside the container that
    /// wants the keyboard** (Ruling 14): `wants` written on an item that is
    /// no focus scope, around a focused field, which has active focus while
    /// the container does not. Let go of, the field loses its focus, so it
    /// shows no caret for keys that now go to a window, and the scene asks
    /// anew once the field takes active focus again.
    #[test]
    fn a_let_go_takes_the_focus_from_a_field_inside_the_container_that_wants_the_keyboard() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted(
                "solium-hosted-let-go-container",
                r#"
                import QtQuick
                import QtQuick.Controls
                import Solium
                Item {
                    property bool refocus: false
                    readonly property bool typing: field.activeFocus
                    onRefocusChanged: if (refocus) field.forceActiveFocus()
                    Solium.keyboard.wants: true
                    TextField { id: field; width: 40; height: 20; focus: true }
                }
                "#,
                "let-go-container-1",
            );
            let first = (scene.take_keyboard(), scene.get_bool("typing"));
            scene.let_go_keyboard();
            let let_go = (scene.take_keyboard(), scene.get_bool("typing"));
            scene.set_bool("refocus", true);
            let asked_again = (scene.take_keyboard(), scene.get_bool("typing"));
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            let wanted = KeyboardReport::Wanted(Vec::new());
            assert_eq!(
                (first, let_go, asked_again),
                (
                    (wanted.clone(), true),
                    (KeyboardReport::LetGo, false),
                    (wanted, true),
                ),
                "((the first take, the field focused), (after the let-go, the field focused), \
                 (once the field took the focus again, the field focused))"
            );
        });
    }

    /// **A key the scene hears moves its items for the next hit**: Escape
    /// hides the button beside the field, and the point it covered claims
    /// nothing once the key is told.
    #[test]
    fn a_key_the_scene_hears_moves_its_items_for_the_next_hit() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted("solium-hosted-key-moves", FIELD, "key-moves-1");
            let _ = scene.take_keyboard();
            let before = (crate::qml::hosted::generation(), scene.hit(50.0, 10.0));
            tap(&mut scene, 9, 0x0100_0000, "\u{1b}");
            let after = (crate::qml::hosted::generation(), scene.hit(50.0, 10.0));
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                (before.1, after.1, after.0 > before.0),
                (Hit::Press, Hit::Nothing, true),
                "(the button before Escape, the point after it, whether the generation moved)"
            );
        });
    }

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

    /// **A press on a tweak sends its id** with `Solium.send`, which the
    /// compositor hands to `tweaks.lua` with no data (Ruling 15).
    #[test]
    fn a_press_on_a_tweak_sends_its_id() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/qml/tweaks.qml"));
            let mut scene = Scene::for_monitor(
                path,
                320,
                400,
                Some(r#"{"entries":[{"id":"pane:border","label":"Border","group":""}]}"#),
                "tweaks-send-1",
            )
            .expect("the panel builds");
            // Down the panel until the press lands on the entry, wherever the
            // theme's sizes put it.
            let mut sent = None;
            for step in 0..80_u32 {
                click(
                    &mut scene,
                    (60.0, f64::from(step * 5)),
                    u64::from(step) * 1000,
                );
                sent = scene.take_action();
                if sent.is_some() {
                    break;
                }
            }
            drop(scene);
            assert_eq!(
                sent,
                Some(("pane:border".to_owned(), crate::json::Json::Null))
            );
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

    /// **A scene's reserve is reported once per change**, an unset edge as
    /// unset, so the declaration keeps it (Ruling 10).
    #[test]
    fn a_scene_reserve_is_reported_once_per_change() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted(
                "solium-hosted-reserve",
                "import QtQuick\nimport Solium\nItem { property bool hidden: false\n Solium.surface.reserve.bottom: hidden ? 0 : 48 }\n",
                "reserve-1",
            );
            let first = scene.take_reserve();
            let again = scene.take_reserve();
            scene.set_bool("hidden", true);
            let hidden = scene.take_reserve();
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                (first, again, hidden),
                (
                    Some(SceneReserve {
                        bottom: Some(48),
                        ..SceneReserve::default()
                    }),
                    None,
                    Some(SceneReserve {
                        bottom: Some(0),
                        ..SceneReserve::default()
                    }),
                ),
                "(the first take, a second with no change, the take after the bar hid)"
            );
        });
    }

    /// **A negative scene reserve gives the edge back to the declaration**:
    /// an edge the scene set and then set to `-1` is reported unset again
    /// (Ruling 10).
    #[test]
    fn a_negative_scene_reserve_gives_the_edge_back_to_the_declaration() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted(
                "solium-hosted-reserve-negative",
                "import QtQuick\nimport Solium\nItem { property bool given: false\n Solium.surface.reserve.bottom: given ? -1 : 48 }\n",
                "reserve-negative-1",
            );
            let first = scene.take_reserve();
            scene.set_bool("given", true);
            let given = scene.take_reserve();
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                (first, given),
                (
                    Some(SceneReserve {
                        bottom: Some(48),
                        ..SceneReserve::default()
                    }),
                    Some(SceneReserve::default()),
                ),
                "(the first take, the take after the scene gave the edge back)"
            );
        });
    }

    /// **A scene hosted on no monitor may bind a reserve, and reserves
    /// nothing**: a window frame or the loading scene that writes
    /// `Solium.surface.reserve` builds, and says nothing to take.
    #[test]
    fn an_unhosted_scene_may_bind_a_reserve_and_reserves_nothing() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let directory = std::env::temp_dir().join("solium-hosted-reserve-none");
            let _ = std::fs::create_dir_all(&directory);
            let path = directory.join("Scene.qml");
            std::fs::write(
                &path,
                "import QtQuick\nimport Solium\nItem { Solium.surface.reserve.bottom: 48 }\n",
            )
            .expect("writing the scene");
            let mut scene = Scene::for_host(&path, 16, 16, None).expect("the scene builds");
            let taken = scene.take_reserve();
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(taken, None);
        });
    }

    /// **A panel that grows out of the bar leaves the reserve as it was**:
    /// the scene's reserve is the bar's edge, whatever the scene draws, so
    /// quick settings opening over the windows reports no change, and its
    /// items take presses where nothing did before. Illia's requirement for
    /// primitive 3.
    #[test]
    fn a_panel_grown_out_of_the_bar_leaves_the_reserve_as_it_was() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted(
                "solium-hosted-reserve-panel",
                r"
                import QtQuick
                import Solium
                Item {
                    property bool open: false
                    Solium.surface.reserve.bottom: 8
                    MouseArea { x: 0; y: 24; width: 64; height: 8 }
                    MouseArea { x: 0; y: 24 - height; width: 64; height: open ? 20 : 0 }
                }
                ",
                "reserve-panel-1",
            );
            let reserved = scene.take_reserve();
            let closed = scene.hit(10.0, 10.0);
            scene.set_bool("open", true);
            let opened = (scene.take_reserve(), scene.hit(10.0, 10.0));
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                (reserved, closed, opened),
                (
                    Some(SceneReserve {
                        bottom: Some(8),
                        ..SceneReserve::default()
                    }),
                    Hit::Nothing,
                    (None, Hit::Press),
                ),
                "(the bar's reserve, the panel's place while it is closed, \
                 (what opening it reported, the panel's place once it is open))"
            );
        });
    }

    const MENU: &str = r#"
        import QtQuick
        import Solium
        Item {
            id: root
            property bool open: true
            property int dismissed: 0
            Rectangle { id: menu; x: 10; y: 10; width: 20; height: 10; visible: root.open }
            Grab {
                name: "tray-menu"
                target: menu
                active: menu.visible
                onDismissed: { root.dismissed += 1; root.open = false }
            }
        }
    "#;

    /// **A grab is reported with its name while active, and released when it
    /// is not**; a point inside its target is inside, and dismissing it
    /// signals it (Ruling 12).
    #[test]
    fn a_grab_is_held_while_active_and_dismissed_on_request() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted("solium-hosted-grab", MENU, "grab-1");
            let held = scene.take_grab();
            let again = scene.take_grab();
            let inside = scene.grab_contains(15.0, 15.0);
            let outside = scene.grab_contains(50.0, 25.0);
            scene.dismiss();
            let dismissed = scene.get_int("dismissed");
            let released = scene.take_grab();
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                (held, again, inside, outside, dismissed, released),
                (
                    GrabReport::Held("tray-menu".to_owned()),
                    GrabReport::Unchanged,
                    true,
                    false,
                    1,
                    GrabReport::Released,
                ),
                "(the first take, a second with no change, inside the menu, outside it, \
                 the dismissals the scene heard, the take after onDismissed closed the menu)"
            );
        });
    }

    /// **A scene's newest grab is the one reported, and every active one
    /// counts** (Ruling 12): a point inside either target is inside, and a
    /// dismissal reaches both, newest first.
    #[test]
    fn a_scenes_newest_grab_is_reported_and_every_active_one_counts() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted(
                "solium-hosted-two-grabs",
                r#"
                import QtQuick
                import Solium
                Item {
                    id: root
                    property bool late: false
                    property string order: ""
                    Rectangle { id: first; x: 0; y: 0; width: 10; height: 10 }
                    Rectangle { id: second; x: 20; y: 0; width: 10; height: 10 }
                    Grab { name: "first"; target: first; active: true; onDismissed: root.order += "first," }
                    Grab { name: "second"; target: second; active: root.late; onDismissed: root.order += "second," }
                }
                "#,
                "two-grabs-1",
            );
            let alone = scene.take_grab();
            scene.set_bool("late", true);
            let newest = scene.take_grab();
            let inside = (
                scene.grab_contains(5.0, 5.0),
                scene.grab_contains(25.0, 5.0),
            );
            let between = scene.grab_contains(15.0, 5.0);
            scene.dismiss();
            let order = scene.take_string("order");
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                (alone, newest, inside, between, order.as_deref()),
                (
                    GrabReport::Held("first".to_owned()),
                    GrabReport::Held("second".to_owned()),
                    (true, true),
                    false,
                    Some("second,first,"),
                ),
                "(the first alone, the newest once both are active, inside each target, \
                 between them, the order they heard the dismissal in)"
            );
        });
    }

    /// **A scene with no active grab says so at its first take, and once**:
    /// a scene rebuilt for an edit with no popup open lets go of the grab
    /// the scene before it held.
    #[test]
    fn a_scene_with_no_active_grab_says_so_once() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted(
                "solium-hosted-no-grab",
                "import QtQuick\nItem {}\n",
                "no-grab-1",
            );
            let first = scene.take_grab();
            let again = scene.take_grab();
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                (first, again),
                (GrabReport::Released, GrabReport::Unchanged),
                "(the first take, the second)"
            );
        });
    }

    /// **A Qt Quick Controls `Popup` is a grab's target**, as an item is:
    /// a point on the popup is inside it, whether the target is the `Popup`
    /// itself or the item Qt draws it as. A `Popup` is no item, and without
    /// this its grab has no target, so every press on the open menu would
    /// dismiss it. A target that is neither, which QML takes, has no points,
    /// and the log says so.
    #[test]
    fn a_controls_popup_is_a_grabs_target() {
        const POPUP: &str = r"
            import QtQuick
            import QtQuick.Controls
            import Solium
            Item {
                readonly property int targeted: grab.target !== null ? 1 : 0
                QtObject { id: notAnItem }
                Popup {
                    id: menu
                    x: 10; y: 0; width: 20; height: 20; padding: 0
                    visible: true
                    enter: null; exit: null
                    closePolicy: Popup.NoAutoClose
                    contentItem: MouseArea {}
                }
                Grab { id: grab; name: 'menu'; target: TARGET; active: menu.visible; onDismissed: menu.close() }
            }
        ";
        on_the_qt_thread(|| {
            let (first, mut popup) = hosted(
                "solium-hosted-grab-popup",
                &POPUP.replace("TARGET", "menu"),
                "grab-popup-1",
            );
            let as_popup = (popup.get_int("targeted"), popup.grab_contains(20.0, 10.0));
            drop(popup);
            let _ = std::fs::remove_dir_all(&first);
            let (second, mut item) = hosted(
                "solium-hosted-grab-popup-item",
                &POPUP.replace("TARGET", "menu.contentItem.parent"),
                "grab-popup-2",
            );
            let as_item = (item.get_int("targeted"), item.grab_contains(20.0, 10.0));
            drop(item);
            let _ = std::fs::remove_dir_all(&second);
            let (third, mut object) = hosted(
                "solium-hosted-grab-popup-object",
                &POPUP.replace("TARGET", "notAnItem"),
                "grab-popup-3",
            );
            let as_object = (object.get_int("targeted"), object.grab_contains(20.0, 10.0));
            drop(object);
            let _ = std::fs::remove_dir_all(&third);
            assert_eq!(
                (as_popup, as_item, as_object),
                ((1, true), (1, true), (1, false)),
                "((the Popup as the target: set, a point on it inside), \
                 (its item as the target: set, a point on it inside), \
                 (an object neither: set, a point on the popup not inside))"
            );
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

    /// **`Monitors` lists every published monitor, `get` answers by name, and
    /// a changed value is one `dataChanged` for that role only** (Ruling 17).
    #[test]
    fn the_monitors_model_lists_every_row_and_changes_one_role_at_a_time() {
        use crate::models::diff::{diff, render};

        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted(
                "solium-hosted-monitors",
                r#"
                import QtQuick
                import Solium
                Item {
                    readonly property int count: Monitors.count
                    readonly property int widthOfB: Monitors.get("model-b").area.width
                    property int changedRoles: -1
                    Connections {
                        target: Monitors
                        function onDataChanged(topLeft, bottomRight, roles) { changedRoles = roles.length }
                    }
                }
                "#,
                "model-a",
            );
            let before = vec![
                monitor_row("model-a", 1920, 1.0),
                monitor_row("model-b", 1600, 1.0),
            ];
            assert!(super::apply_rows(
                super::Model::Monitors,
                &render(&diff(&[], &before))
            ));
            assert!(scene.get_int("count") >= 2);
            assert_eq!(scene.get_int("widthOfB"), 1600);
            let after = vec![
                monitor_row("model-a", 1920, 1.0),
                monitor_row("model-b", 1500, 1.0),
            ];
            assert!(super::apply_rows(
                super::Model::Monitors,
                &render(&diff(&before, &after))
            ));
            assert_eq!(
                (scene.get_int("widthOfB"), scene.get_int("changedRoles")),
                (1500, 1),
                "one value changed, so one role"
            );
            let _ = super::apply_rows(super::Model::Monitors, &render(&diff(&after, &[])));
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

    /// **Every action a scene sends is queued with its data, in order**, so
    /// two in one frame both arrive (Ruling 15): an object, a bare value
    /// and nothing at all.
    #[test]
    fn solium_send_queues_every_action_with_its_data_in_order() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted(
                "solium-hosted-send",
                r#"
                import QtQuick
                import Solium
                Item {
                    Component.onCompleted: {
                        Solium.send("windows.focus", { id: 7 })
                        Solium.send("workspaces.go", { id: "2", monitor: "DP-1" })
                        Solium.send("volume", 0.5)
                        Solium.send("plain")
                    }
                }
                "#,
                "send-1",
            );
            let taken: Vec<(String, String)> = std::iter::from_fn(|| scene.take_action())
                .map(|(action, data)| (action, data.render()))
                .collect();
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                taken,
                vec![
                    ("windows.focus".to_owned(), r#"{"id":7}"#.to_owned()),
                    (
                        "workspaces.go".to_owned(),
                        r#"{"id":"2","monitor":"DP-1"}"#.to_owned()
                    ),
                    ("volume".to_owned(), "0.5".to_owned()),
                    ("plain".to_owned(), "null".to_owned()),
                ]
            );
        });
    }

    /// **A scene that is not hosted may call `Solium.send`, and queues
    /// nothing**: there is no surface for its action to come from.
    #[test]
    fn an_unhosted_scene_may_send_and_queues_nothing() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let directory = std::env::temp_dir().join("solium-hosted-send-none");
            let _ = std::fs::create_dir_all(&directory);
            let path = directory.join("Scene.qml");
            std::fs::write(
                &path,
                "import QtQuick\nimport Solium\nItem {\n    Component.onCompleted: Solium.send(\"windows.focus\", { id: 7 })\n}\n",
            )
            .expect("writing the scene");
            let mut scene = Scene::for_host(&path, 16, 16, None).expect("the scene builds");
            let taken = scene.take_action();
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(taken, None);
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

    fn window_row(
        id: u64,
        monitor: &str,
        focused: bool,
        focus_order: u32,
    ) -> crate::models::diff::Row {
        use crate::json::Json;
        crate::models::diff::Row {
            key: id.to_string(),
            values: std::collections::BTreeMap::from([
                (
                    "id",
                    Json::Number(f64::from(u32::try_from(id).unwrap_or(0))),
                ),
                ("title", Json::Text(format!("window {id}"))),
                ("monitor", Json::Text(monitor.to_owned())),
                ("focused", Json::Bool(focused)),
                ("focusOrder", Json::Number(f64::from(focus_order))),
                ("onStage", Json::Bool(true)),
                ("state", Json::Text("shown".to_owned())),
            ]),
        }
    }

    /// **`WindowList` filters by monitor and by stage and sorts by recent
    /// focus, again as focus moves, and is never reset; the facade follows
    /// focus and is never null; a gone window's row reads invalid, not
    /// null** (Ruling 17, Ruling 18).
    #[test]
    fn the_windows_model_filters_sorts_and_keeps_its_facades() {
        use crate::json::Json;
        use crate::models::diff::{diff, render};

        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted(
                "solium-hosted-windows",
                r#"
                import QtQuick
                import Solium
                Item {
                    // Asked again as the count changes, so it is the row once
                    // there is one, and whatever `get` answers once it goes.
                    property var held: { Windows.count; return Windows.get(902) }
                    readonly property string focusedTitle: Windows.focused.present ? Windows.focused.title : ""
                    readonly property int heldValid: held.valid ? 1 : 0
                    WindowList { id: here; monitor: "w-left"; sort: "mru" }
                    WindowList { id: offStage; onStage: false }
                    readonly property int here: here.count
                    readonly property int offStageCount: offStage.count
                    property string first: ""
                    property int resets: 0
                    function readFirst() { first = here.count > 0 ? here.data(here.index(0, 0), Qt.UserRole + 1 + 2) : "" }
                    Connections {
                        target: here
                        function onLayoutChanged() { readFirst() }
                        function onRowsInserted() { readFirst() }
                        function onRowsRemoved() { readFirst() }
                        function onRowsMoved() { readFirst() }
                        function onModelReset() { resets += 1 }
                    }
                }
                "#,
                "w-left",
            );
            let mut parked = window_row(904, "w-right", false, 3);
            parked.values.insert("onStage", Json::Bool(false));
            let rows = vec![
                window_row(901, "w-left", false, 1),
                window_row(902, "w-left", true, 0),
                window_row(903, "w-right", false, 2),
                parked.clone(),
            ];
            assert!(super::apply_rows(
                super::Model::Windows,
                &render(&diff(&[], &rows))
            ));
            assert_eq!(
                scene.get_int("here"),
                2,
                "WindowList kept another monitor's window"
            );
            assert_eq!(
                scene.get_int("offStageCount"),
                1,
                "WindowList {{ onStage: false }} is not only the window off stage"
            );
            assert_eq!(scene.get_string_for_test("focusedTitle"), "window 902");
            assert_eq!(
                scene.get_string_for_test("first"),
                "902",
                "sorted by recent focus, the focused window is first"
            );
            assert_eq!(
                scene.get_int("heldValid"),
                1,
                "Windows.get(902) is not the window's live row"
            );

            let refocused = vec![
                window_row(901, "w-left", true, 0),
                window_row(902, "w-left", false, 1),
                window_row(903, "w-right", false, 2),
                parked.clone(),
            ];
            assert!(super::apply_rows(
                super::Model::Windows,
                &render(&diff(&rows, &refocused))
            ));
            assert_eq!(
                scene.get_string_for_test("focusedTitle"),
                "window 901",
                "the facade did not follow focus"
            );
            assert_eq!(
                scene.get_string_for_test("first"),
                "901",
                "the list was not sorted again as focus moved"
            );

            let after = vec![
                window_row(901, "w-left", true, 0),
                window_row(903, "w-right", false, 1),
                parked,
            ];
            assert!(super::apply_rows(
                super::Model::Windows,
                &render(&diff(&refocused, &after))
            ));
            assert_eq!(
                (
                    scene.get_int("heldValid"),
                    scene.get_int("here"),
                    scene.get_int("resets")
                ),
                (0, 1, 0),
                "a gone window's row must read invalid, and still be an object, and the \
                 list is never reset: (heldValid, how many on w-left, resets)"
            );
            let _ = super::apply_rows(super::Model::Windows, &render(&diff(&after, &[])));
            assert_eq!(
                scene.get_string_for_test("focusedTitle"),
                "",
                "with no window focused, the facade is absent"
            );
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    /// **Qt Quick's `Window` is still Qt Quick's beside the windows model**
    /// (Ruling 1a): `Windows`' row type has no name in `Solium`, so a scene
    /// that imports both and writes `Window {}` builds a Qt Quick window.
    #[test]
    fn a_quick_window_is_still_qt_quicks_beside_the_windows_model() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted(
                "solium-hosted-quick-window",
                r#"
                import QtQuick
                import Solium
                Item {
                    Window { id: own; visible: false }
                    readonly property int quick: own.contentItem ? 1 : 0
                }
                "#,
                "quick-window-1",
            );
            assert_eq!(scene.get_int("quick"), 1, "Window is not Qt Quick's window");
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    /// **With nothing focused, `Windows.focused` is empty**, as the absent
    /// row is, and not the window focused last: a bar that shows the focused
    /// title shows none on an empty desk (Ruling 17).
    #[test]
    fn the_focused_facade_is_empty_with_nothing_focused() {
        use crate::models::diff::{diff, render};

        on_the_qt_thread(|| {
            let (directory, mut scene) = hosted(
                "solium-hosted-focused-empty",
                r#"
                import QtQuick
                import Solium
                Item {
                    readonly property string title: Windows.focused.title
                    readonly property int focusedId: Windows.focused.id
                    readonly property int present: Windows.focused.present ? 1 : 0
                }
                "#,
                "focused-empty-1",
            );
            let focused = vec![window_row(911, "focused-empty-1", true, 0)];
            assert!(super::apply_rows(
                super::Model::Windows,
                &render(&diff(&[], &focused))
            ));
            let before = scene.get_string_for_test("title");
            let unfocused = vec![window_row(911, "focused-empty-1", false, 0)];
            assert!(super::apply_rows(
                super::Model::Windows,
                &render(&diff(&focused, &unfocused))
            ));
            let after = (
                scene.get_int("present"),
                scene.get_string_for_test("title"),
                scene.get_int("focusedId"),
            );
            // Taken out before asserting, so a failure leaves no row behind
            // for the next test on the Qt thread.
            let _ = super::apply_rows(super::Model::Windows, &render(&diff(&unfocused, &[])));
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                before, "window 911",
                "the premise: the facade is the focused window"
            );
            assert_eq!(
                after,
                (0, String::new(), 0),
                "(present, title, id) with nothing focused"
            );
        });
    }

    /// **A delegate reads `id`, `state` and `parent` through `model`**: in a
    /// delegate, the item's own `state` and `parent` win over the roles of
    /// those names, and `id` is QML's own word, so `model.` is how a delegate
    /// spells all three.
    #[test]
    fn a_delegate_reads_id_state_and_parent_through_model() {
        use crate::json::Json;
        use crate::models::diff::{diff, render};

        on_the_qt_thread(|| {
            let (directory, scene) = hosted(
                "solium-hosted-window-delegate",
                r#"
                import QtQuick
                import Solium
                Item {
                    Repeater {
                        id: each
                        model: Windows
                        Item {
                            readonly property string read: model.id + "/" + model.state + "/" + model.parent
                            readonly property string own: state + "/" + (parent !== null)
                        }
                    }
                    readonly property string read: each.count > 0 ? each.itemAt(0).read : ""
                    readonly property string own: each.count > 0 ? each.itemAt(0).own : ""
                }
                "#,
                "window-delegate-1",
            );
            let mut child = window_row(921, "window-delegate-1", false, 0);
            child.values.insert("parent", Json::Number(77.0));
            let rows = vec![child];
            assert!(super::apply_rows(
                super::Model::Windows,
                &render(&diff(&[], &rows))
            ));
            let read = (
                scene.get_string_for_test("read"),
                scene.get_string_for_test("own"),
            );
            let _ = super::apply_rows(super::Model::Windows, &render(&diff(&rows, &[])));
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                read,
                ("921/shown/77".to_owned(), "/true".to_owned()),
                "(the roles through `model`, the item's own `state` and `parent`)"
            );
        });
    }
}
