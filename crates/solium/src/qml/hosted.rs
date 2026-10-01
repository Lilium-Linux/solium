//! The compositor's half of what a hosted scene and the compositor say to each
//! other: properties written in place, the monitor it is on, and, from later
//! tasks, pointer events, what it claims, what it reserves, its grabs, its
//! keyboard wants and its actions.

use std::{
    ffi::{CString, c_char, c_int},
    path::Path,
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
    }
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
        let (Ok(path), Ok(value)) = (CString::new(path), CString::new(value.render())) else {
            return false;
        };
        // SAFETY: the scene is live for as long as `self`, and both strings
        // outlive the call.
        unsafe { ffi::solium_qml_scene_set_json(self.scene, path.as_ptr(), value.as_ptr()) != 0 }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;

    use crate::qml::{Scene, qt_test::on_the_qt_thread};

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
