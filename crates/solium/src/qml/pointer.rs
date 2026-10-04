//! The pointer's half of what the host and a scene say to each other:
//! `Solium.cursor`, published from here once a frame
//! (`tests::a_published_pointer_reaches_solium_cursor`); a scene whose root
//! item says how big it is (`tests::a_scene_sized_by_its_root_keeps_its_own_size`);
//! and the hotspot that root sets
//! (`tests::the_hotspot_the_root_sets_is_the_scenes`).

use std::{ffi::CString, path::Path};

use anyhow::Result;

use super::Scene;

#[expect(unsafe_code, reason = "the Qt host is C++; this is its C ABI")]
mod ffi {
    use std::ffi::{c_char, c_int};

    unsafe extern "C" {
        pub(super) fn solium_qml_pointer_publish(json: *const c_char) -> c_int;
        pub(super) fn solium_qml_host_next_sized_by_root(width: c_int, height: c_int);
        pub(super) fn solium_qml_scene_root_size(
            scene: *const super::super::ffi::Scene,
            width: *mut f64,
            height: *mut f64,
        ) -> c_int;
        pub(super) fn solium_qml_scene_cursor_hotspot(
            scene: *const super::super::ffi::Scene,
            x: *mut f64,
            y: *mut f64,
        ) -> c_int;
    }
}

/// Hand Qt the pointer as `Solium.cursor` reads it, one JSON object. False when
/// Qt could not take it, before it has started, so the caller sends it again.
/// `tests::a_published_pointer_reaches_solium_cursor`.
#[expect(unsafe_code, reason = "calling into the Qt host")]
pub(crate) fn publish(json: &str) -> bool {
    // A value a binding reads can move a scene's items.
    super::hosted::touched();
    let Ok(json) = CString::new(json) else {
        return false;
    };
    // SAFETY: `json` outlives the call; the host copies what it keeps.
    unsafe { ffi::solium_qml_pointer_publish(json.as_ptr()) != 0 }
}

impl Scene {
    /// A scene whose root item says how big it is: the compositor never writes
    /// the root's width or height, so a binding on either survives every
    /// resize, and a root that sets neither is `default` logical pixels square.
    /// `tests::a_scene_sized_by_its_root_keeps_its_own_size`.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn sized_by_root(qml_path: &Path, default: i32) -> Result<Self> {
        let default = default.max(1);
        // SAFETY: touches only process-global state, consumed by the next
        // build.
        unsafe { ffi::solium_qml_host_next_sized_by_root(default, default) };
        let built = Self::for_host(qml_path, default, default, None);
        // Cleared whether or not the scene was built, so no later scene is
        // sized by its root by accident, as `for_monitor` clears its monitor.
        // `tests::a_scene_built_after_one_sized_by_its_root_is_sized_by_the_host`.
        // SAFETY: as above; zero is the documented "none".
        unsafe { ffi::solium_qml_host_next_sized_by_root(0, 0) };
        built
    }

    /// The root item's own size, in logical pixels.
    /// `tests::a_scene_sized_by_its_root_keeps_its_own_size`.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn root_size(&self) -> (f64, f64) {
        let (mut width, mut height) = (0.0, 0.0);
        // SAFETY: the scene is live for as long as `self`, and the two
        // outputs are written before the call returns.
        let read =
            unsafe { ffi::solium_qml_scene_root_size(self.scene, &raw mut width, &raw mut height) };
        if read == 0 {
            (0.0, 0.0)
        } else {
            (width, height)
        }
    }

    /// `Solium.cursor.hotspot` as the scene's root item set it, in logical
    /// pixels; the top-left corner when it set none.
    /// `tests::the_hotspot_the_root_sets_is_the_scenes`.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn cursor_hotspot(&self) -> (f64, f64) {
        let (mut x, mut y) = (0.0, 0.0);
        // SAFETY: as `root_size`.
        let read =
            unsafe { ffi::solium_qml_scene_cursor_hotspot(self.scene, &raw mut x, &raw mut y) };
        if read == 0 { (0.0, 0.0) } else { (x, y) }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::{Path, PathBuf};

    use super::super::Scene;
    use crate::qml::qt_test::on_the_qt_thread;

    /// `qml` written to `<temp>/<name>/Scene.qml`, in a fresh directory the
    /// caller removes.
    pub(crate) fn written(name: &str, qml: &str) -> (PathBuf, PathBuf) {
        crate::qml::start().expect("Qt starts");
        let directory = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let path = directory.join("Scene.qml");
        std::fs::write(&path, qml).expect("writing the scene");
        (directory, path)
    }

    /// Another scene beside the one [`written`] wrote, in a file of its own,
    /// written before anything in the directory is built.
    ///
    /// Never the same file written again: Qt keeps what it compiled on disk,
    /// keyed by the file's path and the millisecond it was changed at, so a
    /// file rewritten in the millisecond it was first built can build as it
    /// was the first time, and a test asserting the second version fails once
    /// in a few runs. And never a file added after a build: Qt keeps the
    /// directory's listing too, and refuses a file it has not listed.
    pub(crate) fn beside(directory: &Path, name: &str, qml: &str) -> PathBuf {
        let path = directory.join(name);
        std::fs::write(&path, qml).expect("writing the scene");
        path
    }

    fn built(path: &Path) -> Scene {
        Scene::for_host(path, 32, 32, None).expect("the scene builds")
    }

    /// **What the compositor publishes reaches `Solium.cursor` in every object
    /// of a scene**: the shape by its CSS name, whether a button is held, the
    /// velocity along each axis, the monitor's scale and the configured size,
    /// each notified once when it changes and not when it does not.
    #[test]
    fn a_published_pointer_reaches_solium_cursor() {
        on_the_qt_thread(|| {
            let (directory, path) = written(
                "solium-pointer-published",
                r#"
                import QtQuick
                import Solium
                Item {
                    readonly property string shape: Solium.cursor.shape
                    readonly property int pressed: Solium.cursor.pressed ? 1 : 0
                    readonly property int vx: Math.round(Solium.cursor.velocity.x)
                    readonly property int vy: Math.round(Solium.cursor.velocity.y)
                    readonly property int scale100: Math.round(Solium.cursor.scale * 100)
                    readonly property int size: Solium.cursor.size
                    readonly property string inner: child.shape
                    property int shapes: 0
                    Item {
                        id: child
                        readonly property string shape: Solium.cursor.shape
                        onShapeChanged: parent.shapes++
                    }
                }
                "#,
            );
            let mut scene = built(&path);
            assert!(super::publish(
                r#"{"shape":"wait","pressed":false,"velocity":{"x":0,"y":0},"scale":1,"size":24}"#
            ));
            let before = scene.get_int("shapes");
            assert!(super::publish(
                r#"{"shape":"text","pressed":true,"velocity":{"x":120,"y":-40},"scale":2,"size":32}"#
            ));
            let read = |scene: &mut Scene| {
                (
                    scene.get_string_for_test("shape"),
                    scene.get_string_for_test("inner"),
                    ["pressed", "vx", "vy", "scale100", "size"].map(|name| scene.get_int(name)),
                )
            };
            assert_eq!(
                read(&mut scene),
                ("text".to_owned(), "text".to_owned(), [1, 120, -40, 200, 32]),
                "(the root's shape, a child's, [pressed, velocity x, velocity y, scale x 100, \
                 size])"
            );
            assert!(super::publish(
                r#"{"shape":"text","pressed":true,"velocity":{"x":10,"y":0},"scale":2,"size":32}"#
            ));
            let shapes = scene.get_int("shapes") - before;
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                shapes, 1,
                "the shape changed once and was notified {shapes} times"
            );
        });
    }

    /// **A scene sized by its root keeps its own size**: the compositor never
    /// writes the root's width or height, so a binding on them survives a
    /// resize, and a root that sets neither is the default square.
    #[test]
    fn a_scene_sized_by_its_root_keeps_its_own_size() {
        on_the_qt_thread(|| {
            let (directory, path) = written(
                "solium-pointer-own-size",
                "import QtQuick\nItem { property int base: 24; width: base + 16; height: base + 8 }\n",
            );
            let bare = beside(&directory, "Bare.qml", "import QtQuick\nItem {}\n");
            let mut scene = Scene::sized_by_root(&path, 24).expect("the scene builds");
            let built = scene.root_size();
            scene.resize(80, 64, 2.0);
            let resized = scene.root_size();
            scene.set_int("base", 32);
            let rebound = scene.root_size();
            drop(scene);
            let mut bare = Scene::sized_by_root(&bare, 30).expect("the scene builds");
            let bare_built = bare.root_size();
            bare.resize(10, 10, 1.0);
            let bare_resized = bare.root_size();
            drop(bare);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                [built, resized, rebound, bare_built, bare_resized],
                [
                    (40.0, 32.0),
                    (40.0, 32.0),
                    (48.0, 40.0),
                    (30.0, 30.0),
                    (30.0, 30.0)
                ],
                "[built, resized, its binding moved, a root with no size, resized]"
            );
        });
    }

    /// **A scene built after one sized by its root is sized by the host**, as
    /// every other scene is: the root's own size is asked for one build only.
    #[test]
    fn a_scene_built_after_one_sized_by_its_root_is_sized_by_the_host() {
        on_the_qt_thread(|| {
            let (directory, path) = written(
                "solium-pointer-own-size-once",
                "import QtQuick\nItem { width: 40; height: 40 }\n",
            );
            let first = Scene::sized_by_root(&path, 24).expect("the scene builds");
            let mut second = built(&path);
            second.resize(16, 16, 1.0);
            let size = second.root_size();
            drop((first, second));
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(size, (16.0, 16.0), "the next scene kept its own size");
        });
    }

    /// **The hotspot is the one the scene's root item sets**: not a child's,
    /// the top-left corner when the root sets none, and the corner for a point
    /// that is not a number.
    #[test]
    fn the_hotspot_the_root_sets_is_the_scenes() {
        on_the_qt_thread(|| {
            let (directory, path) = written(
                "solium-pointer-hotspot",
                r"
                import QtQuick
                import Solium
                Item {
                    width: 32; height: 32
                    Solium.cursor.hotspot: Qt.point(6, 4)
                    Item { Solium.cursor.hotspot: Qt.point(20, 20) }
                }
                ",
            );
            let none = beside(
                &directory,
                "None.qml",
                "import QtQuick\nItem { width: 32; height: 32 }\n",
            );
            let nan = beside(
                &directory,
                "NotANumber.qml",
                "import QtQuick\nimport Solium\nItem { Solium.cursor.hotspot: Qt.point(NaN, 3) }\n",
            );
            let set = built(&path).cursor_hotspot();
            let none = built(&none).cursor_hotspot();
            let nan = built(&nan).cursor_hotspot();
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                [set, none, nan],
                [(6.0, 4.0), (0.0, 0.0), (0.0, 0.0)],
                "[the root's, a root that set none, a point that is not a number]"
            );
        });
    }

    /// **`Solium.region` and `Solium.material` are accepted, and materials are
    /// off** (#199): the issue's own liquid-glass pointer builds, reads
    /// `Solium.materialState` as `"off"`, and so shows its plain drawing.
    #[test]
    fn a_region_and_a_material_are_accepted_and_materials_are_off() {
        on_the_qt_thread(|| {
            let (directory, path) = written(
                "solium-pointer-material",
                r#"
                import QtQuick
                import Solium
                Item {
                    width: 40; height: 40
                    Solium.cursor.hotspot: Qt.point(6, 6)
                    readonly property string region: glass.Solium.region
                    readonly property string state: glass.Solium.materialState
                    readonly property int alpha: Math.round(glass.color.a * 100)
                    Rectangle {
                        id: glass
                        anchors.fill: parent; radius: width / 2
                        Solium.region: "pointer"
                        Solium.material: ({ effect: "glass-rect", source: "live", rim: 30 })
                        color: Qt.rgba(1, 1, 1, Solium.materialState === "full" ? 0.08 : 0.8)
                    }
                }
                "#,
            );
            let mut scene = built(&path);
            let read = (
                scene.get_string_for_test("region"),
                scene.get_string_for_test("state"),
                scene.get_int("alpha"),
            );
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                read,
                ("pointer".to_owned(), "off".to_owned(), 80),
                "(the region, the material's state, the glass's alpha x 100)"
            );
        });
    }
}
