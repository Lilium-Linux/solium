//! What Solium's own lock draws on each monitor: one QML scene per monitor, in
//! the one engine, on the lock band -- the frame's whole content while locked,
//! under only the pointer (`render::elements`).
//!
//! The rule of `lock.rs` holds here too: **a lock that fails leaves the session
//! locked and blank, never exposed.** So every way a scene can fail ends on
//! something that is not the session:
//!
//! * the scene file is missing, or will not load: the **fallback**, a scene
//!   compiled into the compositor that shows the time and "Locked" and
//!   nothing of the session (`qml/lock/Fallback.qml`)
//!   (`tests::a_scene_that_will_not_load_shows_the_fallback`);
//! * the scene fails while it runs -- it stops rendering -- the fallback
//!   takes its place, and the log says why
//!   (`tests::a_scene_that_fails_at_run_time_is_swapped_for_the_fallback`);
//! * the fallback fails too, or no scene has drawn yet on a monitor: the
//!   lock's backdrop, which `render.rs` draws under the scene on every frame
//!   whatever happens here.
//!
//! A scene is built for every monitor when the native lock is taken, and for a
//! monitor plugged in while it is held (`Solium::sync_lock_scenes`), and they
//! go when it ends, so nothing of the lock runs while the session does. Which
//! scene is the configuration's to say (`lock.scene`); until it says one, it
//! is the fallback.
//!
//! The scene is told the fingers on its monitor as Qt touch events, so its
//! `DragHandler`s and `TapHandler`s work (`input::touch`), and the pointer's
//! motion and presses as Qt mouse events, so its buttons can be clicked
//! (`Solium::lock_scene_motion`, `Solium::lock_scene_button`). Nothing else
//! hears either. A press or a finger on a `SecretKeypad` key is the
//! exception: the compositor finds the key itself, types it into the
//! password, and the scene hears nothing of it, not even the pointer passing
//! over the pad (`lock_auth.rs`). Keys never reach the scene at all. It is
//! given nothing that could unlock: it shows things, and only Rust ends the
//! lock (`Solium::unlock_natively`).

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    time::Duration,
};

use smithay::{
    backend::renderer::gles::GlesRenderer,
    output::Output,
    utils::{Logical, Point, Rectangle},
};

use crate::{
    qml::hosted::{ScenePointer, SceneTouch},
    render::Drawn,
    surface::{Raster, ShellSurface},
};

/// The fallback's QML, in the binary, so it loads when nothing on disk does.
pub(crate) const FALLBACK: &str = include_str!("../qml/lock/Fallback.qml");

/// What names the fallback: the file it is compiled from, where Solium's QML
/// ships. Nothing is read from it; it is what the log and the pacing report
/// call the scene.
fn fallback_path() -> PathBuf {
    crate::assets::root().join("qml/lock/Fallback.qml")
}

/// What the native lock shows on one monitor, over its backdrop.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shown {
    /// The configuration's lock scene.
    Scene,
    /// The compiled-in fallback.
    Fallback,
    /// No scene: the backdrop alone.
    Backdrop,
}

/// One monitor's scene.
#[derive(Debug)]
struct Instance {
    surface: ShellSurface,
    /// Whether it is the fallback.
    fallback: bool,
}

/// The native lock's scenes, one per monitor, by connector name.
#[derive(Debug, Default)]
pub(crate) struct LockScenes {
    /// The scene file the native lock shows, or `None` for the fallback.
    scene: Option<PathBuf>,
    instances: HashMap<String, Instance>,
    /// The monitors neither the scene nor the fallback would load on, which
    /// show the backdrop and are not tried again until the next lock.
    failed: HashSet<String>,
    /// Where the pointer is, as the scenes have been told it.
    pub(crate) pointer: LockPointer,
}

/// The pointer at the native lock, as its scenes have been told it.
#[derive(Debug, Default)]
pub(crate) struct LockPointer {
    /// The monitor whose scene last heard the pointer.
    pub(crate) on: Option<String>,
    /// The scene a press went to, drawn across its rectangle: it hears the
    /// pointer wherever it goes until every button is up, as Qt's own
    /// windows do.
    pub(crate) held: Option<(String, Rectangle<i32, Logical>)>,
    /// Whether a press is on a keypad key: its release, and every other
    /// button until then, goes nowhere.
    pub(crate) on_pad: bool,
    /// Whether the scene under the pointer was told it left, because the
    /// pointer is over its keypad.
    pub(crate) hidden: bool,
}

impl LockScenes {
    /// Build a scene for every one of `outputs` that has none, and drop the
    /// scenes of monitors that have gone, while `locked` natively; drop every
    /// scene otherwise.
    ///
    /// Built only on the thread Qt runs on, which in a test is not every
    /// thread (`scripted::builds_here`).
    pub(crate) fn sync(&mut self, outputs: &[Output], locked: bool) {
        if !locked {
            self.instances.clear();
            self.failed.clear();
            self.pointer = LockPointer::default();
            return;
        }
        let names: Vec<String> = outputs.iter().map(Output::name).collect();
        self.instances.retain(|monitor, _| names.contains(monitor));
        self.failed.retain(|monitor| names.contains(monitor));
        if !crate::scripted::builds_here() {
            return;
        }
        for monitor in names {
            if self.instances.contains_key(&monitor) || self.failed.contains(&monitor) {
                continue;
            }
            match self.build(&monitor) {
                Some(instance) => {
                    self.instances.insert(monitor, instance);
                }
                None => {
                    self.failed.insert(monitor);
                }
            }
        }
    }

    /// The scene for `monitor`, or the fallback when there is no scene or it
    /// will not load.
    fn build(&self, monitor: &str) -> Option<Instance> {
        if let Some(scene) = self.scene.as_ref() {
            if !scene.is_file() {
                tracing::error!(
                    scene = %scene.display(),
                    monitor,
                    "the lock scene is not there; the fallback stands in"
                );
            } else {
                match ShellSurface::hosted(scene.clone(), "{}", monitor) {
                    Ok(surface) => {
                        return Some(Instance {
                            surface,
                            fallback: false,
                        });
                    }
                    Err(err) => tracing::error!(
                        ?err,
                        scene = %scene.display(),
                        monitor,
                        "the lock scene would not load; the fallback stands in"
                    ),
                }
            }
        }
        fallback(monitor)
    }

    /// What the lock shows on `output`.
    #[cfg(test)]
    pub(crate) fn shown_on(&self, output: &Output) -> Shown {
        match self.instances.get(&output.name()) {
            Some(Instance { fallback: true, .. }) => Shown::Fallback,
            Some(Instance {
                fallback: false, ..
            }) => Shown::Scene,
            None => Shown::Backdrop,
        }
    }

    /// Draw `output`'s scene across `area`, in the monitor's own coordinates,
    /// and swap it for the fallback if it failed to render: the next frame
    /// is the fallback's.
    pub(crate) fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        output: &Output,
        area: Rectangle<i32, Logical>,
        now: Duration,
        raster: Raster,
    ) -> Drawn {
        let monitor = output.name();
        let Some(instance) = self.instances.get_mut(&monitor) else {
            return Drawn {
                element: None,
                animating: false,
            };
        };
        let mut drawn = instance.surface.element(renderer, area, now, 1.0, raster);
        if self.after_draw(&monitor) {
            // Asked for at once, so the fallback is on the next frame rather
            // than whenever something else damages the screen.
            drawn.animating = true;
        }
        drawn
    }

    /// After `monitor`'s scene has drawn: a scene that failed to render is
    /// replaced by the fallback, and a fallback that failed by the backdrop.
    /// Whether the scene was replaced.
    /// `tests::a_scene_that_fails_at_run_time_is_swapped_for_the_fallback`.
    fn after_draw(&mut self, monitor: &str) -> bool {
        let Some(instance) = self.instances.get(monitor) else {
            return false;
        };
        if !instance.surface.failing() {
            return false;
        }
        if instance.fallback {
            tracing::error!(
                monitor,
                "the lock's fallback scene failed while it ran; the backdrop stands in"
            );
            self.instances.remove(monitor);
            self.failed.insert(monitor.to_owned());
            return true;
        }
        tracing::error!(
            monitor,
            "the lock scene failed while it ran; the fallback stands in"
        );
        match fallback(monitor) {
            Some(instance) => {
                self.instances.insert(monitor.to_owned(), instance);
            }
            None => {
                self.instances.remove(monitor);
                self.failed.insert(monitor.to_owned());
            }
        }
        true
    }

    /// Deliver a finger to `monitor`'s scene, drawn across `area` in the
    /// global space. Nothing, on a monitor showing the backdrop.
    pub(crate) fn touch(
        &mut self,
        monitor: &str,
        area: Rectangle<i32, Logical>,
        location: Point<f64, Logical>,
        event: &SceneTouch,
    ) {
        if let Some(instance) = self.instances.get_mut(monitor) {
            instance.surface.touch(area, location, event);
        }
    }

    /// Deliver a pointer event to `monitor`'s scene, drawn across `area`.
    /// Nothing, on a monitor showing the backdrop.
    pub(crate) fn pointer_event(
        &mut self,
        monitor: &str,
        area: Rectangle<i32, Logical>,
        location: Point<f64, Logical>,
        event: &ScenePointer,
    ) {
        if let Some(instance) = self.instances.get_mut(monitor) {
            instance.surface.pointer(area, location, event);
        }
    }

    /// Tell `monitor`'s scene the pointer left it.
    pub(crate) fn leave(&mut self, monitor: &str) {
        if let Some(instance) = self.instances.get_mut(monitor) {
            instance.surface.leave();
        }
    }

    /// The `SecretKeypad` key at `location` on `monitor`'s scene, drawn
    /// across `area`.
    pub(crate) fn keypad_at(
        &self,
        monitor: &str,
        area: Rectangle<i32, Logical>,
        location: Point<f64, Logical>,
    ) -> Option<usize> {
        self.instances
            .get(monitor)
            .and_then(|instance| instance.surface.keypad_at(area, location))
    }

    /// Show `scene` from the next lock on, or the fallback with `None`.
    #[cfg(test)]
    pub(crate) fn use_scene(&mut self, scene: Option<PathBuf>) {
        self.scene = scene;
    }

    /// `output`'s scene, for a test to read.
    #[cfg(test)]
    pub(crate) fn instance_for_test(&mut self, output: &Output) -> Option<&mut ShellSurface> {
        self.instances
            .get_mut(&output.name())
            .map(|instance| &mut instance.surface)
    }

    /// How many monitors have a scene.
    #[cfg(test)]
    pub(crate) fn count(&self) -> usize {
        self.instances.len()
    }
}

/// The compiled-in fallback for `monitor`, or `None` and a line in the log
/// when even that will not load.
fn fallback(monitor: &str) -> Option<Instance> {
    match ShellSurface::compiled(fallback_path(), FALLBACK, "{}", monitor) {
        Ok(surface) => Some(Instance {
            surface,
            fallback: true,
        }),
        Err(err) => {
            tracing::error!(
                ?err,
                monitor,
                "the lock's fallback scene would not load; the backdrop stands in"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::output::{Mode, PhysicalProperties, Scale, Subpixel};

    fn monitor(name: &str) -> Output {
        let output = Output::new(
            name.to_owned(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "solium".to_owned(),
                model: "lock-scene".to_owned(),
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
        output
    }

    /// A scene file of `source`, in a directory of the test's own.
    fn scene(name: &str, source: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("solium-lock-scene-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("a directory for the scene");
        let path = directory.join("Lock.qml");
        std::fs::write(&path, source).expect("writing the scene");
        path
    }

    /// **The fallback is in the binary**: it loads from the text compiled
    /// in, whatever is on disk, here under a path where there is no file at
    /// all, and it is the fallback the lock shows with no scene chosen.
    #[test]
    fn the_fallback_is_compiled_in_and_loads_with_no_file_on_disk() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let nowhere = PathBuf::from("/nonexistent/solium/qml/lock/Fallback.qml");
            assert!(!nowhere.exists());
            let built = ShellSurface::compiled(nowhere, FALLBACK, "{}", "fallback-test");
            assert!(
                built.is_ok(),
                "the compiled-in fallback did not load: {:?}",
                built.err()
            );

            let output = monitor("fallback-default");
            let mut scenes = LockScenes::default();
            scenes.sync(std::slice::from_ref(&output), true);
            assert_eq!(scenes.shown_on(&output), Shown::Fallback);
        });
    }

    /// **A scene that will not load shows the fallback on its monitor**, and
    /// so does one that is not there. A scene that loads is shown.
    #[test]
    fn a_scene_that_will_not_load_shows_the_fallback() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let output = monitor("lock-broken");
            let mut scenes = LockScenes::default();

            scenes.use_scene(Some(scene(
                "broken",
                "import QtQuick\nItem { this is not QML }\n",
            )));
            scenes.sync(std::slice::from_ref(&output), true);
            assert_eq!(scenes.shown_on(&output), Shown::Fallback, "a broken scene");

            scenes.sync(&[], false);
            scenes.use_scene(Some(PathBuf::from("/nonexistent/Lock.qml")));
            scenes.sync(std::slice::from_ref(&output), true);
            assert_eq!(scenes.shown_on(&output), Shown::Fallback, "a missing scene");

            scenes.sync(&[], false);
            scenes.use_scene(Some(scene("working", "import QtQuick\nItem {}\n")));
            scenes.sync(std::slice::from_ref(&output), true);
            assert_eq!(scenes.shown_on(&output), Shown::Scene, "a scene that loads");
        });
    }

    /// **A scene that fails at run time is swapped for the fallback**, and a
    /// fallback that fails for the backdrop. Played through the check made
    /// after every draw, with the failure a render would have reported: a
    /// render needs a GPU this test cannot have.
    #[test]
    fn a_scene_that_fails_at_run_time_is_swapped_for_the_fallback() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let output = monitor("lock-run-time");
            let mut scenes = LockScenes::default();
            scenes.use_scene(Some(scene("run-time", "import QtQuick\nItem {}\n")));
            scenes.sync(std::slice::from_ref(&output), true);
            assert_eq!(scenes.shown_on(&output), Shown::Scene, "the premise");
            assert!(
                !scenes.after_draw(&output.name()),
                "a scene that drew stays"
            );

            scenes
                .instance_for_test(&output)
                .expect("a scene")
                .fail_for_test();
            assert!(scenes.after_draw(&output.name()));
            assert_eq!(scenes.shown_on(&output), Shown::Fallback);

            scenes
                .instance_for_test(&output)
                .expect("the fallback")
                .fail_for_test();
            assert!(scenes.after_draw(&output.name()));
            assert_eq!(scenes.shown_on(&output), Shown::Backdrop);
            scenes.sync(std::slice::from_ref(&output), true);
            assert_eq!(
                scenes.shown_on(&output),
                Shown::Backdrop,
                "and it is not built again until the next lock"
            );
        });
    }

    /// **The scenes go with their monitors and with the lock**: one plugged
    /// in gets one, one unplugged takes its own, and unlocking takes them all.
    #[test]
    fn the_scenes_follow_the_monitors_and_go_with_the_lock() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let (left, right) = (monitor("lock-left"), monitor("lock-right"));
            let mut scenes = LockScenes::default();
            scenes.sync(std::slice::from_ref(&left), true);
            assert_eq!(scenes.count(), 1);
            scenes.sync(&[left.clone(), right.clone()], true);
            assert_eq!(scenes.shown_on(&right), Shown::Fallback, "plugged in");
            scenes.sync(std::slice::from_ref(&right), true);
            assert_eq!(scenes.shown_on(&left), Shown::Backdrop, "unplugged");
            scenes.sync(std::slice::from_ref(&right), false);
            assert_eq!(scenes.count(), 0, "unlocked");
        });
    }
}
