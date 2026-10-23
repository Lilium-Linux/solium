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
//! **The entry.** When Solium's own lock is taken, the session is locked at
//! once: input is the lock's, and a capture sees nothing of the session. The
//! scenes are live from that instant and read `Lock.state` as `"locking"`
//! for `lock.enter_ms`, so a scene can come down over the desktop like a
//! sheet; and for that long only, the desktop is still drawn under the
//! scenes on the monitors that were lit when the lock was taken and show a
//! scene that is not the fallback. At the bound the entry is over whatever
//! the scene is doing: the backdrop is under every scene, and only from then
//! on is a monitor's frame one that shows the lock, so `locked` (logind's
//! `LockedHint`, and the inhibitor held for sleep) waits for the entry. A
//! scene that fails during the entry is swapped down the chain with the
//! backdrop under its stand-in at once, and the lock taken before sleep, or
//! with no monitor lit, has no entry at all
//! (`state::tests::real_client::lock_focus::native::entry`).
//!
//! **The exit.** When Solium's own lock ends, the session is unlocked at once:
//! input goes to the desktop and the desktop is drawn. The lock's scenes are
//! drawn over it for `lock.exit_ms` more, reading `Lock.state` as
//! `"unlocking"`, so a scene can slide or fade itself away; at that bound they
//! are gone, whatever the scene is doing
//! (`lock_auth::tests::the_exit_plays_over_the_desktop_for_exit_ms_and_input_is_the_desktops_at_once`).
//!
//! **The peek.** While the session is unlocked, the configuration can pull
//! the lock's scenes down over the desktop with a finger, as an iPhone's lock
//! follows the finger pulled down from the top: `sol.lock_peek(progress)`
//! makes them live and draws them over the desktop, reading `Lock.state` as
//! `"peeking"` and `Lock.progress` as how far down, from 0 to 1. It is a
//! picture and nothing more: nothing is locked, input stays the session's
//! (the configuration's own gesture holds the finger), and the scenes read
//! the redacted models they always do. `sol.lock_peek(false)` ends it with
//! the exit, reading `"unlocking"` for `lock.exit_ms` at most, and
//! `sol.lock_now()` takes the lock from it, its entry going on from the
//! peek's progress for what is left of `lock.enter_ms`. Only scenes that are
//! not the fallback are drawn in a peek, as in an entry: the fallback does
//! not move, and would cover the desktop at the finger's first pixel
//! (`state::tests::real_client::lock_focus::native::peek`).
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

/// `lock.exit_ms`'s default: §4.16's bounded exit, which the shipped scene
/// spends taking its sheet back up and off the top.
pub(crate) const EXIT_MS: u32 = 250;

/// `lock.enter_ms`'s default: the sheet coming down over the desktop, about
/// as long as an iPhone's lock takes.
pub(crate) const ENTER_MS: u32 = 350;

/// The longest `lock.enter_ms` may be. Input is the lock's from the first
/// instant, but the desktop is still on screen through the entry and
/// `locked` waits for it, so no longer than a glance, as the exit.
pub(crate) const ENTER_MAX_MS: u32 = 600;

/// `lock.keypad_margin`'s default, in logical pixels: about a fingertip, so
/// the pointer's way to a key is hidden from the scene as well as its path
/// across the pad.
pub(crate) const KEYPAD_MARGIN: u32 = 32;

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
    /// `lock.enter_ms`: how long the scenes come down over the desktop as
    /// the lock is taken, at most [`ENTER_MAX_MS`].
    pub(crate) enter: Duration,
    /// `lock.exit_ms`: how long the scenes are drawn over the desktop after
    /// the unlock, at most [`EXIT_MAX_MS`].
    pub(crate) exit: Duration,
    /// `lock.keypad_margin`: how far around a `SecretKeypad`, in logical
    /// pixels, the pointer's motion is kept from the scene.
    pub(crate) keypad_margin: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            native: true,
            scene: SHIPPED.to_owned(),
            prompt_on: PromptOn::Pointer,
            enter: Duration::from_millis(u64::from(ENTER_MS)),
            exit: Duration::from_millis(u64::from(EXIT_MS)),
            keypad_margin: KEYPAD_MARGIN,
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
    /// lock, through a peek and through an exit. Quiet otherwise.
    live: bool,
    /// Whether every scene is to be built again once they are quiet: the
    /// configuration named another scene.
    rebuild: bool,
    /// When the entry under way ends, on the compositor's clock.
    enter_until: Option<Duration>,
    /// The monitors the desktop is drawn on under the scene through the
    /// entry: lit when the lock was taken, and showing a scene that is not
    /// the fallback and has not failed since.
    entering_on: HashSet<String>,
    /// Where the entry under way started, from 0 to 1: the peek it took the
    /// lock from (`Lock.progress` through it), 0 for a lock taken from none.
    enter_from: f64,
    /// When the exit under way ends, on the compositor's clock.
    exit_until: Option<Duration>,
    /// Whether the exit under way ends a peek rather than a lock, so the
    /// fallback, which a peek never draws, is not drawn through it either.
    exit_after_peek: bool,
    /// The peek under way: how far down the configuration has pulled the
    /// scenes over the unlocked desktop, from 0 to 1 (`sol.lock_peek`).
    peek: Option<f64>,
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
            enter_until: None,
            entering_on: HashSet::new(),
            enter_from: 0.0,
            exit_until: None,
            exit_after_peek: false,
            peek: None,
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
        self.entering_on.retain(|monitor| names.contains(monitor));
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
                Resolved::Own(path) => match ShellSurface::for_lock(path.clone(), monitor) {
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
                match ShellSurface::for_lock(self.shipped.clone(), monitor) {
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
        // Its stand-in covers at once: no more of the desktop under it.
        self.entering_on.remove(monitor);
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

    /// The lock was taken at `now`, on the monitors `lit` names, from a peek
    /// `from` of the way down (0 for none): its scenes come down over the
    /// desktop for what is left of `lock.enter_ms`, `1 - from` of it, if
    /// that is not 0 and a monitor is lit. Which monitors draw the desktop
    /// under their scene is settled by [`Self::keep_entry_where_scenes_are`],
    /// once the scenes are there.
    pub(crate) fn begin_entry(&mut self, now: Duration, lit: Vec<String>, from: f64) {
        self.end_entry();
        let from = if from.is_finite() {
            from.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let enter = self.settings.enter;
        let left = if from <= 0.0 {
            enter
        } else {
            Duration::try_from_secs_f64(enter.as_secs_f64() * (1.0 - from))
                .unwrap_or(Duration::ZERO)
                .min(enter)
        };
        if left.is_zero() || lit.is_empty() {
            return;
        }
        self.enter_until = Some(now + left);
        self.entering_on = lit.into_iter().collect();
        self.enter_from = from;
    }

    /// Where the entry under way started, from 0 to 1: `Lock.progress`
    /// through it. 0 with no entry.
    pub(crate) fn entry_from(&self) -> f64 {
        if self.enter_until.is_some() {
            self.enter_from
        } else {
            0.0
        }
    }

    /// Keep the entry only on the monitors that show a scene that is not the
    /// fallback, which stays still and covers at once; with none left, there
    /// is no entry.
    pub(crate) fn keep_entry_where_scenes_are(&mut self) {
        let instances = &self.instances;
        self.entering_on.retain(|monitor| {
            instances
                .get(monitor)
                .is_some_and(|instance| instance.kind != Kind::Fallback)
        });
        if self.entering_on.is_empty() {
            self.end_entry();
        }
    }

    /// Whether the scenes are reading `"locking"` at `now`.
    pub(crate) fn entering(&self, now: Duration) -> bool {
        self.enter_until.is_some_and(|until| now < until)
    }

    /// Whether an entry is under way and not yet ended by [`settle`]: what
    /// `locked` waits for, so a frame built under it is never one that shows
    /// the lock, even one presented after its bound.
    pub(crate) fn entry_pending(&self) -> bool {
        self.enter_until.is_some()
    }

    /// Whether an entry under way has reached its bound by `now`.
    fn enter_over(&self, now: Duration) -> bool {
        self.enter_until.is_some_and(|until| now >= until)
    }

    /// End the entry, whether or not it is over.
    pub(crate) fn end_entry(&mut self) {
        self.enter_until = None;
        self.entering_on.clear();
        self.enter_from = 0.0;
    }

    /// Whether the desktop is drawn under `monitor`'s scene at `now`: during
    /// the entry, on a monitor it plays on, showing a scene that is not the
    /// fallback.
    pub(crate) fn desktop_under(&self, monitor: &str, now: Duration) -> bool {
        self.entering(now)
            && self.entering_on.contains(monitor)
            && self
                .instances
                .get(monitor)
                .is_some_and(|instance| instance.kind != Kind::Fallback)
    }

    /// Whether the desktop is drawn under any monitor's scene at `now`.
    pub(crate) fn desktop_under_any(&self, now: Duration) -> bool {
        self.entering_on
            .iter()
            .any(|monitor| self.desktop_under(monitor, now))
    }

    /// The lock ended at `now`: its scenes stay over the desktop for
    /// `lock.exit_ms`, if there are any and it is not 0. The pointer at them
    /// is forgotten, since it is the desktop's now.
    pub(crate) fn begin_exit(&mut self, now: Duration) {
        self.end_entry();
        self.peek = None;
        self.exit_after_peek = false;
        self.pointer = LockPointer::default();
        if self.settings.exit.is_zero() || self.instances.is_empty() {
            self.exit_until = None;
            return;
        }
        self.exit_until = Some(now + self.settings.exit);
    }

    /// Pull the scenes `progress` of the way down over the unlocked desktop
    /// (0 to 1; anything else is held to that): the peek begins, or follows
    /// the finger. An exit still playing is over: the peek has the scenes
    /// now, from where they are. For [`Solium::lock_peek`], which decides
    /// whether a peek may be.
    pub(crate) fn peek(&mut self, progress: f64) {
        self.exit_until = None;
        self.exit_after_peek = false;
        self.peek = Some(progress.clamp(0.0, 1.0));
    }

    /// How far down the peek under way is, or `None` with none.
    pub(crate) fn peeking(&self) -> Option<f64> {
        self.peek
    }

    /// End the peek at `now`, if there is one: its scenes go back up through
    /// the exit, for `lock.exit_ms` at most. Where it was, or `None` with no
    /// peek.
    pub(crate) fn end_peek(&mut self, now: Duration) -> Option<f64> {
        let was = self.peek?;
        self.begin_exit(now);
        self.exit_after_peek = self.exit_until.is_some();
        Some(was)
    }

    /// Drop the peek with no exit, because the session is being locked:
    /// where it was, or `None` with no peek.
    pub(crate) fn take_peek(&mut self) -> Option<f64> {
        self.peek.take()
    }

    /// Whether `monitor`'s scene is drawn over the unlocked desktop at `now`:
    /// through a peek, and through the exit after a lock or a peek; never the
    /// fallback for a peek, which does not move.
    pub(crate) fn over_desktop(&self, monitor: &str, now: Duration) -> bool {
        let Some(instance) = self.instances.get(monitor) else {
            return false;
        };
        let moves = instance.kind != Kind::Fallback;
        if self.peek.is_some() {
            return moves;
        }
        self.exiting(now) && (moves || !self.exit_after_peek)
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
        self.exit_after_peek = false;
    }

    /// Whether the scenes are live: drawn and ticking.
    #[cfg(test)]
    pub(crate) fn is_live(&self) -> bool {
        self.live
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

    /// Whether `location` on `monitor`'s scene, drawn across `area`, is
    /// within `lock.keypad_margin` of a shown `SecretKeypad`: where the
    /// scene is told nothing of the pointer's motion.
    pub(crate) fn near_keypad(
        &self,
        monitor: &str,
        area: Rectangle<i32, Logical>,
        location: Point<f64, Logical>,
    ) -> bool {
        let margin = f64::from(self.settings.keypad_margin);
        self.instances
            .get(monitor)
            .is_some_and(|instance| instance.surface.near_keypad(area, location, margin))
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
    match ShellSurface::compiled_for_lock(fallback_path(), FALLBACK, monitor) {
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

/// Once a loop, in both backends: an entry that has reached its bound ends,
/// and the lock is then confirmed as for any frame, from the frames built
/// after it; and an exit that has reached its bound ends, and the scenes go
/// quiet, whatever they were doing.
/// `lock_auth::tests::the_entry_is_bounded_even_if_the_scene_never_finishes`,
/// `lock_auth::tests::the_exit_plays_over_the_desktop_for_exit_ms_and_input_is_the_desktops_at_once`.
pub(crate) fn settle(state: &mut Solium) {
    let now = state.clock.now();
    if state.lock_scenes.enter_over(now) {
        state.end_lock_entry();
    }
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
            let built = ShellSurface::compiled_for_lock(nowhere, FALLBACK, "fallback-test");
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

    /// **A scene that fails during the entry is swapped down the chain, and
    /// its stand-in covers at once**: the desktop is drawn under a scene
    /// coming down, and under no stand-in for it, though the entry goes on
    /// for the other monitors. Nor under the fallback, which stays still,
    /// nor on a monitor that was not lit, or not there, when the lock was
    /// taken; and with no scene but the fallback there is no entry at all.
    #[test]
    fn a_scene_that_fails_during_the_entry_is_covered_at_once() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let (left, right, dark) = (
                monitor("entry-left"),
                monitor("entry-right"),
                monitor("entry-dark"),
            );
            let all = [left.clone(), right.clone(), dark.clone()];
            let mut scenes = LockScenes::default();
            scenes.use_scene(scene("entry", "import QtQuick\nItem {}\n"));
            let now = Duration::from_secs(10);
            scenes.begin_entry(now, vec![left.name(), right.name()], 0.0);
            scenes.sync(&all, true);
            scenes.keep_entry_where_scenes_are();
            let at = now + Duration::from_millis(1);
            assert!(
                scenes.desktop_under(&left.name(), at),
                "the premise: an entry"
            );
            assert!(scenes.desktop_under(&right.name(), at));
            assert!(
                !scenes.desktop_under(&dark.name(), at),
                "the desktop under a monitor that was dark at the lock"
            );

            scenes
                .instance_for_test(&left)
                .expect("a scene")
                .fail_for_test();
            assert!(scenes.after_draw(&left.name()));
            assert_eq!(
                scenes.shown_on(&left),
                Shown::Shipped,
                "the premise: a stand-in"
            );
            assert!(
                !scenes.desktop_under(&left.name(), at),
                "the desktop under the stand-in for a scene that failed in its entry"
            );
            assert!(
                scenes.desktop_under(&right.name(), at),
                "the entry ended on the monitor whose scene did not fail"
            );
            let plugged = monitor("entry-plugged");
            scenes.sync(&[left.clone(), right.clone(), plugged.clone()], true);
            assert!(
                !scenes.desktop_under(&plugged.name(), at),
                "the desktop under a monitor plugged in during the entry"
            );

            let mut fallback = LockScenes::default();
            fallback.use_fallback();
            fallback.begin_entry(now, vec![left.name()], 0.0);
            fallback.sync(std::slice::from_ref(&left), true);
            fallback.keep_entry_where_scenes_are();
            assert!(
                !fallback.entry_pending() && !fallback.desktop_under(&left.name(), at),
                "the fallback has an entry"
            );
        });
    }

    /// **A peek draws a scene that moves, and never the fallback**: pulled
    /// down, a monitor showing a scene is drawn over the desktop and one
    /// showing the fallback, which would cover it all at the first pixel, is
    /// not, nor through the way back up after the peek; the exit after a
    /// lock draws either. A peek begun during an exit takes it over, and a
    /// lock taken from a peek comes down for what is left of `enter_ms`.
    #[test]
    fn a_peek_draws_a_scene_that_moves_and_never_the_fallback() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let output = monitor("peek-own");
            let now = Duration::from_secs(10);
            let mut own = LockScenes::default();
            own.use_scene(scene("peek", "import QtQuick\nItem {}\n"));
            own.sync(std::slice::from_ref(&output), false);
            let mut fallback = LockScenes::default();
            fallback.use_fallback();
            fallback.sync(std::slice::from_ref(&output), false);
            assert_eq!(fallback.shown_on(&output), Shown::Fallback, "the premise");

            for scenes in [&mut own, &mut fallback] {
                assert!(
                    !scenes.over_desktop(&output.name(), now),
                    "drawn with no peek"
                );
                scenes.peek(0.4);
            }
            assert!(
                own.over_desktop(&output.name(), now),
                "a scene is not drawn in a peek"
            );
            assert!(
                !fallback.over_desktop(&output.name(), now),
                "the fallback is drawn in a peek"
            );
            for scenes in [&mut own, &mut fallback] {
                assert_eq!(scenes.end_peek(now), Some(0.4));
            }
            let at = now + Duration::from_millis(1);
            assert!(own.over_desktop(&output.name(), at), "no way back up");
            assert!(
                !fallback.over_desktop(&output.name(), at),
                "the fallback is drawn on the way back up from a peek"
            );

            // After a lock, the exit draws either; a peek then takes it over.
            fallback.end_exit();
            fallback.begin_exit(now);
            assert!(
                fallback.over_desktop(&output.name(), at),
                "the exit after a lock does not draw the fallback"
            );
            own.peek(0.2);
            assert!(!own.exiting(at), "a peek did not take over from the exit");
            assert_eq!(own.peeking(), Some(0.2));

            // A lock taken from the peek: what is left of the entry.
            let from = own.take_peek().unwrap_or_default();
            own.begin_entry(now, vec![output.name()], from);
            let left = own.settings().enter.mul_f64(0.8);
            assert!(own.entering(now + left - Duration::from_millis(1)));
            assert!(
                !own.entering(now + left + Duration::from_millis(1)),
                "the entry from a peek lasted the whole of `enter_ms`"
            );
            assert!((own.entry_from() - 0.2).abs() < 1e-9);
            own.begin_entry(now, vec![output.name()], 1.0);
            assert!(
                !own.entry_pending(),
                "an entry from a peek all the way down"
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
