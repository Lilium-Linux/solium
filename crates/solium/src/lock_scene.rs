//! What Solium's own lock draws on each monitor: one QML scene per monitor, in
//! the one engine, on the lock band -- the frame's whole content while locked,
//! under only the pointer (`render::elements`).
//!
//! The rule of `lock.rs` holds here too: **a lock that fails leaves the session
//! locked and blank, never exposed.** So every way a scene can fail ends on
//! something that is not the session:
//!
//! * the scene the configuration names (`lock.scene`) is missing, or will not
//!   load: the **shipped scene**, `qml/lock/Lock.qml`; and if that will not
//!   either, the **fallback**, a scene compiled into the compositor that shows
//!   the time, "Locked" and the password's dots, and nothing of the session
//!   (`qml/lock/Fallback.qml`). The log says which, and why
//!   (`tests::a_broken_scene_falls_back_to_the_shipped_one_and_then_to_the_fallback`);
//! * the scene fails while it runs -- it stops rendering -- the next one down
//!   that chain takes its place, and the log says why
//!   (`tests::a_scene_that_fails_at_run_time_is_swapped_down_the_chain`);
//! * the fallback fails too, or no scene has drawn yet on a monitor: the
//!   lock's backdrop, which `render.rs` draws under the scene on every frame
//!   whatever happens here.
//!
//! **The scenes are built ahead.** With `lock.native` on, each monitor's scene
//! is built when the session starts and when the monitor is plugged in, and
//! kept between locks, so the first locked frame is already the scene's. While
//! the session is unlocked a kept scene is *quiet*: nothing draws it, its Timers
//! and animations are stopped, and a change to it asks for no frame, so it
//! never wakes the compositor (`qml::Scene::set_quiet`,
//! `tests::a_kept_scene_neither_ticks_nor_wakes_while_unlocked`). A scene built
//! as a stand-in, because the one the configuration names failed, is built
//! again from that one when the lock ends, so a scene that has been mended is
//! the next lock's.
//!
//! **The exit.** When Solium's own lock ends, the session is unlocked at once:
//! input goes to the desktop and the desktop is drawn. The lock's scenes are
//! drawn over it for `lock.exit_ms` more, reading `Lock.state` as
//! `"unlocking"`, so a scene can slide or fade itself away; at that bound they
//! are gone, whatever the scene is doing
//! (`lock_auth::tests::the_exit_plays_over_the_desktop_for_exit_ms_and_input_is_the_desktops_at_once`).
//!
//! The scene is told the fingers on its monitor as Qt touch events, so its
//! `DragHandler`s and `TapHandler`s work (`input::touch`), and the pointer's
//! motion and presses as Qt mouse events, so its buttons can be clicked
//! (`Solium::lock_scene_motion`, `Solium::lock_scene_button`). Nothing else
//! hears either, and neither reaches it during the exit, when input is the
//! desktop's. A press or a finger on a `SecretKeypad` key is the exception:
//! the compositor finds the key itself, types it into the password, and the
//! scene hears nothing of it, not even the pointer passing over the pad
//! (`lock_auth.rs`). Keys never reach the scene at all. It is given nothing
//! that could unlock: it shows things, and only Rust ends the lock
//! (`Solium::unlock_natively`).

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
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
    state::Solium,
    surface::{Raster, ShellSurface},
};

/// The fallback's QML, in the binary, so it loads when nothing on disk does.
pub(crate) const FALLBACK: &str = include_str!("../qml/lock/Fallback.qml");

/// The name of the scene Solium ships, which is `lock.scene`'s default.
pub(crate) const SHIPPED: &str = "Lock";

/// `lock.exit_ms`'s default: §4.16's bounded exit, the content lifting and
/// fading on `exit` (160 ms) inside it.
pub(crate) const EXIT_MS: u32 = 250;

/// The longest `lock.exit_ms` may be. Input is the desktop's from the unlock
/// on, so the exit is time spent typing into a desktop the lock still covers:
/// long enough for an iPad's slide on a spring, and no longer than a glance.
pub(crate) const EXIT_MAX_MS: u32 = 600;

/// What names the fallback: the file it is compiled from, where Solium's QML
/// ships. Nothing is read from it; it is what the log and the pacing report
/// call the scene.
fn fallback_path() -> PathBuf {
    crate::assets::root().join("qml/lock/Fallback.qml")
}

/// `lock.*` for what Solium's own lock shows. Handed over by `sol.lock`,
/// beside the password settings `lock_auth.rs` reads and the locker settings
/// `logind.rs` reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Settings {
    /// `lock.native`: whether Solium's own lock is the session's, which it is
    /// unless `lock.command` names a locker. Its scenes are kept ready only
    /// while it is.
    pub(crate) native: bool,
    /// `lock.scene`: a path, or a name looked for in the user's
    /// `qml/lock/` and then in the shipped one ([`find_scene`]).
    pub(crate) scene: String,
    /// `lock.prompt_on`: which monitor shows the password field.
    pub(crate) prompt_on: PromptOn,
    /// `lock.exit_ms`: how long the scenes are drawn over the desktop after
    /// the unlock, at most [`EXIT_MAX_MS`].
    pub(crate) exit: Duration,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            native: true,
            scene: SHIPPED.to_owned(),
            prompt_on: PromptOn::Pointer,
            exit: Duration::from_millis(u64::from(EXIT_MS)),
        }
    }
}

/// `lock.prompt_on`: the monitor `Lock.promptOn` names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PromptOn {
    /// `"primary"`: the primary monitor.
    Primary,
    /// `"pointer"`: the monitor the pointer is on, following it.
    Pointer,
    /// A monitor by its connector name, or the primary one while it is not
    /// plugged in.
    Monitor(String),
}

impl PromptOn {
    pub(crate) fn named(name: &str) -> Option<Self> {
        match name {
            "" => None,
            "primary" => Some(Self::Primary),
            "pointer" => Some(Self::Pointer),
            monitor => Some(Self::Monitor(monitor.to_owned())),
        }
    }
}

/// Where the lock scene `name` is: a path, absolute or `~/`, as given; or a
/// name, with `.qml` added when it has none, looked for in `user` (the
/// user's `qml/lock/`) and then in `shipped` (Solium's), the way a pane style
/// or a `sol.surface` scene is. `None` for a name in neither.
/// `tests::a_bare_name_is_looked_for_in_the_users_directory_first`.
pub(crate) fn find_scene(name: &str, user: Option<&Path>, shipped: &Path) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    if let Some(rest) = name.strip_prefix("~/") {
        return std::env::var_os("HOME").map(|home| PathBuf::from(home).join(rest));
    }
    if name.starts_with('/') {
        return Some(PathBuf::from(name));
    }
    let file = if name.ends_with(".qml") {
        name.to_owned()
    } else {
        format!("{name}.qml")
    };
    user.into_iter()
        .chain(std::iter::once(shipped))
        .map(|directory| directory.join(&file))
        .find(|candidate| candidate.is_file())
}

/// Which scene an instance is, in the order they stand in for each other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    /// The configuration's own scene, when it is not the shipped one.
    Own,
    /// The shipped scene, `qml/lock/Lock.qml`.
    Shipped,
    /// The compiled-in fallback.
    Fallback,
}

/// What the native lock shows on one monitor, over its backdrop.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shown {
    /// The configuration's own scene.
    Own,
    /// The shipped scene.
    Shipped,
    /// The compiled-in fallback.
    Fallback,
    /// No scene: the backdrop alone.
    Backdrop,
}

/// What `lock.scene` comes to on this machine.
enum Resolved {
    /// A scene of the user's own, at this path.
    Own(PathBuf),
    /// The shipped one.
    Shipped,
    /// Nothing is called that.
    Missing,
}

/// One monitor's scene.
#[derive(Debug)]
struct Instance {
    surface: ShellSurface,
    kind: Kind,
}

/// The native lock's scenes, one per monitor, by connector name.
#[derive(Debug)]
pub(crate) struct LockScenes {
    settings: Settings,
    /// The shipped scene's file.
    shipped: PathBuf,
    /// The user's `qml/lock/`, for a test; a test never reads the
    /// developer's own configuration.
    #[cfg(test)]
    user: Option<PathBuf>,
    instances: HashMap<String, Instance>,
    /// The monitors neither a scene nor the fallback would load on, which
    /// show the backdrop and are not tried again until the lock ends.
    failed: HashSet<String>,
    /// Whether the scenes are live: drawn and ticking, while Solium holds the
    /// lock and through its exit. Quiet otherwise.
    live: bool,
    /// Whether every scene is to be built again once they are quiet: the
    /// configuration named another scene.
    rebuild: bool,
    /// When the exit under way ends, on the compositor's clock.
    exit_until: Option<Duration>,
    /// Where the pointer is, as the scenes have been told it.
    pub(crate) pointer: LockPointer,
}

impl Default for LockScenes {
    fn default() -> Self {
        Self {
            settings: Settings::default(),
            shipped: crate::assets::qml()
                .join("lock")
                .join(format!("{SHIPPED}.qml")),
            #[cfg(test)]
            user: None,
            instances: HashMap::new(),
            failed: HashSet::new(),
            live: false,
            rebuild: false,
            exit_until: None,
            pointer: LockPointer::default(),
        }
    }
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
    /// Take `settings` from now on. Another scene is built once the scenes
    /// are quiet, so a lock under way keeps what it shows.
    pub(crate) fn configure(&mut self, settings: Settings) {
        if settings == self.settings {
            return;
        }
        if settings.scene != self.settings.scene {
            self.rebuild = true;
        }
        tracing::debug!(?settings, "the lock's scene settings set");
        self.settings = settings;
    }

    pub(crate) fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Keep a scene on every one of `outputs`, and none on a monitor that has
    /// gone: live, drawn and ticking, while `live`; quiet, kept for the next
    /// lock, while not, if `lock.native` is on; none at all otherwise.
    ///
    /// Going quiet, a scene that stands in for the one the configuration
    /// names is dropped and built again from that one, and so is every scene
    /// after the configuration named another.
    ///
    /// Built only on the thread Qt runs on, which in a test is not every
    /// thread (`scripted::builds_here`).
    pub(crate) fn sync(&mut self, outputs: &[Output], live: bool) {
        if !live && !self.settings.native {
            self.instances.clear();
            self.failed.clear();
            self.pointer = LockPointer::default();
            self.live = false;
            return;
        }
        if !live && (self.live || self.rebuild) {
            if self.rebuild {
                self.instances.clear();
            } else {
                let first = self.first_choice();
                self.instances.retain(|_, instance| instance.kind == first);
            }
            self.failed.clear();
            self.rebuild = false;
            self.pointer = LockPointer::default();
        }
        let names: Vec<String> = outputs.iter().map(Output::name).collect();
        self.instances.retain(|monitor, _| names.contains(monitor));
        self.failed.retain(|monitor| names.contains(monitor));
        if crate::scripted::builds_here() {
            for monitor in names {
                if self.instances.contains_key(&monitor) || self.failed.contains(&monitor) {
                    continue;
                }
                match self.build(&monitor, Kind::Own) {
                    Some(mut instance) => {
                        instance.surface.set_quiet(!live);
                        self.instances.insert(monitor, instance);
                    }
                    None => {
                        self.failed.insert(monitor);
                    }
                }
            }
        }
        if live != self.live {
            for instance in self.instances.values_mut() {
                instance.surface.set_quiet(!live);
            }
            self.live = live;
        }
    }

    /// What `lock.scene` comes to on this machine.
    fn resolve(&self) -> Resolved {
        let shipped_directory = self.shipped.parent().unwrap_or(Path::new(""));
        match find_scene(
            &self.settings.scene,
            self.user_directory().as_deref(),
            shipped_directory,
        ) {
            None => Resolved::Missing,
            Some(path) if path == self.shipped => Resolved::Shipped,
            Some(path) => Resolved::Own(path),
        }
    }

    /// The user's `qml/lock/`.
    #[cfg(not(test))]
    fn user_directory(&self) -> Option<PathBuf> {
        crate::qml::user_qml_dir().map(|directory| directory.join("lock"))
    }

    #[cfg(test)]
    fn user_directory(&self) -> Option<PathBuf> {
        self.user.clone()
    }

    /// The scene a monitor would be shown if every scene loaded.
    fn first_choice(&self) -> Kind {
        match self.resolve() {
            Resolved::Own(_) => Kind::Own,
            Resolved::Shipped | Resolved::Missing if self.shipped.is_file() => Kind::Shipped,
            Resolved::Shipped | Resolved::Missing => Kind::Fallback,
        }
    }

    /// The first scene down the chain from `from` that loads on `monitor`:
    /// the configuration's own, the shipped one, the fallback. `None`, and
    /// a line in the log, when not even the fallback will.
    fn build(&self, monitor: &str, from: Kind) -> Option<Instance> {
        if from == Kind::Own {
            match self.resolve() {
                Resolved::Own(path) if !path.is_file() => tracing::error!(
                    scene = %path.display(),
                    monitor,
                    "the lock scene is not there; the shipped one stands in"
                ),
                Resolved::Own(path) => match ShellSurface::hosted(path.clone(), "{}", monitor) {
                    Ok(surface) => {
                        return Some(Instance {
                            surface,
                            kind: Kind::Own,
                        });
                    }
                    Err(err) => tracing::error!(
                        ?err,
                        scene = %path.display(),
                        monitor,
                        "the lock scene would not load; the shipped one stands in"
                    ),
                },
                Resolved::Missing => tracing::error!(
                    scene = self.settings.scene,
                    monitor,
                    "no lock scene is called this, in your qml/lock/ or Solium's; \
                     the shipped one stands in"
                ),
                Resolved::Shipped => {}
            }
        }
        if from <= Kind::Shipped {
            if self.shipped.is_file() {
                match ShellSurface::hosted(self.shipped.clone(), "{}", monitor) {
                    Ok(surface) => {
                        return Some(Instance {
                            surface,
                            kind: Kind::Shipped,
                        });
                    }
                    Err(err) => tracing::error!(
                        ?err,
                        scene = %self.shipped.display(),
                        monitor,
                        "the shipped lock scene would not load; the fallback stands in"
                    ),
                }
            } else {
                tracing::error!(
                    scene = %self.shipped.display(),
                    monitor,
                    "the shipped lock scene is not there; the fallback stands in"
                );
            }
        }
        fallback(monitor)
    }

    /// What the lock shows on `output`.
    #[cfg(test)]
    pub(crate) fn shown_on(&self, output: &Output) -> Shown {
        match self
            .instances
            .get(&output.name())
            .map(|instance| instance.kind)
        {
            Some(Kind::Own) => Shown::Own,
            Some(Kind::Shipped) => Shown::Shipped,
            Some(Kind::Fallback) => Shown::Fallback,
            None => Shown::Backdrop,
        }
    }

    /// Draw `output`'s scene across `area`, in the monitor's own coordinates,
    /// and swap it for the next one down the chain if it failed to render:
    /// the next frame is that one's.
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
            // Asked for at once, so the stand-in is on the next frame rather
            // than whenever something else damages the screen.
            drawn.animating = true;
        }
        drawn
    }

    /// After `monitor`'s scene has drawn: a scene that failed to render is
    /// replaced by the next one down the chain, and the fallback by the
    /// backdrop. Whether the scene was replaced.
    /// `tests::a_scene_that_fails_at_run_time_is_swapped_down_the_chain`.
    fn after_draw(&mut self, monitor: &str) -> bool {
        let Some(instance) = self.instances.get(monitor) else {
            return false;
        };
        if !instance.surface.failing() {
            return false;
        }
        let next = match instance.kind {
            Kind::Own => {
                tracing::error!(
                    monitor,
                    scene = self.settings.scene,
                    "the lock scene failed while it ran; the shipped one stands in"
                );
                Some(Kind::Shipped)
            }
            Kind::Shipped => {
                tracing::error!(
                    monitor,
                    "the shipped lock scene failed while it ran; the fallback stands in"
                );
                Some(Kind::Fallback)
            }
            Kind::Fallback => {
                tracing::error!(
                    monitor,
                    "the lock's fallback scene failed while it ran; the backdrop stands in"
                );
                None
            }
        };
        match next.and_then(|from| self.build(monitor, from)) {
            Some(mut instance) => {
                instance.surface.set_quiet(!self.live);
                self.instances.insert(monitor.to_owned(), instance);
            }
            None => {
                self.instances.remove(monitor);
                self.failed.insert(monitor.to_owned());
            }
        }
        true
    }

    /// The lock ended at `now`: its scenes stay over the desktop for
    /// `lock.exit_ms`, if there are any and it is not 0. The pointer at them
    /// is forgotten, since it is the desktop's now.
    pub(crate) fn begin_exit(&mut self, now: Duration) {
        self.pointer = LockPointer::default();
        if self.settings.exit.is_zero() || self.instances.is_empty() {
            self.exit_until = None;
            return;
        }
        self.exit_until = Some(now + self.settings.exit);
    }

    /// Whether the scenes are being drawn over the desktop at `now`.
    pub(crate) fn exiting(&self, now: Duration) -> bool {
        self.exit_until.is_some_and(|until| now < until)
    }

    /// Whether an exit under way has reached its bound by `now`.
    fn exit_over(&self, now: Duration) -> bool {
        self.exit_until.is_some_and(|until| now >= until)
    }

    /// End the exit, whether or not it is over: the session locked again.
    pub(crate) fn end_exit(&mut self) {
        self.exit_until = None;
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

    /// Show the scene at `path` from the next sync on, as `lock.scene`
    /// naming it would, with every scene built anew.
    #[cfg(test)]
    pub(crate) fn use_scene(&mut self, path: PathBuf) {
        self.settings.scene = path.to_string_lossy().into_owned();
        self.instances.clear();
        self.failed.clear();
    }

    /// Show the fallback from the next sync on: no scene of the user's and
    /// no shipped one to be found.
    #[cfg(test)]
    pub(crate) fn use_fallback(&mut self) {
        self.use_scene(PathBuf::from("/nonexistent/solium/lock/Own.qml"));
        self.shipped = PathBuf::from("/nonexistent/solium/qml/lock/Lock.qml");
    }

    /// Take `directory` as the user's `qml/lock/`.
    #[cfg(test)]
    pub(crate) fn use_user_directory(&mut self, directory: PathBuf) {
        self.user = Some(directory);
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
            kind: Kind::Fallback,
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

/// Once a loop, in both backends: an exit that has reached its bound ends,
/// and the scenes go quiet, whatever they were doing.
/// `lock_auth::tests::the_exit_plays_over_the_desktop_for_exit_ms_and_input_is_the_desktops_at_once`.
pub(crate) fn settle(state: &mut Solium) {
    let now = state.clock.now();
    if !state.lock_scenes.exit_over(now) {
        return;
    }
    state.lock_scenes.end_exit();
    state.sync_lock_scenes();
    // `Lock.state` reads "idle" again, for the next lock.
    state.lock_auth.touch();
    state.redraw = true;
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

    /// A directory of the test's own, emptied.
    fn directory(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("solium-lock-scene-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a directory for the scene");
        directory
    }

    /// A scene file of `source`, in a directory of the test's own.
    fn scene(name: &str, source: &str) -> PathBuf {
        let path = directory(name).join("Lock.qml");
        std::fs::write(&path, source).expect("writing the scene");
        path
    }

    /// **The fallback is in the binary**: it loads from the text compiled
    /// in, whatever is on disk, here under a path where there is no file at
    /// all, and it is what the lock shows when no other scene is there.
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
            scenes.use_fallback();
            scenes.sync(std::slice::from_ref(&output), true);
            assert_eq!(scenes.shown_on(&output), Shown::Fallback);
        });
    }

    /// **The shipped scene is the default, and it loads**: with nothing
    /// configured, `lock.scene` is the shipped `Lock.qml`, and it builds
    /// without a word from QML.
    #[test]
    fn the_shipped_scene_is_the_default_and_loads_cleanly() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let output = monitor("lock-shipped");
            let mut scenes = LockScenes::default();
            let mark = crate::qml::warning_mark();
            scenes.sync(std::slice::from_ref(&output), true);
            assert_eq!(scenes.shown_on(&output), Shown::Shipped);
            scenes
                .instance_for_test(&output)
                .expect("the shipped scene")
                .scene_for_test()
                .resize(1920, 1080, 1.0);
            assert_eq!(
                crate::qml::warnings_since(mark),
                0,
                "the shipped lock scene warned as it loaded"
            );
        });
    }

    /// **A broken scene falls back to the shipped one, and then to the
    /// fallback, never to the desktop**: a scene that will not load and one
    /// that is not there both show the shipped scene; with the shipped one
    /// broken as well, the fallback. A scene that loads is shown.
    #[test]
    fn a_broken_scene_falls_back_to_the_shipped_one_and_then_to_the_fallback() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let output = monitor("lock-broken");
            let mut scenes = LockScenes::default();

            scenes.use_scene(scene(
                "broken",
                "import QtQuick\nItem { this is not QML }\n",
            ));
            scenes.sync(std::slice::from_ref(&output), true);
            assert_eq!(scenes.shown_on(&output), Shown::Shipped, "a broken scene");

            scenes.use_scene(PathBuf::from("/nonexistent/Lock.qml"));
            scenes.sync(std::slice::from_ref(&output), true);
            assert_eq!(scenes.shown_on(&output), Shown::Shipped, "a missing scene");

            scenes.use_scene(scene("working", "import QtQuick\nItem {}\n"));
            scenes.sync(std::slice::from_ref(&output), true);
            assert_eq!(scenes.shown_on(&output), Shown::Own, "a scene that loads");

            scenes.use_scene(scene("broken-too", "import QtQuick\nItem { nor this }\n"));
            scenes.shipped = scene("broken-shipped", "import QtQuick\nItem { nor that }\n");
            scenes.sync(std::slice::from_ref(&output), true);
            assert_eq!(
                scenes.shown_on(&output),
                Shown::Fallback,
                "a broken scene, and a broken shipped one"
            );
        });
    }

    /// **A scene that fails at run time is swapped down the chain**: the
    /// configuration's own for the shipped one, that for the fallback, and
    /// the fallback for the backdrop. Played through the check made after
    /// every draw, with the failure a render would have reported: a render
    /// needs a GPU this test cannot have. When the lock ends, the scene the
    /// configuration names is built again.
    #[test]
    fn a_scene_that_fails_at_run_time_is_swapped_down_the_chain() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let output = monitor("lock-run-time");
            let mut scenes = LockScenes::default();
            scenes.use_scene(scene("run-time", "import QtQuick\nItem {}\n"));
            scenes.sync(std::slice::from_ref(&output), true);
            assert_eq!(scenes.shown_on(&output), Shown::Own, "the premise");
            assert!(
                !scenes.after_draw(&output.name()),
                "a scene that drew stays"
            );

            for (stand_in, what) in [
                (Shown::Shipped, "the own scene failed"),
                (Shown::Fallback, "the shipped one failed"),
                (Shown::Backdrop, "the fallback failed"),
            ] {
                scenes
                    .instance_for_test(&output)
                    .expect("a scene")
                    .fail_for_test();
                assert!(scenes.after_draw(&output.name()));
                assert_eq!(scenes.shown_on(&output), stand_in, "{what}");
            }
            scenes.sync(std::slice::from_ref(&output), true);
            assert_eq!(
                scenes.shown_on(&output),
                Shown::Backdrop,
                "a monitor nothing would draw on was tried again while the lock lasts"
            );
            scenes.sync(std::slice::from_ref(&output), false);
            assert_eq!(
                scenes.shown_on(&output),
                Shown::Own,
                "the lock ended, and the scene the configuration names was not built again"
            );
        });
    }

    /// **The scenes are built ahead and kept between locks**, while
    /// `lock.native` is on: one per monitor before any lock, the same ones
    /// through a lock and after it, one more for a monitor plugged in and
    /// one fewer for one unplugged. With `lock.native` off none are kept,
    /// and a lock still gets its scenes.
    #[test]
    fn the_scenes_are_built_ahead_kept_between_locks_and_follow_the_monitors() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let (left, right) = (monitor("lock-left"), monitor("lock-right"));
            let mut scenes = LockScenes::default();
            scenes.use_scene(scene(
                "kept",
                "import QtQuick\nItem { property int mark: 0 }\n",
            ));
            scenes.sync(std::slice::from_ref(&left), false);
            assert_eq!(
                scenes.shown_on(&left),
                Shown::Own,
                "built ahead of the lock"
            );
            scenes
                .instance_for_test(&left)
                .expect("the kept scene")
                .scene_for_test()
                .set_int("mark", 7);

            scenes.sync(std::slice::from_ref(&left), true);
            scenes.sync(std::slice::from_ref(&left), false);
            assert_eq!(
                scenes
                    .instance_for_test(&left)
                    .expect("the kept scene")
                    .scene_for_test()
                    .get_int("mark"),
                7,
                "the scene was built again rather than kept"
            );

            scenes.sync(&[left.clone(), right.clone()], false);
            assert_eq!(scenes.shown_on(&right), Shown::Own, "plugged in");
            scenes.sync(std::slice::from_ref(&right), false);
            assert_eq!(scenes.shown_on(&left), Shown::Backdrop, "unplugged");

            scenes.configure(Settings {
                native: false,
                ..scenes.settings().clone()
            });
            scenes.sync(std::slice::from_ref(&right), false);
            assert_eq!(scenes.count(), 0, "kept with `lock.native` off");
            scenes.sync(std::slice::from_ref(&right), true);
            assert_eq!(scenes.count(), 1, "a lock with `lock.native` off");
            scenes.sync(std::slice::from_ref(&right), false);
            assert_eq!(scenes.count(), 0, "and dropped at its end");
        });
    }

    /// **A kept scene neither ticks nor wakes while the session is
    /// unlocked**: a Timer and a looping animation that run from the start
    /// are stopped while it is kept, and run again while it is shown.
    #[test]
    fn a_kept_scene_neither_ticks_nor_wakes_while_unlocked() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let output = monitor("lock-quiet");
            let mut scenes = LockScenes::default();
            scenes.use_scene(scene(
                "quiet",
                r"import QtQuick
                Item {
                    Timer { interval: 1000; running: true; repeat: true }
                    Rectangle {
                        width: 10; height: 10
                        NumberAnimation on x {
                            from: 0; to: 100; duration: 1000; loops: Animation.Infinite
                        }
                    }
                }",
            ));
            let ticking = |scenes: &mut LockScenes| {
                scenes
                    .instance_for_test(&output)
                    .expect("the scene")
                    .scene_for_test()
                    .ticking_for_test()
            };
            scenes.sync(std::slice::from_ref(&output), false);
            assert!(!ticking(&mut scenes), "a kept scene ticks while unlocked");
            scenes.sync(std::slice::from_ref(&output), true);
            assert!(ticking(&mut scenes), "the shown scene does not tick");
            scenes.sync(std::slice::from_ref(&output), false);
            assert!(!ticking(&mut scenes), "the scene ticks after the lock");
        });
    }

    /// **A bare name is looked for in the user's directory first**, then in
    /// the shipped one, with `.qml` added; a path is taken as given; a name
    /// in neither is nothing. And the scene a name finds in the user's
    /// directory is the one a lock shows, over the shipped one.
    #[test]
    fn a_bare_name_is_looked_for_in_the_users_directory_first() {
        let user = directory("user");
        let shipped = directory("shipped");
        for place in [&user, &shipped] {
            std::fs::write(place.join("Lock.qml"), "import QtQuick\nItem {}\n").expect("a scene");
        }
        std::fs::write(shipped.join("Plain.qml"), "import QtQuick\nItem {}\n").expect("a scene");

        assert_eq!(
            find_scene("Lock", Some(&user), &shipped),
            Some(user.join("Lock.qml"))
        );
        assert_eq!(
            find_scene("Lock.qml", None, &shipped),
            Some(shipped.join("Lock.qml"))
        );
        assert_eq!(
            find_scene("Plain", Some(&user), &shipped),
            Some(shipped.join("Plain.qml"))
        );
        assert_eq!(find_scene("Neon", Some(&user), &shipped), None);
        assert_eq!(find_scene("", Some(&user), &shipped), None);
        assert_eq!(
            find_scene("/somewhere/Mine.qml", Some(&user), &shipped),
            Some(PathBuf::from("/somewhere/Mine.qml"))
        );
        let _ = std::fs::remove_dir_all(&shipped);

        crate::qml::qt_test::on_the_qt_thread(move || {
            crate::qml::start().expect("Qt starts");
            let output = monitor("lock-users-own");
            let mut scenes = LockScenes::default();
            scenes.use_user_directory(user.clone());
            scenes.sync(std::slice::from_ref(&output), true);
            assert_eq!(
                scenes.shown_on(&output),
                Shown::Own,
                "the user's own `Lock.qml` is not the one shown"
            );
            let _ = std::fs::remove_dir_all(&user);
        });
    }
}
